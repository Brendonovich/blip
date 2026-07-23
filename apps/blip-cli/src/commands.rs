use std::error::Error;
use std::sync::mpsc;
use std::time::Duration;

use blip_sck::{
    Application, CaptureError, CaptureFilter, Capturer, Display, DisplayMode, ShareableContent,
    StreamConfig, Window, has_permission, request_permission,
};

use crate::cli::{Cli, Command, ResourceCommand, StreamArgs};
use crate::recording;

pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

enum StreamEnd {
    Interrupt,
    Error(String),
}

pub(crate) fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Command::Displays {
            command: ResourceCommand::List,
        } => list_displays(),
        Command::Displays {
            command: ResourceCommand::Info { id },
        } => display_info(id),
        Command::Windows {
            command: ResourceCommand::List,
        } => list_windows(),
        Command::Windows {
            command: ResourceCommand::Info { id },
        } => window_info(id),
        Command::Stream(args) => stream(&args),
        Command::Record(args) => recording::record(&args),
    }
}

fn list_displays() -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    let main_display = content.main_display().map(|display| display.id());
    for display in content.displays() {
        let (width, height) = display_capture_dimensions(&display);
        let marker = if Some(display.id()) == main_display {
            "main"
        } else {
            ""
        };
        println!("{}\t{}x{}\t{marker}", display.id(), width, height);
    }
    Ok(())
}

fn list_windows() -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    for window in content.application_windows() {
        let dimensions = window_capture_dimensions(&window);
        let size = dimensions.map_or_else(
            || "unknown".into(),
            |(width, height)| format!("{width}x{height}"),
        );
        let application = window
            .application()
            .map_or_else(|| "unknown".into(), |application| application.name());
        let title = window.title().unwrap_or_default();
        println!(
            "{}\t{size}\t{}\t{}",
            window.id(),
            sanitize(&application),
            sanitize(&title)
        );
    }
    Ok(())
}

fn display_info(display_id: u32) -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    let display = content
        .displays()
        .into_iter()
        .find(|display| display.id() == display_id)
        .ok_or_else(|| format!("display {display_id} is not available"))?;
    let (logical_width, logical_height) = display.logical_size();
    let (capture_width, capture_height) = display_capture_dimensions(&display);
    let (frame_x, frame_y, frame_width, frame_height) = display.frame();

    println!("id: {}", display.id());
    println!("main: {}", display.is_main());
    println!("built-in: {}", display.is_builtin());
    println!("active: {}", display.is_active());
    println!("online: {}", display.is_online());
    println!("rotation: {:.0} degrees", display.rotation_degrees());
    println!("logical-size: {logical_width}x{logical_height}");
    println!(
        "physical-size: {}x{}",
        display.physical_width(),
        display.physical_height()
    );
    println!("capture-size: {capture_width}x{capture_height}");
    println!("frame: {frame_width:.0}x{frame_height:.0}+{frame_x:.0}+{frame_y:.0}");
    if let Some(mode) = display.current_mode() {
        print_mode("current-mode", mode);
    }
    println!("available-modes:");
    for mode in display.available_modes() {
        print_mode("  -", mode);
    }
    Ok(())
}

fn window_info(window_id: u32) -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    let window = content
        .application_windows()
        .into_iter()
        .find(|window| window.id() == window_id)
        .ok_or_else(|| format!("window {window_id} is not available"))?;
    let application = window.application();
    let (frame_x, frame_y, frame_width, frame_height) = window.frame();
    let capture_size = window_capture_dimensions(&window).map_or_else(
        || "unknown".into(),
        |(width, height)| format!("{width}x{height}"),
    );

    println!("id: {}", window.id());
    println!("title: {}", window.title().unwrap_or_default());
    println!(
        "application: {}",
        application
            .as_ref()
            .map_or_else(|| "unknown".into(), Application::name)
    );
    println!(
        "bundle-id: {}",
        application
            .as_ref()
            .map_or_else(|| "unknown".into(), Application::bundle_identifier,)
    );
    println!("on-screen: {}", window.is_on_screen());
    println!("active: {}", window.is_active());
    println!("layer: {}", window.layer());
    println!("capture-size: {capture_size}");
    println!(
        "display: {}",
        window
            .display()
            .map_or_else(|| "unknown".into(), |display| display.id().to_string())
    );
    println!("frame: {frame_width:.0}x{frame_height:.0}+{frame_x:.0}+{frame_y:.0}");
    println!("available-fps: not reported by ScreenCaptureKit");
    Ok(())
}

fn print_mode(prefix: &str, mode: DisplayMode) {
    let (logical_width, logical_height) = mode.logical_size();
    let (pixel_width, pixel_height) = mode.pixel_size();
    let refresh_rate = mode.refresh_rate();
    let refresh_rate = if refresh_rate > 0.0 {
        format!("{refresh_rate:.2} Hz")
    } else {
        "variable/unspecified".into()
    };
    println!(
        "{prefix}: logical={logical_width}x{logical_height} pixels={pixel_width}x{pixel_height} fps={refresh_rate} io-id={} desktop-usable={}",
        mode.io_id(),
        mode.is_usable_for_desktop()
    );
}

fn stream(args: &StreamArgs) -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    let (filter, description) = select_target(&content, args)?;
    let config = StreamConfig::builder()
        .with_fps(args.fps)
        .with_cursor(args.cursor)
        .with_queue_depth(2);

    let (end_sender, end_receiver) = mpsc::sync_channel(1);
    let error_sender = end_sender.clone();
    ctrlc::set_handler(move || {
        let _ = end_sender.try_send(StreamEnd::Interrupt);
    })?;

    let mut frame_number = 0_u64;
    let capturer = Capturer::builder(filter, config)?
        .with_timeout(CAPTURE_TIMEOUT)
        .with_video_frame_callback(move |frame| {
            frame_number = frame_number.saturating_add(1);
            println!("frame {frame_number}\t{}x{}", frame.width(), frame.height());
        })
        .with_stop_callback(move |error| {
            let _ =
                error_sender.try_send(StreamEnd::Error(error.localizedDescription().to_string()));
        })
        .build()?;

    capturer.start()?;
    eprintln!("streaming {description}; press Ctrl-C to stop");
    match end_receiver.recv()? {
        StreamEnd::Interrupt => capturer.stop()?,
        StreamEnd::Error(message) => return Err(CaptureError::Framework(message).into()),
    }
    Ok(())
}

pub(crate) fn shareable_content() -> Result<ShareableContent, CaptureError> {
    if !has_permission() {
        let _ = request_permission();
        return Err(CaptureError::PermissionDenied);
    }
    ShareableContent::current(CAPTURE_TIMEOUT)
}

pub(crate) type SelectedTarget = (CaptureFilter, String);

pub(crate) fn select_target(
    content: &ShareableContent,
    args: &StreamArgs,
) -> Result<SelectedTarget, Box<dyn Error>> {
    if let Some(display_id) = args.display {
        let display = content
            .displays()
            .into_iter()
            .find(|display| display.id() == display_id)
            .ok_or_else(|| format!("display {display_id} is not available"))?;
        return Ok((
            CaptureFilter::from(display),
            format!("display {display_id}"),
        ));
    }

    if let Some(window_id) = args.window {
        let window = content
            .application_windows()
            .into_iter()
            .find(|window| window.id() == window_id)
            .ok_or_else(|| format!("window {window_id} is not available"))?;
        let filter = CaptureFilter::from(window);
        return Ok((filter, format!("window {window_id}")));
    }

    let display = content.main_display().ok_or(CaptureError::NoDisplay)?;
    let display_id = display.id();
    Ok((
        CaptureFilter::from(display),
        format!("main display {display_id}"),
    ))
}

fn display_capture_dimensions(display: &Display) -> (usize, usize) {
    CaptureFilter::from(display.clone())
        .capture_size()
        .unwrap_or_else(|| (display.physical_width(), display.physical_height()))
}

fn window_capture_dimensions(window: &Window) -> Option<(usize, usize)> {
    CaptureFilter::from(window).capture_size()
}

fn sanitize(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

use std::thread;
use std::time::Duration;

use blip_sck::ShareableContent;
use clap::ValueEnum;

use crate::profiles::{CompletionAction, RecordingFormat, RecordingProfile, RecordingTarget};
use crate::recording::{CaptureSpec, RecordingEvent};
use crate::{CAPTURE_TIMEOUT, CaptureArgs, output_destination};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum HeadlessFormat {
    #[default]
    Mp4,
    Hls,
}

impl From<HeadlessFormat> for RecordingFormat {
    fn from(format: HeadlessFormat) -> Self {
        match format {
            HeadlessFormat::Mp4 => Self::Mp4,
            HeadlessFormat::Hls => Self::Hls,
        }
    }
}

pub(crate) fn run(args: &CaptureArgs) -> Result<String, String> {
    let server_url = args
        .server_url
        .clone()
        .or_else(|| std::env::var("BLIP_SERVER_URL").ok())
        .ok_or_else(|| {
            "--server-url or the BLIP_SERVER_URL environment variable is required".to_owned()
        })?;
    let format = RecordingFormat::from(args.format);
    let profile = RecordingProfile {
        id: "headless".into(),
        name: "Headless test".into(),
        target: RecordingTarget::Remote {
            server_url: server_url.clone(),
        },
        format,
        completion_action: CompletionAction::None,
    };
    profile.validate()?;
    if !blip_sck::has_permission() {
        return Err(
            "Screen Recording permission is required; grant it in System Settings and retry".into(),
        );
    }
    let content = ShareableContent::current(CAPTURE_TIMEOUT).map_err(|error| error.to_string())?;
    let display = match args.display {
        Some(display_id) => content
            .displays()
            .into_iter()
            .find(|display| display.id() == display_id)
            .ok_or_else(|| format!("display {display_id} is not available"))?,
        None => content
            .main_display()
            .ok_or_else(|| "the main display is not available".to_owned())?,
    };
    let output = output_destination(&profile)?;
    eprintln!(
        "blip-capture: recording display {} for {} seconds to {}",
        display.id(),
        args.duration,
        output.media_path.display()
    );

    let (event_sender, event_receiver) = async_channel::unbounded();
    let stop_sender = crate::recording::spawn(
        CaptureSpec::Display(display.id()),
        output.media_path,
        output.completed_path,
        output.cleanup_on_failure,
        Some(server_url),
        format,
        event_sender,
    )?;
    let mut started = false;
    while let Ok(event) = event_receiver.recv_blocking() {
        match event {
            RecordingEvent::Started if !started => {
                started = true;
                let timer_stop_sender = stop_sender.clone();
                let duration = Duration::from_secs(args.duration);
                if let Err(error) = thread::Builder::new()
                    .name("blip-capture-headless-timer".into())
                    .spawn(move || {
                        thread::sleep(duration);
                        let _ = timer_stop_sender.send(());
                    })
                {
                    let _ = stop_sender.send(());
                    return Err(format!("failed to start recording timer: {error}"));
                }
            }
            RecordingEvent::Started => {}
            RecordingEvent::Finished {
                viewer_url: Some(viewer_url),
                ..
            } => return Ok(viewer_url),
            RecordingEvent::Finished {
                viewer_url: None, ..
            } => return Err("headless recording finished without a viewer URL".into()),
            RecordingEvent::Failed(error) => {
                let _ = stop_sender.send(());
                return Err(error);
            }
        }
    }
    Err("recording stopped without a result".into())
}

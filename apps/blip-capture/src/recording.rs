use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Sender;
use blip_avfoundation::{Mp4Writer, WriterError};
use blip_sck::{CaptureFilter, Capturer, PixelFormat, ShareableContent, StreamConfig, VideoFrame};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_QUEUE_DEPTH: usize = 8;

#[derive(Clone, Copy)]
pub(crate) enum CaptureSpec {
    Display(u32),
    Window(u32),
    Region {
        display_id: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
}

pub(crate) enum RecordingEvent {
    Started,
    Finished(PathBuf),
    Failed(String),
}

enum WriterMessage {
    Frame(VideoFrame, Duration),
    Finish,
}

pub(crate) fn spawn(
    spec: CaptureSpec,
    output: PathBuf,
    events: Sender<RecordingEvent>,
) -> Result<mpsc::Sender<()>, String> {
    let (stop_sender, stop_receiver) = mpsc::channel();
    thread::Builder::new()
        .name("blip-capture-recording".into())
        .spawn(move || {
            if let Err(message) = record(spec, &output, &stop_receiver, &events) {
                let _ = events.send_blocking(RecordingEvent::Failed(message));
            } else {
                let _ = events.send_blocking(RecordingEvent::Finished(output));
            }
        })
        .map_err(|error| format!("failed to spawn recording thread: {error}"))?;
    Ok(stop_sender)
}

fn record(
    spec: CaptureSpec,
    output: &Path,
    stop_receiver: &mpsc::Receiver<()>,
    events: &Sender<RecordingEvent>,
) -> Result<(), String> {
    let content = ShareableContent::current(CAPTURE_TIMEOUT).map_err(|error| error.to_string())?;
    let (filter, source_rect) = capture_filter(&content, spec)?;
    let mut config = StreamConfig::builder()
        .with_fps(60)
        .with_cursor(true)
        .with_queue_depth(8)
        .with_pixel_format(PixelFormat::Bgra);
    if let Some((x, y, width, height)) = source_rect {
        config = config.with_source_rect(x, y, width, height);
    }

    let (writer_sender, writer_receiver) = mpsc::sync_channel(FRAME_QUEUE_DEPTH);
    let writer_output = output.to_owned();
    let writer = thread::Builder::new()
        .name("blip-capture-writer".into())
        .spawn(move || write_frames(&writer_output, &writer_receiver))
        .map_err(|error| error.to_string())?;
    let frame_sender = writer_sender.clone();
    let capture_events = events.clone();
    let recording_start = Instant::now();
    let capturer = Capturer::builder(filter, config)
        .map_err(|error| error.to_string())?
        .with_timeout(CAPTURE_TIMEOUT)
        .with_video_frame_callback(move |frame| {
            let _ = frame_sender.try_send(WriterMessage::Frame(frame, recording_start.elapsed()));
        })
        .with_stop_callback(move |error| {
            let _ = capture_events.try_send(RecordingEvent::Failed(
                error.localizedDescription().to_string(),
            ));
        })
        .build()
        .map_err(|error| error.to_string())?;

    capturer.start().map_err(|error| error.to_string())?;
    let _ = events.send_blocking(RecordingEvent::Started);
    stop_receiver.recv().map_err(|error| error.to_string())?;
    capturer.stop().map_err(|error| error.to_string())?;
    writer_sender
        .send(WriterMessage::Finish)
        .map_err(|error| error.to_string())?;
    writer
        .join()
        .map_err(|_| "video writer terminated unexpectedly".to_owned())?
        .map_err(|error| error.to_string())
}

type SourceRect = Option<(f64, f64, f64, f64)>;

fn capture_filter(
    content: &ShareableContent,
    spec: CaptureSpec,
) -> Result<(CaptureFilter, SourceRect), String> {
    if let CaptureSpec::Window(window_id) = spec {
        let window = content
            .application_windows()
            .into_iter()
            .find(|window| window.id() == window_id)
            .ok_or_else(|| "the selected window is no longer available".to_owned())?;
        return Ok((CaptureFilter::from(window), None));
    }

    let (display_id, source_rect) = match spec {
        CaptureSpec::Display(display_id) => (display_id, None),
        CaptureSpec::Region {
            display_id,
            x,
            y,
            width,
            height,
        } => (display_id, Some((x, y, width, height))),
        CaptureSpec::Window(_) => return Err("invalid display capture target".to_owned()),
    };
    let display = content
        .displays()
        .into_iter()
        .find(|display| display.id() == display_id)
        .ok_or_else(|| "the selected display is no longer available".to_owned())?;
    let process_id = i32::try_from(std::process::id())
        .map_err(|_| "process ID exceeds ScreenCaptureKit's range".to_owned())?;
    let own_windows = content.windows().into_iter().filter(|window| {
        window
            .application()
            .is_some_and(|application| application.process_id() == process_id)
    });
    Ok((
        CaptureFilter::display(display)
            .excluding_windows(own_windows)
            .build(),
        source_rect,
    ))
}

fn write_frames(
    output: &Path,
    receiver: &mpsc::Receiver<WriterMessage>,
) -> Result<(), WriterError> {
    let mut writer = None;
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Frame(frame, timestamp) => {
                let writer = match &mut writer {
                    Some(writer) => writer,
                    None => {
                        writer.insert(Mp4Writer::new(output, frame.width(), frame.height(), 60)?)
                    }
                };
                let _ = writer.append(frame.image_buffer(), timestamp)?;
            }
            WriterMessage::Finish => break,
        }
    }
    let mut writer = writer.ok_or(WriterError::NoFrames)?;
    writer.finish()
}

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Sender;
use blip_avfoundation::{HlsWriter, Mp4Writer, WriterError};
use blip_sck::{CaptureFilter, Capturer, PixelFormat, ShareableContent, StreamConfig, VideoFrame};

use crate::profiles::RecordingFormat;

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
    Finished {
        path: PathBuf,
        viewer_url: Option<String>,
    },
    Failed(String),
}

enum WriterMessage {
    Frame(VideoFrame, Duration),
    Finish,
}

pub(crate) fn spawn(
    spec: CaptureSpec,
    output: PathBuf,
    completed_path: PathBuf,
    cleanup_on_failure: Option<PathBuf>,
    server_url: Option<String>,
    format: RecordingFormat,
    events: Sender<RecordingEvent>,
) -> Result<mpsc::Sender<()>, String> {
    let (stop_sender, stop_receiver) = mpsc::channel();
    let spawn_cleanup = cleanup_on_failure.clone();
    let spawn_result = thread::Builder::new()
        .name("blip-capture-recording".into())
        .spawn(move || {
            tracing::info!(
                output = %output.display(),
                format = ?format,
                "Starting recording thread"
            );
            if let Err(message) = record(spec, &output, format, &stop_receiver, &events) {
                tracing::error!(error = %message, "Recording thread failed");
                if let Some(path) = cleanup_on_failure {
                    std::fs::remove_dir_all(path).ok();
                }
                let _ = events.send_blocking(RecordingEvent::Failed(message));
            } else {
                let viewer_url = match server_url {
                    Some(server_url) => {
                        tracing::info!(
                            server_url = %server_url,
                            "Starting upload from recording thread"
                        );
                        match crate::upload::upload(&output, &server_url, format) {
                            Ok(viewer_url) => {
                                tracing::info!(
                                    viewer_url = %viewer_url,
                                    "Upload from recording thread finished"
                                );
                                if format == RecordingFormat::Hls {
                                    std::fs::remove_dir_all(&output).ok();
                                } else {
                                    std::fs::remove_file(&output).ok();
                                }
                                Some(viewer_url)
                            }
                            Err(error) => {
                                tracing::error!(
                                    error = %error,
                                    "Upload from recording thread failed"
                                );
                                let _ = events.send_blocking(RecordingEvent::Failed(format!(
                                    "{error}. The recording was kept at {}",
                                    output.display()
                                )));
                                return;
                            }
                        }
                    }
                    None => None,
                };
                tracing::info!(
                    path = %completed_path.display(),
                    viewer_url = ?viewer_url,
                    "Recording thread finished successfully"
                );
                let _ = events.send_blocking(RecordingEvent::Finished {
                    path: completed_path,
                    viewer_url,
                });
            }
        });
    if let Err(error) = spawn_result {
        if let Some(path) = spawn_cleanup {
            std::fs::remove_dir_all(path).ok();
        }
        return Err(format!("failed to spawn recording thread: {error}"));
    }
    Ok(stop_sender)
}

fn record(
    spec: CaptureSpec,
    output: &Path,
    format: RecordingFormat,
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
        .spawn(move || write_frames(&writer_output, format, &writer_receiver))
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
    tracing::info!("Screen capture started");
    let _ = events.send_blocking(RecordingEvent::Started);
    stop_receiver.recv().map_err(|error| error.to_string())?;
    tracing::info!("Screen capture stop requested, stopping capturer");
    capturer.stop().map_err(|error| error.to_string())?;
    tracing::info!("Screen capture stopped, sending finish to video writer");
    writer_sender
        .send(WriterMessage::Finish)
        .map_err(|error| error.to_string())?;
    writer
        .join()
        .map_err(|_| "video writer terminated unexpectedly".to_owned())?
        .map_err(|error| error.to_string())?;
    tracing::info!("Video writer thread finished");
    Ok(())
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
    format: RecordingFormat,
    receiver: &mpsc::Receiver<WriterMessage>,
) -> Result<(), WriterError> {
    enum Writer {
        Mp4(Mp4Writer),
        Hls(HlsWriter),
    }

    let mut writer: Option<Writer> = None;
    let mut frame_count = 0_usize;
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Frame(frame, timestamp) => {
                let is_first = writer.is_none();
                let writer = match &mut writer {
                    Some(writer) => writer,
                    None => writer.insert(if format == RecordingFormat::Hls {
                        Writer::Hls(HlsWriter::new(
                            output,
                            frame.width(),
                            frame.height(),
                            60,
                            Duration::from_secs(2),
                        )?)
                    } else {
                        Writer::Mp4(Mp4Writer::new(output, frame.width(), frame.height(), 60)?)
                    }),
                };
                if is_first {
                    tracing::info!(
                        width = frame.width(),
                        height = frame.height(),
                        format = ?format,
                        "First frame received, video writer initialized"
                    );
                }
                match writer {
                    Writer::Mp4(writer) => {
                        let _ = writer.append(frame.image_buffer(), timestamp)?;
                    }
                    Writer::Hls(writer) => {
                        let _ = writer.append(frame.image_buffer(), timestamp)?;
                    }
                }
                frame_count = frame_count.saturating_add(1);
                if frame_count.is_multiple_of(60) {
                    tracing::info!(
                        frame_count,
                        elapsed_secs = timestamp.as_secs_f32(),
                        "Recording progress"
                    );
                } else {
                    tracing::trace!(
                        frame_count,
                        elapsed_ms = timestamp.as_millis(),
                        "Recorded frame"
                    );
                }
            }
            WriterMessage::Finish => {
                tracing::info!(frame_count, "Finish requested, closing video writer");
                break;
            }
        }
    }
    let mut writer = writer.ok_or(WriterError::NoFrames)?;
    match &mut writer {
        Writer::Mp4(writer) => writer.finish()?,
        Writer::Hls(writer) => writer.finish()?,
    }
    tracing::info!(
        total_frames = frame_count,
        "Video writer finalized successfully"
    );
    Ok(())
}

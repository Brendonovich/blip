use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Sender;
use blip_avfoundation::{HlsWriter, Mp4Writer, WriterError};
use blip_sck::{
    CaptureColorSpace, CaptureFilter, Capturer, PixelFormat, ShareableContent, StreamConfig,
    VideoFrame,
};

use crate::profiles::RecordingFormat;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_QUEUE_DEPTH: usize = 8;
const MAX_CONCURRENT_HLS_UPLOADS: usize = 3;
const MIN_MP4_BITRATE: f64 = 2_500_000.0;
const MAX_MP4_BITRATE: f64 = 8_000_000.0;

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
    Uploading,
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

enum HlsUploadMessage {
    Asset(PathBuf),
    Complete(PathBuf),
    Abort,
}

type HlsUploadSender = tokio::sync::mpsc::UnboundedSender<HlsUploadMessage>;

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
            let incremental_upload = if format == RecordingFormat::Hls {
                server_url
                    .as_deref()
                    .map(|server_url| spawn_hls_upload(&output, server_url))
                    .transpose()
            } else {
                Ok(None)
            };
            let incremental_upload = match incremental_upload {
                Ok(upload) => upload,
                Err(message) => {
                    if let Some(path) = cleanup_on_failure {
                        std::fs::remove_dir_all(path).ok();
                    }
                    let _ = events.send_blocking(RecordingEvent::Failed(message));
                    return;
                }
            };
            let upload_assets = incremental_upload
                .as_ref()
                .map(|(sender, _)| sender.clone());
            let recording = record(
                spec,
                &output,
                format,
                &stop_receiver,
                &events,
                upload_assets,
            );
            if recording.is_ok() && server_url.is_some() {
                let _ = events.send_blocking(RecordingEvent::Uploading);
            }
            if let Some((sender, _)) = &incremental_upload {
                let message = if recording.is_ok() {
                    HlsUploadMessage::Complete(output.join("playlist.m3u8"))
                } else {
                    HlsUploadMessage::Abort
                };
                let _ = sender.send(message);
            }
            let incremental_result = incremental_upload.map(|(sender, worker)| {
                drop(sender);
                worker
                    .join()
                    .map_err(|_| "HLS upload thread terminated unexpectedly".to_owned())?
            });

            if let Err(message) = recording {
                tracing::error!(error = %message, "Recording thread failed");
                if let Some(path) = cleanup_on_failure {
                    std::fs::remove_dir_all(path).ok();
                }
                let _ = events.send_blocking(RecordingEvent::Failed(message));
            } else {
                let viewer_url = match incremental_result {
                    Some(Ok(viewer_url)) => {
                        std::fs::remove_dir_all(&output).ok();
                        viewer_url
                    }
                    Some(Err(error)) => {
                        tracing::error!(error = %error, "Incremental upload failed");
                        let _ = events.send_blocking(RecordingEvent::Failed(format!(
                            "{error}. The recording was kept at {}",
                            output.display()
                        )));
                        return;
                    }
                    None => match server_url {
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
                    },
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
    upload_assets: Option<HlsUploadSender>,
) -> Result<(), String> {
    let content = ShareableContent::current(CAPTURE_TIMEOUT).map_err(|error| error.to_string())?;
    let (filter, source_rect) = capture_filter(&content, spec)?;
    let mut config = StreamConfig::builder()
        .with_fps(60)
        .with_cursor(true)
        .with_queue_depth(8)
        .with_pixel_format(PixelFormat::Bgra)
        .with_color_space(CaptureColorSpace::Srgb);
    if let Some((x, y, width, height)) = source_rect {
        config = config.with_source_rect(x, y, width, height);
    }

    let (writer_sender, writer_receiver) = mpsc::sync_channel(FRAME_QUEUE_DEPTH);
    let writer_output = output.to_owned();
    let writer = thread::Builder::new()
        .name("blip-capture-writer".into())
        .spawn(move || write_frames(&writer_output, format, &writer_receiver, upload_assets))
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
    let (display_id, source_rect) = match spec {
        CaptureSpec::Display(display_id) => (display_id, None),
        CaptureSpec::Window(window_id) => {
            let window = content
                .application_windows()
                .into_iter()
                .find(|window| window.id() == window_id)
                .ok_or_else(|| "the selected window is no longer available".to_owned())?;
            let display = window
                .display()
                .ok_or_else(|| "the selected window is not on an available display".to_owned())?;
            let source_rect = display_relative_intersection(window.frame(), display.frame())
                .ok_or_else(|| "the selected window has no visible capture area".to_owned())?;
            (display.id(), Some(source_rect))
        }
        CaptureSpec::Region {
            display_id,
            x,
            y,
            width,
            height,
        } => (display_id, Some((x, y, width, height))),
    };
    let display = content
        .displays()
        .into_iter()
        .find(|display| display.id() == display_id)
        .ok_or_else(|| "the selected display is no longer available".to_owned())?;
    let process_id = i32::try_from(std::process::id())
        .map_err(|_| "process ID exceeds ScreenCaptureKit's range".to_owned())?;
    let capture_ui_windows = content.windows().into_iter().filter(|window| {
        window
            .application()
            .is_some_and(|application| application.process_id() == process_id)
            && window.layer() != 0
    });
    Ok((
        CaptureFilter::display(display)
            .excluding_windows(capture_ui_windows)
            .build(),
        source_rect,
    ))
}

fn display_relative_intersection(
    window: (f64, f64, f64, f64),
    display: (f64, f64, f64, f64),
) -> Option<(f64, f64, f64, f64)> {
    let left = window.0.max(display.0);
    let top = window.1.max(display.1);
    let right = (window.0 + window.2).min(display.0 + display.2);
    let bottom = (window.1 + window.3).min(display.1 + display.3);
    let width = right - left;
    let height = bottom - top;
    (width > 0.0 && height > 0.0).then_some((left - display.0, top - display.1, width, height))
}

fn write_frames(
    output: &Path,
    format: RecordingFormat,
    receiver: &mpsc::Receiver<WriterMessage>,
    upload_assets: Option<HlsUploadSender>,
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
                        Writer::Hls(if let Some(upload_assets) = upload_assets.clone() {
                            HlsWriter::new_with_asset_callback(
                                output,
                                frame.width(),
                                frame.height(),
                                60,
                                Duration::from_secs(2),
                                move |path| {
                                    let _ = upload_assets.send(HlsUploadMessage::Asset(path));
                                },
                            )?
                        } else {
                            HlsWriter::new(
                                output,
                                frame.width(),
                                frame.height(),
                                60,
                                Duration::from_secs(2),
                            )?
                        })
                    } else {
                        let writer = if format == RecordingFormat::Mp4 {
                            Mp4Writer::new_with_bitrate_preserving_color(
                                output,
                                frame.width(),
                                frame.height(),
                                60,
                                mp4_bitrate(frame.width(), frame.height(), 60),
                                frame.image_buffer(),
                            )?
                        } else {
                            Mp4Writer::new_preserving_color(
                                output,
                                frame.width(),
                                frame.height(),
                                60,
                                frame.image_buffer(),
                            )?
                        };
                        Writer::Mp4(writer)
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

fn mp4_bitrate(width: usize, height: usize, fps: u32) -> usize {
    let pixel_ratio = width as f64 * height as f64 / (1920.0 * 1080.0);
    let fps_ratio = f64::from(fps.min(60)) / 30.0;
    (1_500_000.0 + pixel_ratio * 1_500_000.0 + fps_ratio * 500_000.0)
        .clamp(MIN_MP4_BITRATE, MAX_MP4_BITRATE)
        .round() as usize
}

type HlsUploadWorker = thread::JoinHandle<Result<Option<String>, String>>;

fn spawn_hls_upload(
    output: &Path,
    server_url: &str,
) -> Result<(HlsUploadSender, HlsUploadWorker), String> {
    let output = output.to_owned();
    let server_url = server_url.to_owned();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let worker = thread::Builder::new()
        .name("blip-capture-hls-upload".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("failed to start HLS upload runtime: {error}"))?;
            runtime.block_on(run_hls_upload(output, server_url, receiver))
        })
        .map_err(|error| format!("failed to spawn HLS upload thread: {error}"))?;
    Ok((sender, worker))
}

async fn run_hls_upload(
    output: PathBuf,
    server_url: String,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<HlsUploadMessage>,
) -> Result<Option<String>, String> {
    let mut upload = crate::upload::HlsUpload::start(&output, &server_url).await?;
    let mut uploads = tokio::task::JoinSet::new();
    let result = async {
        while let Some(message) = receiver.recv().await {
            match message {
                HlsUploadMessage::Asset(path) => {
                    if uploads.len() == MAX_CONCURRENT_HLS_UPLOADS {
                        receive_hls_upload_result(&mut uploads).await?;
                    }
                    upload.register_asset(&path)?;
                    let asset_upload = upload.clone();
                    uploads.spawn(async move { asset_upload.upload_asset(&path).await });
                }
                HlsUploadMessage::Complete(playlist) => {
                    while !uploads.is_empty() {
                        receive_hls_upload_result(&mut uploads).await?;
                    }
                    return upload.finish(&playlist).await.map(Some);
                }
                HlsUploadMessage::Abort => return Ok(None),
            }
        }
        Err("HLS upload stopped before the recording completed".into())
    }
    .await;
    if !matches!(result, Ok(Some(_))) {
        uploads.shutdown().await;
        upload.abort().await;
    }
    result
}

async fn receive_hls_upload_result(
    uploads: &mut tokio::task::JoinSet<Result<(), String>>,
) -> Result<(), String> {
    uploads
        .join_next()
        .await
        .ok_or_else(|| "HLS asset upload task terminated unexpectedly".to_owned())?
        .map_err(|error| format!("HLS asset upload task terminated unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::{MAX_MP4_BITRATE, MIN_MP4_BITRATE, display_relative_intersection, mp4_bitrate};

    #[test]
    fn scales_mp4_bitrate_with_resolution_and_frame_rate() {
        let hd_60 = mp4_bitrate(1920, 1080, 60);

        assert_eq!(hd_60, 4_000_000);
        assert!(mp4_bitrate(1280, 720, 30) < hd_60);
        assert!(mp4_bitrate(1920, 1080, 30) < hd_60);
        assert!(mp4_bitrate(3840, 2160, 60) > hd_60);
    }

    #[test]
    fn bounds_mp4_bitrate() {
        assert_eq!(mp4_bitrate(320, 240, 1), MIN_MP4_BITRATE as usize);
        assert_eq!(mp4_bitrate(7680, 4320, 120), MAX_MP4_BITRATE as usize);
    }

    #[test]
    fn translates_window_bounds_to_display_coordinates() {
        assert_eq!(
            display_relative_intersection(
                (2100.0, 140.0, 800.0, 600.0),
                (1920.0, 0.0, 1920.0, 1080.0)
            ),
            Some((180.0, 140.0, 800.0, 600.0))
        );
    }

    #[test]
    fn clips_window_bounds_to_the_display() {
        assert_eq!(
            display_relative_intersection(
                (-100.0, 900.0, 500.0, 300.0),
                (0.0, 0.0, 1920.0, 1080.0)
            ),
            Some((0.0, 900.0, 400.0, 180.0))
        );
    }
}

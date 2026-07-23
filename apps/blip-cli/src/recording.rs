use std::error::Error;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use blip_avfoundation::{Mp4Writer, WriterError};
use blip_sck::{CaptureError, Capturer, PixelFormat, StreamConfig, VideoFrame};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::cli::RecordArgs;
use crate::commands::{CAPTURE_TIMEOUT, select_target, shareable_content};

const FRAME_QUEUE_DEPTH: usize = 8;

enum RecordEnd {
    Interrupt,
    CaptureError(String),
    WriterError,
}

enum WriterMessage {
    Frame(VideoFrame, Duration),
    Finish,
}

struct WriterStats {
    appended: u64,
    backpressure_drops: u64,
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub(crate) fn record(args: &RecordArgs) -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    let (filter, description) = select_target(&content, &args.capture)?;
    let config = StreamConfig::builder()
        .with_fps(args.capture.fps)
        .with_cursor(args.capture.cursor)
        .with_queue_depth(8)
        .with_pixel_format(PixelFormat::Bgra);

    let (end_sender, end_receiver) = mpsc::sync_channel(1);
    let interrupt_sender = end_sender.clone();
    ctrlc::set_handler(move || {
        let _ = interrupt_sender.try_send(RecordEnd::Interrupt);
    })?;
    let raw_mode = listen_for_stop_key(end_sender.clone())?;

    let (writer_sender, writer_receiver) = mpsc::sync_channel(FRAME_QUEUE_DEPTH);
    let worker_error_sender = end_sender.clone();
    let output = args.output.clone();
    let fps = args.capture.fps;
    let worker = thread::Builder::new()
        .name("blip-avfoundation-writer".into())
        .spawn(move || {
            let result = write_frames(&output, fps, &writer_receiver);
            if result.is_err() {
                let _ = worker_error_sender.try_send(RecordEnd::WriterError);
            }
            result
        })?;

    let queue_drops = Arc::new(AtomicU64::new(0));
    let callback_drops = Arc::clone(&queue_drops);
    let frame_sender = writer_sender.clone();
    let capture_error_sender = end_sender;
    let recording_start = Instant::now();
    let capturer = Capturer::builder(filter, config)?
        .with_timeout(CAPTURE_TIMEOUT)
        .with_video_frame_callback(move |frame| {
            match frame_sender.try_send(WriterMessage::Frame(frame, recording_start.elapsed())) {
                Err(mpsc::TrySendError::Full(_)) => {
                    callback_drops.fetch_add(1, Ordering::Relaxed);
                }
                Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
            }
        })
        .with_stop_callback(move |error| {
            let _ = capture_error_sender.try_send(RecordEnd::CaptureError(
                error.localizedDescription().to_string(),
            ));
        })
        .build()?;

    capturer.start()?;
    let stop_hint = if raw_mode.is_some() {
        "press Space to stop"
    } else {
        "press Ctrl-C to stop"
    };
    eprintln!(
        "recording {description} to {}; {stop_hint}",
        args.output.display()
    );
    let end = end_receiver.recv()?;
    eprintln!("stopping recording...");
    let stop_result = match &end {
        RecordEnd::CaptureError(_) => Ok(()),
        RecordEnd::Interrupt | RecordEnd::WriterError => capturer.stop(),
    };
    let _ = writer_sender.send(WriterMessage::Finish);
    let writer_result = worker
        .join()
        .map_err(|_| "AVFoundation writer thread terminated unexpectedly")?;

    stop_result?;
    if let RecordEnd::CaptureError(message) = end {
        return Err(CaptureError::Framework(message).into());
    }

    let stats = writer_result?;
    let queue_drops = queue_drops.load(Ordering::Relaxed);
    eprintln!(
        "wrote {} frames to {} ({} dropped for backpressure, {} dropped by queue)",
        stats.appended,
        args.output.display(),
        stats.backpressure_drops,
        queue_drops
    );
    Ok(())
}

fn listen_for_stop_key(
    sender: mpsc::SyncSender<RecordEnd>,
) -> Result<Option<RawModeGuard>, Box<dyn Error>> {
    if !io::stdin().is_terminal() {
        return Ok(None);
    }

    enable_raw_mode()?;
    let guard = RawModeGuard;
    if let Err(error) = thread::Builder::new()
        .name("blip-record-input".into())
        .spawn(move || {
            while let Ok(event) = event::read() {
                if let Event::Key(key) = event
                    && is_stop_key(key)
                {
                    let _ = sender.try_send(RecordEnd::Interrupt);
                    break;
                }
            }
        })
    {
        return Err(error.into());
    }
    Ok(Some(guard))
}

fn is_stop_key(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char(' ')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)))
}

fn write_frames(
    output: &Path,
    fps: u32,
    receiver: &mpsc::Receiver<WriterMessage>,
) -> Result<WriterStats, WriterError> {
    let mut writer = None;
    let mut appended = 0_u64;
    let mut backpressure_drops = 0_u64;

    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Frame(frame, timestamp) => {
                let writer = match &mut writer {
                    Some(writer) => writer,
                    None => {
                        writer.insert(Mp4Writer::new(output, frame.width(), frame.height(), fps)?)
                    }
                };
                if writer.append(frame.image_buffer(), timestamp)? {
                    appended = appended.saturating_add(1);
                } else {
                    backpressure_drops = backpressure_drops.saturating_add(1);
                }
            }
            WriterMessage::Finish => break,
        }
    }

    let mut writer = writer.ok_or(WriterError::NoFrames)?;
    writer.finish()?;
    Ok(WriterStats {
        appended,
        backpressure_drops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_on_space_press() {
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(is_stop_key(key), "Space should stop recording");
    }
}

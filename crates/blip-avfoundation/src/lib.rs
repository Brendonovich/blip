#![cfg(target_os = "macos")]

mod camera;

pub use camera::{
    CameraAuthorizationStatus, CameraCaptureFormat, CameraCapturer, CameraDevice, CameraError,
    CameraFrame, camera_authorization_status, list_video_devices, request_camera_access,
};

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoHeightKey,
    AVVideoWidthKey,
};
use objc2_core_media::CMTime;
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

const TIMESCALE: i32 = 1_000_000;
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("video dimensions must be non-zero")]
    InvalidDimensions,
    #[error("frame rate must be greater than zero")]
    InvalidFrameRate,
    #[error("output path is not valid UTF-8")]
    InvalidOutputPath,
    #[error("failed to prepare output: {0}")]
    Output(#[from] std::io::Error),
    #[error("AVFoundation constant {0} is unavailable")]
    MissingConstant(&'static str),
    #[error("AVFoundation rejected the video input")]
    UnsupportedInput,
    #[error("failed to create asset writer: {0}")]
    Create(String),
    #[error("failed to start asset writer: {0}")]
    Start(String),
    #[error("failed to append video frame: {0}")]
    Append(String),
    #[error("recording contains no video frames")]
    NoFrames,
    #[error("video timestamp exceeds AVFoundation's range")]
    TimestampOverflow,
    #[error("timed out while finishing the MP4 file")]
    FinishTimeout,
    #[error("failed to finish MP4 file: {0}")]
    Finish(String),
}

pub struct Mp4Writer {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    frame_duration: Duration,
    start_timestamp_micros: Option<i64>,
    last_relative_timestamp_micros: Option<i64>,
    started: bool,
    finished: bool,
}

impl Mp4Writer {
    /// Creates an H.264 MP4 writer for BGRA pixel buffers.
    ///
    /// # Errors
    ///
    /// Returns an error if the output cannot be prepared or `AVFoundation`
    /// rejects the writer configuration.
    pub fn new(output: &Path, width: usize, height: usize, fps: u32) -> Result<Self, WriterError> {
        if width == 0 || height == 0 {
            return Err(WriterError::InvalidDimensions);
        }
        let frame_duration = Duration::from_secs(1)
            .checked_div(fps)
            .ok_or(WriterError::InvalidFrameRate)?;

        if let Some(parent) = output.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        if output.exists() {
            fs::remove_file(output)?;
        }

        let path = output.to_str().ok_or(WriterError::InvalidOutputPath)?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        // SAFETY: These framework constants exist on every macOS version supported by AVAssetWriter.
        let file_type =
            unsafe { AVFileTypeMPEG4 }.ok_or(WriterError::MissingConstant("AVFileTypeMPEG4"))?;
        // SAFETY: The URL is a local file URL and the static file type is provided by AVFoundation.
        let writer =
            unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type) }
                .map_err(|error| WriterError::Create(error.localizedDescription().to_string()))?;

        // SAFETY: These video setting constants are available with AVFoundation's H.264 encoder.
        let (media_type, codec_key, codec, width_key, height_key) = unsafe {
            (
                AVMediaTypeVideo,
                AVVideoCodecKey,
                AVVideoCodecTypeH264,
                AVVideoWidthKey,
                AVVideoHeightKey,
            )
        };
        let media_type = media_type.ok_or(WriterError::MissingConstant("AVMediaTypeVideo"))?;
        let codec_key = codec_key.ok_or(WriterError::MissingConstant("AVVideoCodecKey"))?;
        let codec = codec.ok_or(WriterError::MissingConstant("AVVideoCodecTypeH264"))?;
        let width_key = width_key.ok_or(WriterError::MissingConstant("AVVideoWidthKey"))?;
        let height_key = height_key.ok_or(WriterError::MissingConstant("AVVideoHeightKey"))?;
        let width = NSNumber::new_usize(width);
        let height = NSNumber::new_usize(height);
        let keys = [codec_key, width_key, height_key];
        let values: [&AnyObject; 3] = [codec, &width, &height];
        let settings = NSDictionary::from_slices(&keys, &values);

        // SAFETY: The dictionary contains the required H.264 codec, width, and height settings.
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        // SAFETY: This property must be configured before writing starts.
        unsafe { input.setExpectsMediaDataInRealTime(true) };
        // SAFETY: The input is unattached and configured for video media.
        if !unsafe { writer.canAddInput(&input) } {
            return Err(WriterError::UnsupportedInput);
        }
        // SAFETY: `canAddInput` succeeded and writing has not started.
        unsafe { writer.addInput(&input) };
        // SAFETY: Existing captured buffers are supplied directly, so no allocation pool is needed.
        let adaptor = unsafe {
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &input,
                None,
            )
        };
        // SAFETY: All inputs and configuration have been added.
        if !unsafe { writer.startWriting() } {
            return Err(WriterError::Start(writer_error(&writer)));
        }

        Ok(Self {
            writer,
            input,
            adaptor,
            frame_duration,
            start_timestamp_micros: None,
            last_relative_timestamp_micros: None,
            started: false,
            finished: false,
        })
    }

    /// Appends a pixel buffer at the supplied source timestamp.
    ///
    /// Returns `Ok(false)` when `AVFoundation` is applying backpressure and the
    /// real-time frame should be dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the timestamp overflows or `AVFoundation` fails.
    pub fn append(
        &mut self,
        pixel_buffer: &CVPixelBuffer,
        timestamp: Duration,
    ) -> Result<bool, WriterError> {
        // SAFETY: The input is retained and attached to the active writer.
        if !unsafe { self.input.isReadyForMoreMediaData() } {
            return Ok(false);
        }

        let timestamp_micros =
            i64::try_from(timestamp.as_micros()).map_err(|_| WriterError::TimestampOverflow)?;
        let start_timestamp = *self.start_timestamp_micros.get_or_insert(timestamp_micros);
        let mut relative_timestamp = timestamp_micros
            .checked_sub(start_timestamp)
            .ok_or(WriterError::TimestampOverflow)?
            .max(0);
        if let Some(last_timestamp) = self.last_relative_timestamp_micros
            && relative_timestamp <= last_timestamp
        {
            relative_timestamp = last_timestamp
                .checked_add(1)
                .ok_or(WriterError::TimestampOverflow)?;
        }
        let source_timestamp = start_timestamp
            .checked_add(relative_timestamp)
            .ok_or(WriterError::TimestampOverflow)?;
        let presentation_time = cm_time(source_timestamp);

        if !self.started {
            // SAFETY: Writing has started, and no samples have been appended yet.
            unsafe {
                self.writer
                    .startSessionAtSourceTime(cm_time(start_timestamp));
            }
            self.started = true;
        }

        // SAFETY: The input is ready, the session is active, and the timestamp is numeric.
        if !unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(pixel_buffer, presentation_time)
        } {
            return Err(WriterError::Append(writer_error(&self.writer)));
        }
        self.last_relative_timestamp_micros = Some(relative_timestamp);
        Ok(true)
    }

    /// Finalizes the video track and MP4 container.
    ///
    /// # Errors
    ///
    /// Returns an error when no frames were written or `AVFoundation` cannot
    /// complete the output file.
    pub fn finish(&mut self) -> Result<(), WriterError> {
        if self.finished {
            return Ok(());
        }
        let start_timestamp = self.start_timestamp_micros.ok_or(WriterError::NoFrames)?;
        let last_relative_timestamp = self
            .last_relative_timestamp_micros
            .ok_or(WriterError::NoFrames)?;
        let frame_duration = i64::try_from(self.frame_duration.as_micros())
            .map_err(|_| WriterError::TimestampOverflow)?;
        let end_timestamp = start_timestamp
            .checked_add(last_relative_timestamp)
            .ok_or(WriterError::TimestampOverflow)?
            .checked_add(frame_duration.max(1))
            .ok_or(WriterError::TimestampOverflow)?;

        // SAFETY: The active session contains all frames and this is its final end time.
        unsafe {
            self.writer.endSessionAtSourceTime(cm_time(end_timestamp));
            self.input.markAsFinished();
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move || {
            let _ = sender.try_send(());
        });
        // SAFETY: All append calls are complete, and the escaping block owns its sender.
        unsafe {
            self.writer.finishWritingWithCompletionHandler(&completion);
        }
        receiver
            .recv_timeout(FINISH_TIMEOUT)
            .map_err(|_| WriterError::FinishTimeout)?;

        // SAFETY: The completion handler has fired, so status and error are stable.
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
            return Err(WriterError::Finish(writer_error(&self.writer)));
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for Mp4Writer {
    fn drop(&mut self) {
        if !self.finished {
            // SAFETY: Cancellation is thread-safe and cleans up an unfinished output file.
            unsafe { self.writer.cancelWriting() };
        }
    }
}

fn writer_error(writer: &AVAssetWriter) -> String {
    // SAFETY: AVAssetWriter documents status and error as thread-safe properties.
    unsafe { writer.error() }.map_or_else(
        || "unknown AVFoundation error".into(),
        |error| error.localizedDescription().to_string(),
    )
}

fn cm_time(value: i64) -> CMTime {
    // SAFETY: The fixed timescale is positive and within CoreMedia's supported range.
    unsafe { CMTime::new(value, TIMESCALE) }
}

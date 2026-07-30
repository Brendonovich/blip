#![cfg(target_os = "macos")]

mod camera;
mod decoder;
mod hls;

pub use camera::{
    CameraAuthorizationStatus, CameraCaptureFormat, CameraCapturer, CameraDevice, CameraError,
    CameraFrame, camera_authorization_status, list_video_devices, request_camera_access,
};
pub use decoder::*;
pub use hls::HlsWriter;

use std::fs;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::mpsc;
use std::time::Duration;

use blip_audio::AudioPacket;
use block2::RcBlock;
use core_foundation::base::TCFType as _;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVFileTypeQuickTimeMovie, AVMediaTypeAudio, AVMediaTypeVideo,
    AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264,
    AVVideoColorPrimaries_ITU_R_709_2, AVVideoColorPrimariesKey, AVVideoColorPropertiesKey,
    AVVideoCompressionPropertiesKey, AVVideoHeightKey, AVVideoTransferFunction_ITU_R_709_2,
    AVVideoTransferFunctionKey, AVVideoWidthKey, AVVideoYCbCrMatrix_ITU_R_709_2,
    AVVideoYCbCrMatrixKey,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMAudioFormatDescription, CMAudioFormatDescriptionCreate, CMBlockBuffer, CMSampleBuffer,
    CMSampleTimingInfo, CMTime, kCMBlockBufferAssureMemoryNowFlag, kCMTimeInvalid,
};
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
    #[error("HLS segment duration must be greater than zero")]
    InvalidSegmentDuration,
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
    #[error("failed to append audio samples: {0}")]
    AppendAudio(String),
    #[error("failed to write HLS output: {0}")]
    HlsOutput(String),
    #[error("recording contains no video frames")]
    NoFrames,
    #[error("video timestamp exceeds AVFoundation's range")]
    TimestampOverflow,
    #[error("timed out while finishing the MP4 file")]
    FinishTimeout,
    #[error("failed to finish MP4 file: {0}")]
    Finish(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoFileType {
    Mp4,
    Mov,
}

struct VideoColorProperties {
    primaries: Option<Retained<NSString>>,
    transfer_function: Option<Retained<NSString>>,
    ycbcr_matrix: Option<Retained<NSString>>,
}

struct Mp4WriterOptions {
    file_type: VideoFileType,
    bitrate: Option<usize>,
    color_properties: Option<VideoColorProperties>,
    system_audio: bool,
}

pub struct Mp4Writer {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    audio_input: Option<Retained<AVAssetWriterInput>>,
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
        Self::new_with_file_type(output, width, height, fps, VideoFileType::Mp4)
    }

    /// Creates an H.264 MP4 writer with a target average bitrate.
    ///
    /// # Errors
    ///
    /// Returns an error if the output cannot be prepared or `AVFoundation`
    /// rejects the writer configuration.
    pub fn new_with_bitrate(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        bitrate: usize,
    ) -> Result<Self, WriterError> {
        Self::new_with_options(
            output,
            width,
            height,
            fps,
            &Mp4WriterOptions {
                file_type: VideoFileType::Mp4,
                bitrate: Some(bitrate),
                color_properties: None,
                system_audio: false,
            },
        )
    }

    /// Creates an H.264 MP4 writer that preserves the source buffer's color metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the output cannot be prepared or `AVFoundation`
    /// rejects the writer configuration.
    pub fn new_preserving_color(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        _source: &CVPixelBuffer,
    ) -> Result<Self, WriterError> {
        Self::new_with_options(
            output,
            width,
            height,
            fps,
            &Mp4WriterOptions {
                file_type: VideoFileType::Mp4,
                bitrate: None,
                color_properties: rec709_color_properties(),
                system_audio: false,
            },
        )
    }

    /// Creates a bitrate-constrained H.264 MP4 writer that preserves source color metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the output cannot be prepared or `AVFoundation`
    /// rejects the writer configuration.
    pub fn new_with_bitrate_preserving_color(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        bitrate: usize,
        _source: &CVPixelBuffer,
    ) -> Result<Self, WriterError> {
        Self::new_with_options(
            output,
            width,
            height,
            fps,
            &Mp4WriterOptions {
                file_type: VideoFileType::Mp4,
                bitrate: Some(bitrate),
                color_properties: rec709_color_properties(),
                system_audio: false,
            },
        )
    }

    /// Creates a bitrate-constrained H.264 MP4 writer with an AAC system-audio track.
    ///
    /// # Errors
    ///
    /// Returns an error if the output cannot be prepared or `AVFoundation`
    /// rejects the video or audio writer configuration.
    pub fn new_with_bitrate_preserving_color_and_system_audio(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        bitrate: usize,
        _source: &CVPixelBuffer,
    ) -> Result<Self, WriterError> {
        Self::new_with_options(
            output,
            width,
            height,
            fps,
            &Mp4WriterOptions {
                file_type: VideoFileType::Mp4,
                bitrate: Some(bitrate),
                color_properties: rec709_color_properties(),
                system_audio: true,
            },
        )
    }

    /// Creates an H.264 writer for BGRA pixel buffers in the selected container.
    ///
    /// # Errors
    ///
    /// Returns an error if the output cannot be prepared or `AVFoundation`
    /// rejects the writer configuration.
    pub fn new_with_file_type(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        file_type: VideoFileType,
    ) -> Result<Self, WriterError> {
        Self::new_with_options(
            output,
            width,
            height,
            fps,
            &Mp4WriterOptions {
                file_type,
                bitrate: None,
                color_properties: None,
                system_audio: false,
            },
        )
    }

    #[allow(clippy::too_many_lines)]
    fn new_with_options(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        options: &Mp4WriterOptions,
    ) -> Result<Self, WriterError> {
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
        // SAFETY: These framework constants exist on every macOS version supported by AVAssetWriter.
        let (mp4_file_type, mov_file_type) = unsafe { (AVFileTypeMPEG4, AVFileTypeQuickTimeMovie) };
        let file_type = match options.file_type {
            VideoFileType::Mp4 => {
                mp4_file_type.ok_or(WriterError::MissingConstant("AVFileTypeMPEG4"))?
            }
            VideoFileType::Mov => {
                mov_file_type.ok_or(WriterError::MissingConstant("AVFileTypeQuickTimeMovie"))?
            }
        };
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
        let compression = options
            .bitrate
            .map(|bitrate| {
                // SAFETY: This setting is available with AVFoundation's H.264 encoder.
                let bitrate_key = unsafe { AVVideoAverageBitRateKey }
                    .ok_or(WriterError::MissingConstant("AVVideoAverageBitRateKey"))?;
                let bitrate = NSNumber::new_usize(bitrate);
                Ok::<_, WriterError>(NSDictionary::from_slices(&[bitrate_key], &[&*bitrate]))
            })
            .transpose()?;
        let color = options
            .color_properties
            .as_ref()
            .map(|color| -> Result<_, WriterError> {
                let mut keys = Vec::new();
                let mut values: Vec<&AnyObject> = Vec::new();
                if let Some(primaries) = &color.primaries {
                    // SAFETY: This setting is available with AVFoundation's H.264 encoder.
                    keys.push(
                        unsafe { AVVideoColorPrimariesKey }
                            .ok_or(WriterError::MissingConstant("AVVideoColorPrimariesKey"))?,
                    );
                    values.push(primaries);
                }
                if let Some(transfer_function) = &color.transfer_function {
                    // SAFETY: This setting is available with AVFoundation's H.264 encoder.
                    keys.push(
                        unsafe { AVVideoTransferFunctionKey }
                            .ok_or(WriterError::MissingConstant("AVVideoTransferFunctionKey"))?,
                    );
                    values.push(transfer_function);
                }
                if let Some(ycbcr_matrix) = &color.ycbcr_matrix {
                    // SAFETY: This setting is available with AVFoundation's H.264 encoder.
                    keys.push(
                        unsafe { AVVideoYCbCrMatrixKey }
                            .ok_or(WriterError::MissingConstant("AVVideoYCbCrMatrixKey"))?,
                    );
                    values.push(ycbcr_matrix);
                }
                Ok(NSDictionary::from_slices(&keys, &values))
            })
            .transpose()?;
        let mut keys = vec![codec_key, width_key, height_key];
        let mut values: Vec<&AnyObject> = vec![codec, &width, &height];
        if let Some(compression) = &compression {
            // SAFETY: This setting is available with AVFoundation's H.264 encoder.
            let compression_key = unsafe { AVVideoCompressionPropertiesKey }.ok_or(
                WriterError::MissingConstant("AVVideoCompressionPropertiesKey"),
            )?;
            keys.push(compression_key);
            values.push(&**compression);
        }
        if let Some(color) = &color {
            // SAFETY: This setting is available with AVFoundation's H.264 encoder.
            let color_key = unsafe { AVVideoColorPropertiesKey }
                .ok_or(WriterError::MissingConstant("AVVideoColorPropertiesKey"))?;
            keys.push(color_key);
            values.push(&**color);
        }
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
        let audio_input = add_audio_input(&writer, options.system_audio)?;
        // SAFETY: All inputs and configuration have been added.
        if !unsafe { writer.startWriting() } {
            return Err(WriterError::Start(writer_error(&writer)));
        }

        Ok(Self {
            writer,
            input,
            audio_input,
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

    /// Appends captured system-audio samples at the supplied source timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error if the timestamp overflows or `CoreMedia` or `AVFoundation`
    /// rejects the audio samples.
    pub fn append_audio(
        &mut self,
        packet: &AudioPacket,
        timestamp: Duration,
    ) -> Result<bool, WriterError> {
        let Some(input) = &self.audio_input else {
            return Ok(false);
        };
        // SAFETY: The audio input is retained and attached to the active writer.
        if !unsafe { input.isReadyForMoreMediaData() } {
            return Ok(false);
        }
        let timestamp_micros =
            i64::try_from(timestamp.as_micros()).map_err(|_| WriterError::TimestampOverflow)?;
        let start_timestamp = self.start_timestamp_micros.ok_or(WriterError::NoFrames)?;
        let presentation_timestamp = timestamp_micros.max(start_timestamp);
        let sample_buffer = audio_sample_buffer(packet, presentation_timestamp)?;
        // SAFETY: The input is ready and the sample buffer has a valid presentation timestamp.
        if !unsafe { input.appendSampleBuffer(&sample_buffer) } {
            return Err(WriterError::AppendAudio(writer_error(&self.writer)));
        }
        Ok(true)
    }

    /// Appends a pixel buffer from the `core-video` crate.
    ///
    /// # Errors
    ///
    /// Returns an error if the timestamp overflows or `AVFoundation` cannot
    /// append the frame.
    pub fn append_core_video(
        &mut self,
        pixel_buffer: &core_video::pixel_buffer::CVPixelBuffer,
        timestamp: Duration,
    ) -> Result<bool, WriterError> {
        let pixel_buffer = pixel_buffer.as_concrete_TypeRef().cast::<CVPixelBuffer>();
        // SAFETY: Both crates wrap the same Core Video CVPixelBufferRef object.
        self.append(unsafe { &*pixel_buffer }, timestamp)
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
            if let Some(input) = &self.audio_input {
                input.markAsFinished();
            }
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

fn rec709_color_properties() -> Option<VideoColorProperties> {
    // SAFETY: These framework constants are available with AVFoundation's H.264 encoder.
    let (primaries, transfer_function, ycbcr_matrix) = unsafe {
        (
            AVVideoColorPrimaries_ITU_R_709_2?,
            AVVideoTransferFunction_ITU_R_709_2?,
            AVVideoYCbCrMatrix_ITU_R_709_2?,
        )
    };
    Some(VideoColorProperties {
        primaries: Some(NSString::from_str(&primaries.to_string())),
        transfer_function: Some(NSString::from_str(&transfer_function.to_string())),
        ycbcr_matrix: Some(NSString::from_str(&ycbcr_matrix.to_string())),
    })
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

pub(crate) fn add_audio_input(
    writer: &AVAssetWriter,
    enabled: bool,
) -> Result<Option<Retained<AVAssetWriterInput>>, WriterError> {
    if !enabled {
        return Ok(None);
    }
    // SAFETY: This framework constant exists on every supported macOS version.
    let media_type =
        unsafe { AVMediaTypeAudio }.ok_or(WriterError::MissingConstant("AVMediaTypeAudio"))?;
    let keys = [
        NSString::from_str("AVFormatIDKey"),
        NSString::from_str("AVSampleRateKey"),
        NSString::from_str("AVNumberOfChannelsKey"),
        NSString::from_str("AVEncoderBitRateKey"),
    ];
    let key_refs: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let format_id = NSNumber::new_usize(0x6161_6320);
    let sample_rate = NSNumber::new_usize(48_000);
    let channels = NSNumber::new_usize(2);
    let bit_rate = NSNumber::new_usize(128_000);
    let value_refs: [&AnyObject; 4] = [&format_id, &sample_rate, &channels, &bit_rate];
    let settings = NSDictionary::from_slices(&key_refs, &value_refs);
    // SAFETY: The settings fully describe 48 kHz stereo AAC output.
    let input = unsafe {
        AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            media_type,
            Some(&settings),
        )
    };
    // SAFETY: This property is configured before writing starts.
    unsafe { input.setExpectsMediaDataInRealTime(true) };
    // SAFETY: The writer has not started and the input is configured for audio media.
    if !unsafe { writer.canAddInput(&input) } {
        return Err(WriterError::UnsupportedInput);
    }
    // SAFETY: `canAddInput` succeeded and the writer has not started.
    unsafe { writer.addInput(&input) };
    Ok(Some(input))
}

#[allow(clippy::too_many_lines)]
pub(crate) fn audio_sample_buffer(
    packet: &AudioPacket,
    presentation_timestamp_micros: i64,
) -> Result<CFRetained<CMSampleBuffer>, WriterError> {
    if packet.sample_rate() != 48_000 || packet.channels() != 2 {
        return Err(WriterError::AppendAudio(
            "expected 48 kHz stereo PCM audio".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(packet.samples().len().saturating_mul(size_of::<f32>()));
    for sample in packet.samples() {
        bytes.extend_from_slice(&sample.to_ne_bytes());
    }

    let mut raw_block_buffer = ptr::null_mut();
    // SAFETY: CoreMedia allocates and owns a block of the requested non-zero size.
    let status = unsafe {
        CMBlockBuffer::create_with_memory_block(
            None,
            ptr::null_mut(),
            bytes.len(),
            None,
            ptr::null(),
            0,
            bytes.len(),
            kCMBlockBufferAssureMemoryNowFlag,
            NonNull::from(&mut raw_block_buffer),
        )
    };
    if status != 0 {
        return Err(WriterError::AppendAudio(format!(
            "failed to allocate CoreMedia audio buffer ({status})"
        )));
    }
    let raw_block_buffer = NonNull::new(raw_block_buffer)
        .ok_or_else(|| WriterError::AppendAudio("CoreMedia returned no audio buffer".into()))?;
    // SAFETY: CoreMedia returned the block buffer at retain count one.
    let block_buffer = unsafe { CFRetained::from_raw(raw_block_buffer) };
    let source = NonNull::new(bytes.as_ptr().cast_mut().cast())
        .ok_or_else(|| WriterError::AppendAudio("PCM packet has no sample bytes".into()))?;
    // SAFETY: `source` references `bytes.len()` readable bytes and the destination has equal size.
    let status =
        unsafe { CMBlockBuffer::replace_data_bytes(source, &block_buffer, 0, bytes.len()) };
    if status != 0 {
        return Err(WriterError::AppendAudio(format!(
            "failed to copy PCM audio into CoreMedia ({status})"
        )));
    }

    let channels = u32::from(packet.channels());
    let sample_size =
        u32::try_from(size_of::<f32>()).map_err(|_| WriterError::TimestampOverflow)?;
    let bytes_per_frame = channels
        .checked_mul(sample_size)
        .ok_or(WriterError::TimestampOverflow)?;
    let mut description = AudioStreamBasicDescription {
        mSampleRate: f64::from(packet.sample_rate()),
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        mBytesPerPacket: bytes_per_frame,
        mFramesPerPacket: 1,
        mBytesPerFrame: bytes_per_frame,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 32,
        mReserved: 0,
    };
    let mut raw_format = ptr::null();
    // SAFETY: The ASBD fully describes packed native-endian interleaved Float32 PCM.
    let status = unsafe {
        CMAudioFormatDescriptionCreate(
            None,
            NonNull::from(&mut description),
            0,
            ptr::null(),
            0,
            ptr::null(),
            None,
            NonNull::from(&mut raw_format),
        )
    };
    if status != 0 {
        return Err(WriterError::AppendAudio(format!(
            "failed to describe PCM audio ({status})"
        )));
    }
    let raw_format = NonNull::new(raw_format.cast_mut())
        .ok_or_else(|| WriterError::AppendAudio("CoreMedia returned no audio format".into()))?;
    // SAFETY: CoreMedia returned the format description at retain count one.
    let format: CFRetained<CMAudioFormatDescription> = unsafe { CFRetained::from_raw(raw_format) };
    let timescale =
        i32::try_from(packet.sample_rate()).map_err(|_| WriterError::TimestampOverflow)?;
    let timing = CMSampleTimingInfo {
        // SAFETY: The validated sample rate is a positive CoreMedia timescale.
        duration: unsafe { CMTime::new(1, timescale) },
        presentationTimeStamp: cm_time(presentation_timestamp_micros),
        // SAFETY: Audio is emitted in presentation order and needs no decode timestamp.
        decodeTimeStamp: unsafe { kCMTimeInvalid },
    };
    let frame_count =
        isize::try_from(packet.frame_count()).map_err(|_| WriterError::TimestampOverflow)?;
    let sample_size =
        usize::try_from(bytes_per_frame).map_err(|_| WriterError::TimestampOverflow)?;
    let mut raw_sample_buffer = ptr::null_mut();
    // SAFETY: All retained inputs and timing/size pointers remain valid for this synchronous call.
    let status = unsafe {
        CMSampleBuffer::create_ready(
            None,
            Some(&block_buffer),
            Some(&format),
            frame_count,
            1,
            &raw const timing,
            1,
            &raw const sample_size,
            NonNull::from(&mut raw_sample_buffer),
        )
    };
    if status != 0 {
        return Err(WriterError::AppendAudio(format!(
            "failed to create PCM sample buffer ({status})"
        )));
    }
    let raw_sample_buffer = NonNull::new(raw_sample_buffer).ok_or_else(|| {
        WriterError::AppendAudio("CoreMedia returned no audio sample buffer".into())
    })?;
    // SAFETY: CoreMedia returned the sample buffer at retain count one.
    Ok(unsafe { CFRetained::from_raw(raw_sample_buffer) })
}

#[cfg(test)]
mod audio_tests {
    use super::*;

    #[test]
    fn creates_core_media_sample_from_platform_neutral_pcm()
    -> Result<(), Box<dyn std::error::Error>> {
        let packet = AudioPacket::new(vec![0.0, 0.25, 0.5, 0.75], 48_000, 2, None)?;
        let sample_buffer = audio_sample_buffer(&packet, 1_000_000)?;

        // SAFETY: The helper returned a fully initialized, retained sample buffer.
        assert_eq!(unsafe { sample_buffer.num_samples() }, 2);
        // SAFETY: The helper assigned a valid presentation timestamp.
        assert_eq!(
            unsafe { sample_buffer.presentation_time_stamp().seconds() },
            1.0
        );
        Ok(())
    }
}

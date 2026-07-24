use std::fmt::Write as _;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAssetSegmentReport, AVAssetSegmentType, AVAssetWriter, AVAssetWriterDelegate,
    AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeProfileMPEG4AppleHLS, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264,
    AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_media::CMTime;
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSData, NSDictionary, NSNumber, NSObject, NSObjectProtocol};
use objc2_uniform_type_identifiers::UTTypeMPEG4Movie;

use crate::{FINISH_TIMEOUT, WriterError, cm_time, writer_error};

const PLAYLIST_NAME: &str = "playlist.m3u8";
const INITIALIZATION_SEGMENT_NAME: &str = "init.mp4";

struct SegmentDelegateIvars {
    sink: Mutex<PlaylistSink>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = SegmentDelegateIvars]
    struct SegmentDelegate;

    unsafe impl NSObjectProtocol for SegmentDelegate {}

    unsafe impl AVAssetWriterDelegate for SegmentDelegate {
        #[unsafe(method(assetWriter:didOutputSegmentData:segmentType:segmentReport:))]
        unsafe fn did_output_segment(
            &self,
            _writer: &AVAssetWriter,
            segment_data: &NSData,
            segment_type: AVAssetSegmentType,
            segment_report: Option<&AVAssetSegmentReport>,
        ) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let Ok(mut sink) = self.ivars().sink.lock() else {
                    return;
                };
                if sink.error.is_some() {
                    return;
                }
                let result = copy_data(segment_data)
                    .and_then(|data| sink.write_segment(segment_type, segment_report, &data));
                if let Err(error) = result {
                    sink.error = Some(error.to_string());
                }
            }));
        }
    }
);

impl SegmentDelegate {
    fn new(output: PathBuf, segment_duration: Duration) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SegmentDelegateIvars {
            sink: Mutex::new(PlaylistSink::new(output, segment_duration)),
        });
        // SAFETY: `this` is allocated with fully initialized ivars and NSObject permits `init`.
        unsafe { msg_send![super(this), init] }
    }

    fn check_error(&self) -> Result<(), WriterError> {
        let sink = self
            .ivars()
            .sink
            .lock()
            .map_err(|_| WriterError::HlsOutput("segment writer lock was poisoned".into()))?;
        match &sink.error {
            Some(error) => Err(WriterError::HlsOutput(error.clone())),
            None => Ok(()),
        }
    }

    fn finish_playlist(&self) -> Result<(), WriterError> {
        let mut sink = self
            .ivars()
            .sink
            .lock()
            .map_err(|_| WriterError::HlsOutput("segment writer lock was poisoned".into()))?;
        if let Some(error) = &sink.error {
            return Err(WriterError::HlsOutput(error.clone()));
        }
        sink.finished = true;
        sink.write_playlist().map_err(WriterError::Output)
    }
}

struct PlaylistSink {
    output: PathBuf,
    configured_segment_duration: Duration,
    segment_durations: Vec<Duration>,
    initialized: bool,
    finished: bool,
    error: Option<String>,
}

impl PlaylistSink {
    fn new(output: PathBuf, segment_duration: Duration) -> Self {
        Self {
            output,
            configured_segment_duration: segment_duration,
            segment_durations: Vec::new(),
            initialized: false,
            finished: false,
            error: None,
        }
    }

    fn write_segment(
        &mut self,
        segment_type: AVAssetSegmentType,
        report: Option<&AVAssetSegmentReport>,
        data: &[u8],
    ) -> Result<(), std::io::Error> {
        if segment_type == AVAssetSegmentType::Initialization {
            fs::write(self.output.join(INITIALIZATION_SEGMENT_NAME), data)?;
            self.initialized = true;
        } else if segment_type == AVAssetSegmentType::Separable {
            let index = self.segment_durations.len();
            fs::write(self.output.join(segment_name(index)), data)?;
            self.segment_durations
                .push(report_duration(report).unwrap_or(self.configured_segment_duration));
        }
        self.write_playlist()
    }

    fn write_playlist(&self) -> Result<(), std::io::Error> {
        if !self.initialized {
            return Ok(());
        }
        let playlist = playlist(
            self.configured_segment_duration,
            &self.segment_durations,
            self.finished,
        );
        let temporary = self.output.join(format!(".{PLAYLIST_NAME}.tmp"));
        fs::write(&temporary, playlist)?;
        fs::rename(temporary, self.output.join(PLAYLIST_NAME))
    }
}

/// An AVFoundation-backed H.264 writer that produces an fMP4 HLS playlist.
pub struct HlsWriter {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    delegate: Retained<SegmentDelegate>,
    frame_duration: Duration,
    start_timestamp_micros: Option<i64>,
    last_relative_timestamp_micros: Option<i64>,
    started: bool,
    finished: bool,
}

impl HlsWriter {
    /// Creates an Apple HLS fMP4 writer for BGRA pixel buffers.
    ///
    /// The output directory is populated incrementally with `playlist.m3u8`,
    /// `init.mp4`, and numbered `.m4s` media segments.
    ///
    /// # Errors
    ///
    /// Returns an error when the dimensions, frame rate, or segment duration
    /// are invalid, or `AVFoundation` rejects the HLS writer configuration.
    pub fn new(
        output: &Path,
        width: usize,
        height: usize,
        fps: u32,
        segment_duration: Duration,
    ) -> Result<Self, WriterError> {
        if width == 0 || height == 0 {
            return Err(WriterError::InvalidDimensions);
        }
        let frame_duration = Duration::from_secs(1)
            .checked_div(fps)
            .ok_or(WriterError::InvalidFrameRate)?;
        if segment_duration.is_zero() {
            return Err(WriterError::InvalidSegmentDuration);
        }
        fs::create_dir_all(output)?;

        // SAFETY: UTTypeMPEG4Movie is a system-provided constant for the MPEG-4 container.
        let writer =
            unsafe { AVAssetWriter::initWithContentType(AVAssetWriter::alloc(), UTTypeMPEG4Movie) };
        // SAFETY: The Apple HLS profile is a system-provided AVFoundation constant.
        let profile = unsafe { AVFileTypeProfileMPEG4AppleHLS }.ok_or(
            WriterError::MissingConstant("AVFileTypeProfileMPEG4AppleHLS"),
        )?;
        // SAFETY: Segmentation properties are configured before writing starts.
        unsafe {
            writer.setOutputFileTypeProfile(Some(profile));
            writer.setPreferredOutputSegmentInterval(duration_time(segment_duration)?);
            writer.setInitialSegmentStartTime(cm_time(0));
        }

        // SAFETY: These constants are available with AVFoundation's H.264 encoder.
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
        // SAFETY: The dictionary contains the required H.264 output settings.
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        // SAFETY: This property must be configured before writing starts.
        unsafe { input.setExpectsMediaDataInRealTime(true) };
        // SAFETY: The writer and input are configured for MPEG-4 video.
        if !unsafe { writer.canAddInput(&input) } {
            return Err(WriterError::UnsupportedInput);
        }
        // SAFETY: `canAddInput` succeeded and writing has not started.
        unsafe { writer.addInput(&input) };
        // SAFETY: Captured pixel buffers are supplied directly.
        let adaptor = unsafe {
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &input,
                None,
            )
        };
        let delegate = SegmentDelegate::new(output.to_owned(), segment_duration);
        // SAFETY: The retained delegate implements AVAssetWriterDelegate and outlives the writer.
        unsafe {
            writer.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        // SAFETY: All writer configuration and inputs have been installed.
        if !unsafe { writer.startWriting() } {
            return Err(WriterError::Start(writer_error(&writer)));
        }

        Ok(Self {
            writer,
            input,
            adaptor,
            delegate,
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
    /// Returns an error if the timestamp overflows, encoding fails, or a
    /// playlist or segment cannot be written.
    pub fn append(
        &mut self,
        pixel_buffer: &CVPixelBuffer,
        timestamp: Duration,
    ) -> Result<bool, WriterError> {
        self.delegate.check_error()?;
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
        if !self.started {
            // SAFETY: Writing has started and HLS timestamps are normalized to zero.
            unsafe { self.writer.startSessionAtSourceTime(cm_time(0)) };
            self.started = true;
        }
        // SAFETY: The input is ready, the session is active, and the timestamp is numeric.
        if !unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(pixel_buffer, cm_time(relative_timestamp))
        } {
            return Err(WriterError::Append(writer_error(&self.writer)));
        }
        self.last_relative_timestamp_micros = Some(relative_timestamp);
        self.delegate.check_error()?;
        Ok(true)
    }

    /// Finalizes the final segment and writes an HLS VOD end marker.
    ///
    /// # Errors
    ///
    /// Returns an error when no frames were written or `AVFoundation` cannot
    /// finish encoding and writing the HLS output.
    pub fn finish(&mut self) -> Result<(), WriterError> {
        if self.finished {
            return Ok(());
        }
        let last_timestamp = self
            .last_relative_timestamp_micros
            .ok_or(WriterError::NoFrames)?;
        let frame_duration = i64::try_from(self.frame_duration.as_micros())
            .map_err(|_| WriterError::TimestampOverflow)?;
        let end_timestamp = last_timestamp
            .checked_add(frame_duration.max(1))
            .ok_or(WriterError::TimestampOverflow)?;
        // SAFETY: The session contains all frames and this is its final end time.
        unsafe {
            self.writer.endSessionAtSourceTime(cm_time(end_timestamp));
            self.input.markAsFinished();
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let completion = RcBlock::new(move || {
            let _ = sender.try_send(());
        });
        // SAFETY: All append calls are complete and the block owns its sender.
        unsafe { self.writer.finishWritingWithCompletionHandler(&completion) };
        receiver
            .recv_timeout(FINISH_TIMEOUT)
            .map_err(|_| WriterError::FinishTimeout)?;
        // SAFETY: The completion handler has fired, so status and error are stable.
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
            return Err(WriterError::Finish(writer_error(&self.writer)));
        }
        self.delegate.check_error()?;
        self.delegate.finish_playlist()?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for HlsWriter {
    fn drop(&mut self) {
        if !self.finished {
            // SAFETY: Cancellation is thread-safe and cleans up an unfinished writer.
            unsafe { self.writer.cancelWriting() };
        }
    }
}

fn duration_time(duration: Duration) -> Result<CMTime, WriterError> {
    let micros = i64::try_from(duration.as_micros()).map_err(|_| WriterError::TimestampOverflow)?;
    Ok(cm_time(micros))
}

fn report_duration(report: Option<&AVAssetSegmentReport>) -> Option<Duration> {
    let report = report?;
    // SAFETY: Segment reports are immutable for the duration of the delegate callback.
    let tracks = unsafe { report.trackReports() };
    if tracks.count() == 0 {
        return None;
    }
    let track = tracks.objectAtIndex(0);
    // SAFETY: The track report is immutable for the duration of this callback.
    Duration::try_from_secs_f64(unsafe { track.duration().seconds() }).ok()
}

fn copy_data(data: &NSData) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = vec![0; data.length()];
    if !bytes.is_empty() {
        let destination = NonNull::new(bytes.as_mut_ptr().cast())
            .ok_or_else(|| std::io::Error::other("failed to allocate segment data"))?;
        // SAFETY: `bytes` has exactly `data.length()` writable bytes.
        unsafe { data.getBytes_length(destination, bytes.len()) };
    }
    Ok(bytes)
}

fn segment_name(index: usize) -> String {
    format!("segment{index:05}.m4s")
}

fn playlist(
    configured_duration: Duration,
    segment_durations: &[Duration],
    finished: bool,
) -> String {
    let maximum_duration = segment_durations
        .iter()
        .copied()
        .max()
        .unwrap_or(configured_duration)
        .max(configured_duration);
    let target_duration = maximum_duration
        .as_secs()
        .saturating_add(u64::from(maximum_duration.subsec_nanos() != 0));
    let mut output = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:{}\n#EXT-X-MAP:URI=\"{}\"\n",
        target_duration.max(1),
        if finished { "VOD" } else { "EVENT" },
        INITIALIZATION_SEGMENT_NAME,
    );
    for (index, duration) in segment_durations.iter().enumerate() {
        let _ = writeln!(
            output,
            "#EXTINF:{:.6},\n{}",
            duration.as_secs_f64(),
            segment_name(index),
        );
    }
    if finished {
        output.push_str("#EXT-X-ENDLIST\n");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_incremental_event_playlist() {
        let playlist = playlist(
            Duration::from_secs(2),
            &[Duration::from_millis(2_001)],
            false,
        );
        assert!(playlist.contains("#EXT-X-TARGETDURATION:3"));
        assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(playlist.contains("#EXTINF:2.001000,\nsegment00000.m4s"));
        assert!(!playlist.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn finishes_as_vod_playlist() {
        let playlist = playlist(Duration::from_secs(2), &[], true);
        assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));
    }
}

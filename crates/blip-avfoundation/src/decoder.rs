#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division
)]

use std::path::{Path, PathBuf};
use std::ptr;
use std::time::Duration;

use core_foundation::base::TCFType as _;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderTrackOutput, AVAssetTrack, AVMediaTypeVideo, AVURLAsset,
};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

const TIMESCALE: i32 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum DecoderError {
    #[error("file path is not valid UTF-8")]
    InvalidPath,
    #[error("no video track found in file")]
    NoVideoTrack,
    #[error("failed to create AVAssetReader: {0}")]
    CreateReader(String),
    #[error("failed to create AVAssetReaderTrackOutput: {0}")]
    CreateOutput(String),
    #[error("failed to start reading: {0}")]
    StartReading(String),
    #[error("failed to decode frame at requested time")]
    DecodeFailed,
}

struct ActiveReader {
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
    last_frame: core_video::pixel_buffer::CVPixelBuffer,
    last_ts: f64,
}

pub struct VideoDecoder {
    path: PathBuf,
    asset: Retained<AVURLAsset>,
    track: Retained<AVAssetTrack>,
    duration: Duration,
    width: usize,
    height: usize,
    nominal_fps: f64,
    reader: Option<ActiveReader>,
    cache: std::collections::BTreeMap<i64, core_video::pixel_buffer::CVPixelBuffer>,
}

impl VideoDecoder {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DecoderError> {
        let path = path.as_ref().to_path_buf();
        let url_str = path.to_str().ok_or(DecoderError::InvalidPath)?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(url_str));
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };

        #[allow(deprecated)]
        let media_type = unsafe { AVMediaTypeVideo }.ok_or(DecoderError::NoVideoTrack)?;
        #[allow(deprecated)]
        let tracks = unsafe { asset.tracksWithMediaType(media_type) };
        let track = tracks.firstObject().ok_or(DecoderError::NoVideoTrack)?;

        let cm_duration = unsafe { asset.duration() };
        let duration_secs = if cm_duration.timescale > 0 {
            cm_duration.value as f64 / cm_duration.timescale as f64
        } else {
            0.0
        };
        let duration = Duration::from_secs_f64(duration_secs.max(0.0));

        let size = unsafe { track.naturalSize() };
        let width = size.width.abs() as usize;
        let height = size.height.abs() as usize;

        let nominal_fps = unsafe { track.nominalFrameRate() } as f64;
        let nominal_fps = if nominal_fps > 0.0 { nominal_fps } else { 30.0 };

        Ok(Self {
            path,
            asset,
            track,
            duration,
            width,
            height,
            nominal_fps,
            reader: None,
            cache: std::collections::BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub fn nominal_fps(&self) -> f64 {
        self.nominal_fps
    }

    pub fn frame_at(
        &mut self,
        time_secs: f64,
    ) -> Result<core_video::pixel_buffer::CVPixelBuffer, DecoderError> {
        let tolerance = 0.5 / self.nominal_fps;
        let target_micros = (time_secs * TIMESCALE as f64).round() as i64;
        let tol_micros = (tolerance * TIMESCALE as f64).round() as i64;
        if let Some((_, frame)) = self
            .cache
            .range((target_micros - tol_micros)..=(target_micros + tol_micros))
            .min_by_key(|(ts_micros, _)| (**ts_micros - target_micros).abs())
        {
            return Ok(frame.clone());
        }
        if let Some(mut active) = self.reader.take() {
            let delta = time_secs - active.last_ts;
            if delta >= -0.01 && delta <= 3.0 {
                if delta <= tolerance {
                    let res = active.last_frame.clone();
                    self.reader = Some(active);
                    return Ok(res);
                }
                let mut found = false;
                while let Some(sample_buffer) = unsafe { active.output.copyNextSampleBuffer() } {
                    let ts = unsafe { sample_buffer.presentation_time_stamp().seconds() };
                    if let Some(image_buffer) = unsafe { sample_buffer.image_buffer() } {
                        let pixel_buffer = ptr::from_ref(&*image_buffer)
                            .cast_mut()
                            .cast::<core_video::buffer::__CVBuffer>();
                        let cv_pixel_buffer = unsafe {
                            core_video::pixel_buffer::CVPixelBuffer::wrap_under_get_rule(
                                pixel_buffer,
                            )
                        };
                        let ts_micros = (ts * TIMESCALE as f64).round() as i64;
                        self.cache.insert(ts_micros, cv_pixel_buffer.clone());
                        active.last_frame = cv_pixel_buffer;
                        active.last_ts = ts;
                    }
                    if active.last_ts >= time_secs - tolerance {
                        found = true;
                        break;
                    }
                }
                if found {
                    let res = active.last_frame.clone();
                    self.reader = Some(active);
                    return Ok(res);
                }
            }
            unsafe { active.reader.cancelReading() };
        }

        let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&self.asset) }
            .map_err(|e| DecoderError::CreateReader(e.to_string()))?;

        let value = (time_secs * TIMESCALE as f64).round() as i64;
        let start = unsafe { CMTime::new(value, TIMESCALE) };
        let duration = unsafe { CMTime::new((self.duration.as_secs() + 3600) as i64, 1) };
        let time_range = unsafe { CMTimeRange::new(start, duration) };
        unsafe { reader.setTimeRange(time_range) };

        let key_pf = NSString::from_str("PixelFormatType");
        let key_w = NSString::from_str("Width");
        let key_h = NSString::from_str("Height");
        let keys: [&NSString; 3] = [&*key_pf, &*key_w, &*key_h];

        let pf_val = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
        let w_val = NSNumber::new_usize(self.width);
        let h_val = NSNumber::new_usize(self.height);
        let values: [&AnyObject; 3] = [&*pf_val, &*w_val, &*h_val];
        let settings = NSDictionary::from_slices(&keys, &values);

        let output = unsafe {
            AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                &self.track,
                Some(&settings),
            )
        };
        unsafe { output.setAlwaysCopiesSampleData(true) };

        if !unsafe { reader.canAddOutput(&output) } {
            return Err(DecoderError::CreateOutput(
                "canAddOutput returned false".into(),
            ));
        }
        unsafe { reader.addOutput(&output) };

        if !unsafe { reader.startReading() } {
            return Err(DecoderError::StartReading(
                "startReading returned false".into(),
            ));
        }

        let mut last_frame = None;
        let mut last_ts = 0.0;
        while let Some(sample_buffer) = unsafe { output.copyNextSampleBuffer() } {
            let ts = unsafe { sample_buffer.presentation_time_stamp().seconds() };
            if let Some(image_buffer) = unsafe { sample_buffer.image_buffer() } {
                let pixel_buffer = ptr::from_ref(&*image_buffer)
                    .cast_mut()
                    .cast::<core_video::buffer::__CVBuffer>();
                let cv_pixel_buffer = unsafe {
                    core_video::pixel_buffer::CVPixelBuffer::wrap_under_get_rule(pixel_buffer)
                };
                let ts_micros = (ts * TIMESCALE as f64).round() as i64;
                self.cache.insert(ts_micros, cv_pixel_buffer.clone());
                last_frame = Some(cv_pixel_buffer);
                last_ts = ts;
            }
            if last_ts >= time_secs - tolerance {
                break;
            }
        }

        let cv_pixel_buffer = last_frame.ok_or(DecoderError::DecodeFailed)?;

        self.reader = Some(ActiveReader {
            reader,
            output,
            last_frame: cv_pixel_buffer.clone(),
            last_ts,
        });

        Ok(cv_pixel_buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_cache_backward_scrubbing() {
        let path = PathBuf::from("/tmp/test-bundle.blip/inputs/screen.mp4");
        if !path.exists() {
            return;
        }
        let mut decoder = VideoDecoder::open(&path).expect("open decoder");
        let _ = decoder.frame_at(0.0).expect("frame 0.0");
        let _ = decoder.frame_at(0.1).expect("frame 0.1");
        let _ = decoder.frame_at(0.2).expect("frame 0.2");
        let _ = decoder.frame_at(0.05).expect("frame 0.05 backward");
        let _ = decoder.frame_at(0.15).expect("frame 0.15 backward");
    }
}

use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::kCGColorSpaceSRGB;
use objc2_core_media::CMTime;
use objc2_core_video::{kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange};
use objc2_screen_capture_kit::SCStreamConfiguration;

use crate::{CaptureError, CaptureFilter};

pub struct StreamConfig {
    raw: Retained<SCStreamConfiguration>,
    captures_audio: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum PixelFormat {
    /// Eight-bit BGRA with one packed plane.
    #[default]
    Bgra,
    /// Eight-bit full-range 4:2:0 YCbCr with separate luma and chroma planes.
    Yuv420BiPlanarFullRange,
}

#[derive(Debug, Clone, Copy)]
pub enum CaptureColorSpace {
    Srgb,
}

impl PixelFormat {
    const fn as_raw(self) -> u32 {
        match self {
            Self::Bgra => kCVPixelFormatType_32BGRA,
            Self::Yuv420BiPlanarFullRange => kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        }
    }
}

impl StreamConfig {
    #[must_use]
    pub fn builder() -> StreamConfigBuilder {
        StreamConfigBuilder::default()
    }

    #[must_use]
    pub(crate) fn as_raw(&self) -> &SCStreamConfiguration {
        &self.raw
    }

    #[must_use]
    pub(crate) const fn captures_audio(&self) -> bool {
        self.captures_audio
    }
}

#[derive(Debug, Clone, Default)]
pub struct StreamConfigBuilder {
    fps: Option<u32>,
    shows_cursor: bool,
    queue_depth: Option<u8>,
    pixel_format: PixelFormat,
    color_space: Option<CaptureColorSpace>,
    source_rect: Option<(f64, f64, f64, f64)>,
    captures_audio: bool,
}

impl StreamConfigBuilder {
    pub(crate) fn build_for(self, filter: &CaptureFilter) -> Result<StreamConfig, CaptureError> {
        let (width, height) = self
            .source_rect
            .map_or_else(
                || filter.capture_size(),
                |rect| {
                    dimensions(
                        rect.2 * filter.point_pixel_scale(),
                        rect.3 * filter.point_pixel_scale(),
                    )
                },
            )
            .ok_or_else(|| {
                CaptureError::InvalidConfiguration(
                    "failed to determine the capture target's pixel dimensions".into(),
                )
            })?;
        self.build(width, height)
    }

    #[must_use]
    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = Some(fps);
        self
    }

    #[must_use]
    pub fn with_cursor(mut self, shows_cursor: bool) -> Self {
        self.shows_cursor = shows_cursor;
        self
    }

    #[must_use]
    pub fn with_queue_depth(mut self, queue_depth: u8) -> Self {
        self.queue_depth = Some(queue_depth);
        self
    }

    /// Sets the pixel layout requested from `ScreenCaptureKit`.
    #[must_use]
    pub fn with_pixel_format(mut self, pixel_format: PixelFormat) -> Self {
        self.pixel_format = pixel_format;
        self
    }

    /// Requests color conversion into a known output color space.
    #[must_use]
    pub fn with_color_space(mut self, color_space: CaptureColorSpace) -> Self {
        self.color_space = Some(color_space);
        self
    }

    /// Crops the captured display to a rectangle in logical display coordinates.
    #[must_use]
    pub fn with_source_rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.source_rect = Some((x, y, width, height));
        self
    }

    /// Requests system audio alongside screen frames on macOS 13 and later.
    #[must_use]
    pub fn with_system_audio(mut self, captures_audio: bool) -> Self {
        self.captures_audio = captures_audio;
        self
    }

    fn build(self, width: usize, height: usize) -> Result<StreamConfig, CaptureError> {
        let timescale = self
            .fps
            .map(|fps| {
                if fps == 0 {
                    return Err(CaptureError::InvalidConfiguration(
                        "frame rate must be greater than zero".into(),
                    ));
                }
                i32::try_from(fps).map_err(|_| {
                    CaptureError::InvalidConfiguration(
                        "frame rate exceeds CoreMedia's supported range".into(),
                    )
                })
            })
            .transpose()?;

        if self
            .queue_depth
            .is_some_and(|depth| !(1..=8).contains(&depth))
        {
            return Err(CaptureError::InvalidConfiguration(
                "queue depth must be between 1 and 8".into(),
            ));
        }

        if self.source_rect.is_some_and(|(x, y, width, height)| {
            !x.is_finite()
                || !y.is_finite()
                || !width.is_finite()
                || !height.is_finite()
                || width <= 0.0
                || height <= 0.0
        }) {
            return Err(CaptureError::InvalidConfiguration(
                "source rectangle must have finite coordinates and positive dimensions".into(),
            ));
        }

        // SAFETY: `new` is available on every macOS version that provides ScreenCaptureKit.
        let config = unsafe { SCStreamConfiguration::new() };
        // SAFETY: Values have been validated against ScreenCaptureKit and CoreMedia constraints.
        unsafe {
            config.setWidth(width);
            config.setHeight(height);
            if let Some(timescale) = timescale {
                config.setMinimumFrameInterval(CMTime::new(1, timescale));
            }
            if let Some(queue_depth) = self.queue_depth {
                config.setQueueDepth(isize::from(queue_depth));
            }
            config.setPixelFormat(self.pixel_format.as_raw());
            if let Some(color_space) = self.color_space {
                let name = match color_space {
                    CaptureColorSpace::Srgb => kCGColorSpaceSRGB,
                };
                config.setColorSpaceName(name);
            }
            config.setShowsCursor(self.shows_cursor);
            if self.captures_audio && objc2::available!(macos = 13.0) {
                config.setCapturesAudio(true);
                config.setSampleRate(48_000);
                config.setChannelCount(2);
            }
            if let Some((x, y, width, height)) = self.source_rect {
                config.setSourceRect(CGRect::new(CGPoint::new(x, y), CGSize::new(width, height)));
            }
        }

        Ok(StreamConfig {
            raw: config,
            captures_audio: self.captures_audio && objc2::available!(macos = 13.0),
        })
    }
}

fn dimensions(width: f64, height: f64) -> Option<(usize, usize)> {
    fn dimension(value: f64) -> Option<usize> {
        if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
            return None;
        }
        #[allow(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        Some(value.round() as usize)
    }

    Some((dimension(width)?, dimension(height)?))
}

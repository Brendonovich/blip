use std::ptr::{self, NonNull};
use std::slice;

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CGRect, Type};
use objc2_core_graphics::CGRectMakeWithDictionaryRepresentation;
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    CVImageBuffer, CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetHeight, CVPixelBufferGetIOSurface, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVReturnSuccess,
};
use objc2_screen_capture_kit::{
    SCStreamFrameInfo, SCStreamFrameInfoContentRect, SCStreamFrameInfoContentScale,
    SCStreamFrameInfoScaleFactor,
};

use crate::CaptureError;

pub struct VideoFrame {
    sample_buffer: CFRetained<CMSampleBuffer>,
    image_buffer: CFRetained<CVImageBuffer>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameGeometry {
    pub content_rect: FrameRect,
    pub native_dimensions: (usize, usize),
}

impl FrameRect {
    #[must_use]
    pub fn dimensions(self) -> Option<(usize, usize)> {
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

        Some((dimension(self.width)?, dimension(self.height)?))
    }
}

// SAFETY: Captured sample and image buffers are immutable while retained, and CoreMedia and
// CoreVideo permit retained buffers to move between processing queues.
unsafe impl Send for VideoFrame {}

impl VideoFrame {
    pub(crate) fn new(sample_buffer: &CMSampleBuffer) -> Option<Self> {
        // SAFETY: ScreenCaptureKit provides a valid sample for the duration of its callback.
        let image_buffer = unsafe { sample_buffer.image_buffer() }?;
        Some(Self {
            sample_buffer: sample_buffer.retain(),
            image_buffer,
        })
    }

    #[must_use]
    pub fn sample_buffer(&self) -> &CMSampleBuffer {
        &self.sample_buffer
    }

    #[must_use]
    pub fn image_buffer(&self) -> &CVImageBuffer {
        &self.image_buffer
    }

    #[must_use]
    pub fn width(&self) -> usize {
        CVPixelBufferGetWidth(self.image_buffer())
    }

    #[must_use]
    pub fn height(&self) -> usize {
        CVPixelBufferGetHeight(self.image_buffer())
    }

    /// Returns the live content rectangle within this frame's pixel buffer.
    #[must_use]
    pub fn content_rect(&self) -> Option<FrameRect> {
        self.geometry().map(|geometry| geometry.content_rect)
    }

    /// Returns the live output rectangle and unscaled source dimensions.
    #[must_use]
    pub fn geometry(&self) -> Option<FrameGeometry> {
        // SAFETY: ScreenCaptureKit exports this process-lifetime frame-info key.
        self.frame_geometry(unsafe { SCStreamFrameInfoContentRect })
    }

    fn frame_geometry(&self, key: &SCStreamFrameInfo) -> Option<FrameGeometry> {
        // SAFETY: The sample buffer is retained, and attachments are requested read-only.
        let attachments = unsafe { self.sample_buffer.sample_attachments_array(false) }?;
        // SAFETY: ScreenCaptureKit documents every sample attachment as a CFDictionary.
        let attachments =
            unsafe { CFRetained::cast_unchecked::<CFArray<CFDictionary>>(attachments) };
        let attachment = attachments.get(0)?;
        let value = attachment_value(&attachment, key);
        // SAFETY: The non-null value for this key is documented as a CFDictionary.
        let dictionary = unsafe { value.cast::<CFDictionary>().as_ref() }?;
        let mut rect = CGRect::ZERO;
        // SAFETY: `dictionary` is the CGRect representation and `rect` is writable.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(dictionary), &raw mut rect) } {
            return None;
        }
        // SAFETY: ScreenCaptureKit exports these process-lifetime frame-info keys.
        let (content_scale_key, scale_factor_key) =
            unsafe { (SCStreamFrameInfoContentScale, SCStreamFrameInfoScaleFactor) };
        let content_scale = attachment_number(&attachment, content_scale_key)?;
        let scale_factor = attachment_number(&attachment, scale_factor_key)?;
        let content_rect = FrameRect {
            x: rect.origin.x * scale_factor,
            y: rect.origin.y * scale_factor,
            width: rect.size.width * scale_factor,
            height: rect.size.height * scale_factor,
        };
        let native_dimensions = FrameRect {
            x: 0.0,
            y: 0.0,
            width: content_rect.width / content_scale,
            height: content_rect.height / content_scale,
        }
        .dimensions()?;
        Some(FrameGeometry {
            content_rect,
            native_dimensions,
        })
    }

    /// Copies the frame's native pixel data into one contiguous byte buffer.
    ///
    /// Planes are appended in their native order. Row and plane padding are
    /// omitted, but the pixel format and component layout are otherwise unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame is not IOSurface-backed, its dimensions
    /// overflow, or `CoreVideo` does not expose readable pixel data.
    pub fn to_bytes(&self) -> Result<Vec<u8>, CaptureError> {
        let pixel_buffer = self.image_buffer();
        let _lock = PixelBufferLock::new(pixel_buffer)?;
        let surface = CVPixelBufferGetIOSurface(Some(pixel_buffer)).ok_or_else(|| {
            CaptureError::InvalidFrame("capture pixel buffer is not IOSurface-backed".into())
        })?;
        let mut bytes = Vec::new();

        if surface.plane_count() == 0 {
            copy_surface_rows(
                &mut bytes,
                SurfacePlane {
                    base: NonNull::new(CVPixelBufferGetBaseAddress(pixel_buffer).cast())
                        .ok_or_else(|| {
                            CaptureError::InvalidFrame("capture has no pixel data".into())
                        })?,
                    width: surface.width(),
                    height: surface.height(),
                    element_width: surface.element_width(),
                    element_height: surface.element_height(),
                    bytes_per_element: surface.bytes_per_element(),
                    stride: surface.bytes_per_row(),
                },
            )?;
        } else {
            for plane in 0..surface.plane_count() {
                copy_surface_rows(
                    &mut bytes,
                    SurfacePlane {
                        base: NonNull::new(
                            CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, plane).cast(),
                        )
                        .ok_or_else(|| {
                            CaptureError::InvalidFrame(format!(
                                "capture plane {plane} has no pixel data"
                            ))
                        })?,
                        width: surface.width_of_plane(plane),
                        height: surface.height_of_plane(plane),
                        element_width: surface.element_width_of_plane(plane),
                        element_height: surface.element_height_of_plane(plane),
                        bytes_per_element: surface.bytes_per_element_of_plane(plane),
                        stride: surface.bytes_per_row_of_plane(plane),
                    },
                )?;
            }
        }

        Ok(bytes)
    }
}

fn attachment_value(attachment: &CFDictionary, key: &SCStreamFrameInfo) -> *const std::ffi::c_void {
    // SAFETY: ScreenCaptureKit's attachment dictionary uses retained NSString keys.
    unsafe { attachment.value(ptr::from_ref(key).cast()) }
}

fn attachment_number(attachment: &CFDictionary, key: &SCStreamFrameInfo) -> Option<f64> {
    let value = attachment_value(attachment, key);
    // SAFETY: ScreenCaptureKit documents these frame-info values as CFNumbers.
    unsafe { value.cast::<CFNumber>().as_ref() }?.as_f64()
}

#[derive(Clone, Copy)]
struct SurfacePlane {
    base: NonNull<u8>,
    width: usize,
    height: usize,
    element_width: usize,
    element_height: usize,
    bytes_per_element: usize,
    stride: usize,
}

fn copy_surface_rows(output: &mut Vec<u8>, plane: SurfacePlane) -> Result<(), CaptureError> {
    let elements_per_row = plane.width.div_ceil(plane.element_width.max(1));
    let rows = plane.height.div_ceil(plane.element_height.max(1));
    let row_len = elements_per_row
        .checked_mul(plane.bytes_per_element)
        .ok_or_else(|| CaptureError::InvalidFrame("capture row size overflow".into()))?;
    if plane.stride < row_len {
        return Err(CaptureError::InvalidFrame(
            "capture row stride is shorter than its pixel data".into(),
        ));
    }
    let source_len = plane
        .stride
        .checked_mul(rows)
        .ok_or_else(|| CaptureError::InvalidFrame("capture buffer size overflow".into()))?;
    // SAFETY: The IOSurface is retained and its pixel buffer is locked, so CoreVideo
    // guarantees `stride * rows` accessible bytes from the plane's base address.
    let source = unsafe { slice::from_raw_parts(plane.base.as_ptr(), source_len) };
    for row in source.chunks_exact(plane.stride).take(rows) {
        let data = row.get(..row_len).ok_or_else(|| {
            CaptureError::InvalidFrame("capture row is shorter than expected".into())
        })?;
        output.extend_from_slice(data);
    }
    Ok(())
}

struct PixelBufferLock<'a>(&'a CVPixelBuffer);

impl<'a> PixelBufferLock<'a> {
    fn new(pixel_buffer: &'a CVPixelBuffer) -> Result<Self, CaptureError> {
        // SAFETY: `pixel_buffer` is retained and the read-only lock is paired in `Drop`.
        let status =
            unsafe { CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status != kCVReturnSuccess {
            return Err(CaptureError::InvalidFrame(format!(
                "failed to lock capture pixel buffer ({status})"
            )));
        }
        Ok(Self(pixel_buffer))
    }
}

impl Drop for PixelBufferLock<'_> {
    fn drop(&mut self) {
        // SAFETY: Construction succeeded with the same pixel buffer and lock flags.
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly);
        }
    }
}

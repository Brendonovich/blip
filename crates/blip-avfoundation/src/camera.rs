use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
#[allow(deprecated)]
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeContinuityCamera,
    AVCaptureDeviceTypeDeskViewCamera, AVCaptureDeviceTypeExternal,
    AVCaptureDeviceTypeExternalUnknown, AVCaptureOutput, AVCaptureSession,
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFRetained, Type as _};
use objc2_core_media::{CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{
    CVImageBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSProcessInfo, NSString,
};

static SESSION_LIFECYCLE: Mutex<()> = Mutex::new(());

type FrameCallback = Box<dyn FnMut(CameraFrame) + Send>;
type DropCallback = Box<dyn FnMut() + Send>;

#[derive(Clone)]
pub struct CameraDevice {
    device: Retained<AVCaptureDevice>,
    unique_id: String,
    localized_name: String,
}

// SAFETY: Camera device descriptors are immutable retained AVFoundation objects. Device
// configuration remains serialized by `CameraCapturer`'s lifecycle lock.
unsafe impl Send for CameraDevice {}

impl CameraDevice {
    #[must_use]
    pub fn unique_id(&self) -> &str {
        &self.unique_id
    }

    #[must_use]
    pub fn localized_name(&self) -> &str {
        &self.localized_name
    }
}

pub struct CameraFrame {
    sample_buffer: CFRetained<CMSampleBuffer>,
    image_buffer: CFRetained<CVImageBuffer>,
}

// SAFETY: Captured sample and image buffers are immutable while retained, and CoreMedia and
// CoreVideo permit retained buffers to move between processing queues.
unsafe impl Send for CameraFrame {}

impl CameraFrame {
    fn new(sample_buffer: &CMSampleBuffer) -> Option<Self> {
        // SAFETY: AVFoundation provides a valid sample for the duration of the delegate callback.
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

    #[must_use]
    pub fn presentation_timestamp(&self) -> Option<Duration> {
        // SAFETY: The retained sample buffer has immutable timing metadata.
        let seconds = unsafe { self.sample_buffer.presentation_time_stamp().seconds() };
        Duration::try_from_secs_f64(seconds).ok()
    }
}

struct OutputIvars {
    callbacks: Mutex<Callbacks>,
}

struct Callbacks {
    frame: FrameCallback,
    dropped: Option<DropCallback>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = OutputIvars]
    struct CameraOutput;

    unsafe impl NSObjectProtocol for CameraOutput {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn capture_output(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let Some(frame) = CameraFrame::new(sample_buffer) else {
                return;
            };
            let _ = catch_unwind(AssertUnwindSafe(|| {
                if let Ok(mut callbacks) = self.ivars().callbacks.lock() {
                    (callbacks.frame)(frame);
                }
            }));
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        unsafe fn capture_output_did_drop(
            &self,
            _output: &AVCaptureOutput,
            _sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                if let Ok(mut callbacks) = self.ivars().callbacks.lock()
                    && let Some(callback) = &mut callbacks.dropped
                {
                    callback();
                }
            }));
        }
    }
);

impl CameraOutput {
    fn new(callbacks: Callbacks) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            callbacks: Mutex::new(callbacks),
        });
        // SAFETY: `this` is allocated with fully initialized ivars and NSObject permits `init`.
        unsafe { msg_send![super(this), init] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraAuthorizationStatus {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("AVFoundation video media type is unavailable")]
    MissingVideoMediaType,
    #[error("camera permission was denied")]
    PermissionDenied,
    #[error("timed out while requesting camera permission")]
    PermissionTimeout,
    #[error("camera {0} is no longer available")]
    DeviceUnavailable(String),
    #[error("failed to open camera: {0}")]
    OpenDevice(String),
    #[error("AVFoundation rejected the camera input")]
    UnsupportedInput,
    #[error("AVFoundation rejected the camera output")]
    UnsupportedOutput,
    #[error("camera frame rate must be greater than zero")]
    InvalidFrameRate,
    #[error("failed to configure camera: {0}")]
    Configure(String),
}

struct SelectedFormat {
    format: Retained<AVCaptureDeviceFormat>,
    frame_duration: CMTime,
    pixel_format: u32,
    dimensions: (u32, u32),
    frame_rate: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct CameraCaptureFormat {
    pub dimensions: (u32, u32),
    pub frame_rate: f64,
    pub pixel_format: u32,
}

pub struct CameraCapturer {
    session: Retained<AVCaptureSession>,
    input: Retained<AVCaptureDeviceInput>,
    output: Retained<AVCaptureVideoDataOutput>,
    device: Retained<AVCaptureDevice>,
    _queue: DispatchRetained<DispatchQueue>,
    _delegate: Retained<CameraOutput>,
    selected_format: Option<SelectedFormat>,
    running: AtomicBool,
}

// SAFETY: AVFoundation capture sessions may be started and stopped off the main thread. All
// lifecycle operations are serialized, and frame callbacks run on the retained dispatch queue.
unsafe impl Send for CameraCapturer {}

impl CameraCapturer {
    /// Creates a camera capture session, preferring GPU-friendly bi-planar YUV output.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is unavailable or `AVFoundation` rejects the session.
    pub fn new(
        device: &CameraDevice,
        fps: u32,
        callback: impl FnMut(CameraFrame) + Send + 'static,
    ) -> Result<Self, CameraError> {
        Self::new_inner(device, fps, Box::new(callback), None)
    }

    /// Creates a camera capture session and reports frames dropped by `AVFoundation`.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is unavailable or `AVFoundation` rejects the session.
    pub fn new_with_drop_callback(
        device: &CameraDevice,
        fps: u32,
        callback: impl FnMut(CameraFrame) + Send + 'static,
        drop_callback: impl FnMut() + Send + 'static,
    ) -> Result<Self, CameraError> {
        Self::new_inner(
            device,
            fps,
            Box::new(callback),
            Some(Box::new(drop_callback)),
        )
    }

    fn new_inner(
        device: &CameraDevice,
        fps: u32,
        callback: FrameCallback,
        drop_callback: Option<DropCallback>,
    ) -> Result<Self, CameraError> {
        if fps == 0 {
            return Err(CameraError::InvalidFrameRate);
        }
        ensure_camera_access()?;
        let raw_device = device.device.clone();
        // SAFETY: The enumerated device is retained while its connection state is queried.
        if !unsafe { raw_device.isConnected() } {
            return Err(CameraError::DeviceUnavailable(
                device.localized_name().to_owned(),
            ));
        }
        // SAFETY: The retained device is a currently enumerated video capture device.
        let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&raw_device) }
            .map_err(|error| CameraError::OpenDevice(error.localizedDescription().to_string()))?;
        // SAFETY: Both objects are initialized before being configured and retained by the capturer.
        let (session, output) =
            unsafe { (AVCaptureSession::new(), AVCaptureVideoDataOutput::new()) };
        let delegate = CameraOutput::new(Callbacks {
            frame: callback,
            dropped: drop_callback,
        });
        let queue = DispatchQueue::new("dev.brendonovich.blip.camera", None);
        let selected_format = select_format(&raw_device, fps);

        // SAFETY: Configuration is completed before the session starts and both objects are retained.
        unsafe {
            session.beginConfiguration();
            if !session.canAddInput(&input) {
                session.commitConfiguration();
                return Err(CameraError::UnsupportedInput);
            }
            session.addInput(&input);
            if !session.canAddOutput(&output) {
                session.removeInput(&input);
                session.commitConfiguration();
                return Err(CameraError::UnsupportedOutput);
            }
            session.addOutput(&output);
            output.setAlwaysDiscardsLateVideoFrames(true);
            let available_formats = output.availableVideoCVPixelFormatTypes();
            let pixel_format = selected_format
                .as_ref()
                .map(|selected| selected.pixel_format)
                .filter(|selected| is_gpu_supported_format(*selected))
                .filter(|selected| {
                    available_formats
                        .iter()
                        .any(|available| available.as_u32() == *selected)
                })
                .or_else(|| {
                    [
                        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                        kCVPixelFormatType_32BGRA,
                    ]
                    .into_iter()
                    .find(|candidate| {
                        available_formats
                            .iter()
                            .any(|available| available.as_u32() == *candidate)
                    })
                })
                .unwrap_or(kCVPixelFormatType_32BGRA);
            let key = NSString::from_str("PixelFormatType");
            let value = NSNumber::new_u32(pixel_format);
            let values: [&AnyObject; 1] = [&*value];
            let settings = NSDictionary::from_slices(&[&*key], &values);
            output.setVideoSettings(Some(&settings));
            let protocol =
                ProtocolObject::<dyn AVCaptureVideoDataOutputSampleBufferDelegate>::from_ref(
                    &*delegate,
                );
            output.setSampleBufferDelegate_queue(Some(protocol), Some(&queue));
            session.commitConfiguration();
        }

        Ok(Self {
            session,
            input,
            output,
            device: raw_device,
            _queue: queue,
            _delegate: delegate,
            selected_format,
            running: AtomicBool::new(false),
        })
    }

    /// Starts camera capture, requesting permission if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if camera access is denied or the permission request times out.
    pub fn start(&self) -> Result<(), CameraError> {
        ensure_camera_access()?;
        let _guard = SESSION_LIFECYCLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: The retained device remains connected and the configuration lock is released
        // after the session starts so AVFoundation cannot replace the selected format meanwhile.
        unsafe {
            self.device.lockForConfiguration().map_err(|error| {
                CameraError::Configure(error.localizedDescription().to_string())
            })?;
            if let Some(selected) = &self.selected_format {
                self.device.setActiveFormat(&selected.format);
                self.device
                    .setActiveVideoMinFrameDuration(selected.frame_duration);
            }
            self.session.startRunning();
            self.device.unlockForConfiguration();
        }
        self.running.store(true, Ordering::Release);
        Ok(())
    }

    pub fn stop(&self) {
        if !self.running.swap(false, Ordering::AcqRel) {
            return;
        }
        let _guard = SESSION_LIFECYCLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: Clearing the delegate stops callbacks before the retained session is torn down.
        unsafe {
            self.session.stopRunning();
            self.output.setSampleBufferDelegate_queue(None, None);
            self.session.beginConfiguration();
            self.session.removeOutput(&self.output);
            self.session.removeInput(&self.input);
            self.session.commitConfiguration();
        }
    }

    #[must_use]
    pub fn capture_format(&self) -> Option<CameraCaptureFormat> {
        self.selected_format
            .as_ref()
            .map(|selected| CameraCaptureFormat {
                dimensions: selected.dimensions,
                frame_rate: selected.frame_rate,
                pixel_format: selected.pixel_format,
            })
    }

    #[must_use]
    pub fn active_frame_rate_range(&self) -> Option<(f64, f64)> {
        // SAFETY: Active frame durations are readable while the retained device is running.
        let (minimum_duration, maximum_duration) = unsafe {
            (
                self.device.activeVideoMinFrameDuration().seconds(),
                self.device.activeVideoMaxFrameDuration().seconds(),
            )
        };
        if minimum_duration <= 0.0
            || maximum_duration <= 0.0
            || !minimum_duration.is_finite()
            || !maximum_duration.is_finite()
        {
            return None;
        }
        Some((1.0 / maximum_duration, 1.0 / minimum_duration))
    }
}

fn select_format(device: &AVCaptureDevice, fps: u32) -> Option<SelectedFormat> {
    const TARGET_WIDTH: i32 = 1920;
    const TARGET_HEIGHT: i32 = 1080;

    let desired_rate = f64::from(fps);
    let mut best: Option<(f64, bool, u64, SelectedFormat)> = None;
    // SAFETY: Device formats and their frame-rate ranges are immutable while the device is retained.
    let formats = unsafe { device.formats() };
    for format in &formats {
        // SAFETY: Every video device format has a video format description and immutable ranges.
        let (description, ranges) = unsafe {
            (
                format.formatDescription(),
                format.videoSupportedFrameRateRanges(),
            )
        };
        // SAFETY: The description belongs to this retained video device format.
        let (dimensions, pixel_format) = unsafe {
            (
                CMVideoFormatDescriptionGetDimensions(&description),
                description.media_sub_type(),
            )
        };
        let native_gpu_format = is_gpu_supported_format(pixel_format);
        let dimension_distance = i64::from(dimensions.width)
            .abs_diff(i64::from(TARGET_WIDTH))
            .saturating_add(i64::from(dimensions.height).abs_diff(i64::from(TARGET_HEIGHT)));
        for range in &ranges {
            // SAFETY: Frame-rate range values are immutable and valid for this format.
            let (minimum, maximum) = unsafe { (range.minFrameRate(), range.maxFrameRate()) };
            if !minimum.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
                continue;
            }
            let (rate, frame_duration) = if minimum <= desired_rate && desired_rate <= maximum {
                let timescale = i32::try_from(fps).ok()?;
                // SAFETY: The requested FPS is non-zero and this range explicitly supports it.
                (desired_rate, unsafe { CMTime::new(1, timescale) })
            } else if maximum < desired_rate {
                // SAFETY: `minFrameDuration` is the duration of this range's maximum frame rate.
                (maximum, unsafe { range.minFrameDuration() })
            } else {
                continue;
            };
            let ranked_rate = ranked_frame_rate(desired_rate, rate);
            let should_replace =
                best.as_ref()
                    .is_none_or(|(best_rate, best_native, best_distance, _)| {
                        ranked_rate > *best_rate
                            || ((ranked_rate - *best_rate).abs() < f64::EPSILON
                                && (native_gpu_format && !best_native
                                    || (native_gpu_format == *best_native
                                        && dimension_distance < *best_distance)))
                    });
            if should_replace {
                let width = u32::try_from(dimensions.width).ok()?;
                let height = u32::try_from(dimensions.height).ok()?;
                best = Some((
                    ranked_rate,
                    native_gpu_format,
                    dimension_distance,
                    SelectedFormat {
                        format: format.clone(),
                        frame_duration,
                        pixel_format,
                        dimensions: (width, height),
                        frame_rate: rate,
                    },
                ));
            }
        }
    }
    best.map(|(_, _, _, selected)| selected)
}

fn is_gpu_supported_format(format: u32) -> bool {
    format == kCVPixelFormatType_32BGRA
        || format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
}

fn ranked_frame_rate(desired: f64, actual: f64) -> f64 {
    if (desired - actual).abs() < 1.0 {
        desired
    } else {
        actual
    }
}

impl Drop for CameraCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Lists all currently available video capture devices.
///
/// # Errors
///
/// Returns an error if `AVFoundation`'s video media type constant is unavailable.
#[allow(deprecated)]
pub fn list_video_devices() -> Result<Vec<CameraDevice>, CameraError> {
    let media_type = video_media_type()?;
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    // SAFETY: Each framework constant is only accessed on a macOS version where it is available.
    let device_types = unsafe {
        let mut types = vec![AVCaptureDeviceTypeBuiltInWideAngleCamera];
        if version.majorVersion >= 13 {
            types.push(AVCaptureDeviceTypeDeskViewCamera);
        }
        if version.majorVersion >= 14 {
            types.push(AVCaptureDeviceTypeExternal);
            types.push(AVCaptureDeviceTypeContinuityCamera);
        } else {
            types.push(AVCaptureDeviceTypeExternalUnknown);
        }
        types
    };
    let device_types = NSArray::from_slice(&device_types);
    // SAFETY: The query requests video devices of the explicitly listed types at any position.
    let discovery = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &device_types,
            Some(media_type),
            AVCaptureDevicePosition::Unspecified,
        )
    };
    // SAFETY: The discovery session is retained while its current device list is read.
    let devices = unsafe { discovery.devices() };
    Ok(devices
        .iter()
        .map(|device| {
            // SAFETY: Device identity and display properties are immutable while retained.
            let (unique_id, localized_name) = unsafe {
                (
                    device.uniqueID().to_string(),
                    device.localizedName().to_string(),
                )
            };
            CameraDevice {
                device: device.clone(),
                unique_id,
                localized_name,
            }
        })
        .collect())
}

/// Returns the current camera authorization state.
///
/// # Errors
///
/// Returns an error if `AVFoundation`'s video media type constant is unavailable.
pub fn camera_authorization_status() -> Result<CameraAuthorizationStatus, CameraError> {
    // SAFETY: The static media type is AVMediaTypeVideo.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(video_media_type()?) };
    Ok(map_authorization_status(status))
}

fn map_authorization_status(status: AVAuthorizationStatus) -> CameraAuthorizationStatus {
    match status {
        AVAuthorizationStatus::NotDetermined => CameraAuthorizationStatus::NotDetermined,
        AVAuthorizationStatus::Restricted => CameraAuthorizationStatus::Restricted,
        AVAuthorizationStatus::Denied => CameraAuthorizationStatus::Denied,
        _ => CameraAuthorizationStatus::Authorized,
    }
}

/// Requests camera access and waits up to `timeout` for the user's response.
///
/// # Errors
///
/// Returns an error if the permission response does not arrive before `timeout`.
pub fn request_camera_access(timeout: Duration) -> Result<bool, CameraError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |granted: Bool| {
        let _ = sender.try_send(granted.as_bool());
    });
    // SAFETY: The escaping block owns its sender and the static media type is video.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(
            video_media_type()?,
            &completion,
        );
    }
    receiver
        .recv_timeout(timeout)
        .map_err(|_| CameraError::PermissionTimeout)
}

fn video_media_type() -> Result<&'static objc2_av_foundation::AVMediaType, CameraError> {
    // SAFETY: AVFoundation exports this process-lifetime framework constant on macOS.
    unsafe { AVMediaTypeVideo }.ok_or(CameraError::MissingVideoMediaType)
}

fn ensure_camera_access() -> Result<(), CameraError> {
    match camera_authorization_status()? {
        CameraAuthorizationStatus::Authorized => Ok(()),
        CameraAuthorizationStatus::NotDetermined => {
            if request_camera_access(Duration::from_secs(30))? {
                Ok(())
            } else {
                Err(CameraError::PermissionDenied)
            }
        }
        CameraAuthorizationStatus::Restricted | CameraAuthorizationStatus::Denied => {
            Err(CameraError::PermissionDenied)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_camera_authorization_statuses() {
        assert_eq!(
            map_authorization_status(AVAuthorizationStatus::NotDetermined),
            CameraAuthorizationStatus::NotDetermined
        );
        assert_eq!(
            map_authorization_status(AVAuthorizationStatus::Restricted),
            CameraAuthorizationStatus::Restricted
        );
        assert_eq!(
            map_authorization_status(AVAuthorizationStatus::Denied),
            CameraAuthorizationStatus::Denied
        );
        assert_eq!(
            map_authorization_status(AVAuthorizationStatus::Authorized),
            CameraAuthorizationStatus::Authorized
        );
    }

    #[test]
    fn treats_fractional_ntsc_rates_as_the_requested_rate() {
        assert!((ranked_frame_rate(60.0, 59.94) - 60.0).abs() < f64::EPSILON);
        assert!((ranked_frame_rate(60.0, 30.0) - 30.0).abs() < f64::EPSILON);
    }
}

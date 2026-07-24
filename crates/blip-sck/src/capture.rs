use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{SCStream, SCStreamDelegate, SCStreamOutput, SCStreamOutputType};

use crate::platform::initialize_core_graphics;
use crate::{CaptureError, CaptureFilter, StreamConfig, StreamConfigBuilder, VideoFrame};

type VideoCallback = Box<dyn FnMut(VideoFrame) + Send>;
type StopCallback = Box<dyn FnMut(&NSError) + Send>;

#[derive(Default)]
struct Callbacks {
    video: Option<VideoCallback>,
    stopped: Option<StopCallback>,
}

struct OutputIvars {
    callbacks: Mutex<Callbacks>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = OutputIvars]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            if output_type != SCStreamOutputType::Screen {
                return;
            }

            let Some(frame) = VideoFrame::new(sample_buffer) else {
                return;
            };
            let _ = catch_unwind(AssertUnwindSafe(|| {
                if let Ok(mut callbacks) = self.ivars().callbacks.lock()
                    && let Some(callback) = &mut callbacks.video
                {
                    callback(frame);
                }
            }));
        }
    }

    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                if let Ok(mut callbacks) = self.ivars().callbacks.lock()
                    && let Some(callback) = &mut callbacks.stopped
                {
                    callback(error);
                }
            }));
        }
    }
);

impl StreamOutput {
    fn new(callbacks: Callbacks) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            callbacks: Mutex::new(callbacks),
        });
        // SAFETY: `this` is allocated with fully initialized ivars and NSObject permits `init`.
        unsafe { msg_send![super(this), init] }
    }
}

pub struct Capturer {
    stream: Retained<SCStream>,
    _queue: DispatchRetained<DispatchQueue>,
    _output: Retained<StreamOutput>,
    timeout: Duration,
}

impl Capturer {
    /// Creates a capture builder, defaulting unspecified dimensions to the filter's pixel size.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid or the filter's pixel size is unavailable.
    pub fn builder(
        target: impl Into<CaptureFilter>,
        config: StreamConfigBuilder,
    ) -> Result<CapturerBuilder, CaptureError> {
        let filter = target.into();
        let config = config.build_for(&filter)?;
        Ok(CapturerBuilder {
            filter,
            config,
            callbacks: Callbacks::default(),
            timeout: Duration::from_secs(5),
        })
    }

    /// Starts screen capture and waits for `ScreenCaptureKit`'s completion callback.
    ///
    /// # Errors
    ///
    /// Returns an error when `ScreenCaptureKit` rejects the stream or does not
    /// complete startup before the configured timeout.
    pub fn start(&self) -> Result<(), CaptureError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            let result = completion_result(error, "failed to start capture");
            let _ = sender.try_send(result);
        });

        // SAFETY: The retained stream is configured, and the escaping block owns its sender.
        unsafe {
            self.stream
                .startCaptureWithCompletionHandler(Some(&completion));
        };
        receiver
            .recv_timeout(self.timeout)
            .map_err(|_| CaptureError::Timeout("starting capture"))?
            .map_err(CaptureError::Framework)
    }

    /// Stops screen capture and waits for `ScreenCaptureKit`'s completion callback.
    ///
    /// # Errors
    ///
    /// Returns an error when `ScreenCaptureKit` cannot stop the stream or does not
    /// complete shutdown before the configured timeout.
    pub fn stop(&self) -> Result<(), CaptureError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            let result = completion_result(error, "failed to stop capture");
            let _ = sender.try_send(result);
        });

        // SAFETY: The stream remains retained until this completion has been received or timed out.
        unsafe {
            self.stream
                .stopCaptureWithCompletionHandler(Some(&completion));
        };
        receiver
            .recv_timeout(self.timeout)
            .map_err(|_| CaptureError::Timeout("stopping capture"))?
            .map_err(CaptureError::Framework)
    }
}

pub struct CapturerBuilder {
    filter: CaptureFilter,
    config: StreamConfig,
    callbacks: Callbacks,
    timeout: Duration,
}

impl CapturerBuilder {
    #[must_use]
    pub fn with_video_frame_callback(
        mut self,
        callback: impl FnMut(VideoFrame) + Send + 'static,
    ) -> Self {
        self.callbacks.video = Some(Box::new(callback));
        self
    }

    #[must_use]
    pub fn with_stop_callback(mut self, callback: impl FnMut(&NSError) + Send + 'static) -> Self {
        self.callbacks.stopped = Some(Box::new(callback));
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builds a configured `ScreenCaptureKit` stream.
    ///
    /// # Errors
    ///
    /// Returns an error if `ScreenCaptureKit` rejects the stream output.
    pub fn build(self) -> Result<Capturer, CaptureError> {
        initialize_core_graphics();
        let output = StreamOutput::new(self.callbacks);
        let delegate = ProtocolObject::<dyn SCStreamDelegate>::from_ref(&*output);
        // SAFETY: The filter, configuration, and delegate remain retained for the stream lifetime.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                self.filter.as_raw(),
                self.config.as_raw(),
                Some(delegate),
            )
        };
        let queue = DispatchQueue::new("dev.brendonovich.blip.capture", None);
        let stream_output = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*output);
        // SAFETY: Capturer retains the serial queue and output for the stream lifetime.
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    stream_output,
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|error| {
                    CaptureError::Framework(error.localizedDescription().to_string())
                })?;
        }

        Ok(Capturer {
            stream,
            _queue: queue,
            _output: output,
            timeout: self.timeout,
        })
    }
}

fn completion_result(error: *mut NSError, fallback: &str) -> Result<(), String> {
    if error.is_null() {
        Ok(())
    } else {
        // SAFETY: Callers pass the non-null NSError pointer received by an active callback.
        Err(unsafe { error.as_ref() }.map_or_else(
            || fallback.to_owned(),
            |error| error.localizedDescription().to_string(),
        ))
    }
}

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::NSError;
use objc2_screen_capture_kit::SCShareableContent;

use crate::platform::initialize_core_graphics;
use crate::{Application, CaptureError, Display, Window};

pub struct ShareableContent(Retained<SCShareableContent>);

// SAFETY: ScreenCaptureKit content objects are immutable and documented for use across queues.
unsafe impl Send for ShareableContent {}

impl ShareableContent {
    /// Loads the displays, windows, and applications currently available for capture.
    ///
    /// # Errors
    ///
    /// Returns an error if `ScreenCaptureKit` fails or does not respond before `timeout`.
    pub fn current(timeout: Duration) -> Result<Self, CaptureError> {
        initialize_core_graphics();
        let (sender, receiver) = mpsc::sync_channel(1);
        let completion = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                // SAFETY: ScreenCaptureKit passes either null or a valid object for this callback.
                let result = unsafe { Retained::retain(content) }
                    .map(Self)
                    .ok_or_else(|| error_message(error, "failed to load shareable content"));
                let _ = sender.try_send(result);
            },
        );

        // SAFETY: The escaping block owns its sender and ScreenCaptureKit copies it as required.
        unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&completion) };
        receiver
            .recv_timeout(timeout)
            .map_err(|_| CaptureError::Timeout("loading shareable content"))?
            .map_err(CaptureError::Framework)
    }

    #[must_use]
    pub fn displays(&self) -> Vec<Display> {
        // SAFETY: `self` retains a valid SCShareableContent object.
        unsafe { self.0.displays() }
            .into_iter()
            .map(Display::from_raw)
            .collect()
    }

    #[must_use]
    pub fn windows(&self) -> Vec<Window> {
        let displays = Arc::<[Display]>::from(self.displays());
        // SAFETY: `self` retains a valid SCShareableContent object.
        unsafe { self.0.windows() }
            .into_iter()
            .map(|window| Window::from_raw(window, Arc::clone(&displays)))
            .collect()
    }

    #[must_use]
    pub fn application_windows(&self) -> Vec<Window> {
        self.windows()
            .into_iter()
            .filter(Window::is_application_window)
            .collect()
    }

    #[must_use]
    pub fn applications(&self) -> Vec<Application> {
        // SAFETY: `self` retains a valid SCShareableContent object.
        unsafe { self.0.applications() }
            .into_iter()
            .map(Application::from_raw)
            .collect()
    }

    #[must_use]
    pub fn main_display(&self) -> Option<Display> {
        let main_display_id = objc2_core_graphics::CGMainDisplayID();
        self.displays()
            .into_iter()
            .find(|display| display.id() == main_display_id)
    }
}

fn error_message(error: *mut NSError, fallback: &str) -> String {
    // SAFETY: Callers pass the nullable NSError pointer received by an active callback.
    unsafe { error.as_ref() }.map_or_else(
        || fallback.to_owned(),
        |error| error.localizedDescription().to_string(),
    )
}

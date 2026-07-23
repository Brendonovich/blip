use std::sync::Arc;

use objc2::rc::Retained;
use objc2_core_foundation::{CFArray, CFRetained};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayCopyAllDisplayModes, CGDisplayCopyDisplayMode, CGDisplayIsActive,
    CGDisplayIsBuiltin, CGDisplayIsMain, CGDisplayIsOnline, CGDisplayMode, CGDisplayPixelsHigh,
    CGDisplayPixelsWide, CGDisplayRotation, CGRectIntersectsRect, CGWindowID,
};
use objc2_screen_capture_kit::{SCDisplay, SCRunningApplication, SCWindow};

#[derive(Clone)]
pub struct Display(pub(crate) Retained<SCDisplay>);

impl Display {
    pub(crate) fn from_raw(display: Retained<SCDisplay>) -> Self {
        Self(display)
    }

    #[must_use]
    pub fn id(&self) -> CGDirectDisplayID {
        // SAFETY: `self` retains a valid SCDisplay object.
        unsafe { self.0.displayID() }
    }

    #[must_use]
    pub fn physical_width(&self) -> usize {
        CGDisplayPixelsWide(self.id())
    }

    #[must_use]
    pub fn physical_height(&self) -> usize {
        CGDisplayPixelsHigh(self.id())
    }

    #[must_use]
    pub fn logical_size(&self) -> (isize, isize) {
        // SAFETY: `self` retains a valid SCDisplay object.
        unsafe { (self.0.width(), self.0.height()) }
    }

    #[must_use]
    pub fn frame(&self) -> (f64, f64, f64, f64) {
        // SAFETY: `self` retains a valid SCDisplay object.
        let frame = unsafe { self.0.frame() };
        (
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        )
    }

    #[must_use]
    pub fn is_main(&self) -> bool {
        CGDisplayIsMain(self.id())
    }

    #[must_use]
    pub fn is_builtin(&self) -> bool {
        CGDisplayIsBuiltin(self.id())
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        CGDisplayIsActive(self.id())
    }

    #[must_use]
    pub fn is_online(&self) -> bool {
        CGDisplayIsOnline(self.id())
    }

    #[must_use]
    pub fn rotation_degrees(&self) -> f64 {
        CGDisplayRotation(self.id())
    }

    #[must_use]
    pub fn current_mode(&self) -> Option<DisplayMode> {
        CGDisplayCopyDisplayMode(self.id()).map(|mode| DisplayMode::from_raw(&mode))
    }

    #[must_use]
    pub fn available_modes(&self) -> Vec<DisplayMode> {
        // SAFETY: Passing no options returns a CFArray containing only CGDisplayMode objects.
        let Some(modes) = (unsafe { CGDisplayCopyAllDisplayModes(self.id(), None) }) else {
            return Vec::new();
        };
        // SAFETY: CoreGraphics documents every array element as a CGDisplayMode.
        let modes = unsafe { CFRetained::cast_unchecked::<CFArray<CGDisplayMode>>(modes) };
        modes
            .iter()
            .map(|mode| DisplayMode::from_raw(&mode))
            .collect()
    }

    #[must_use]
    pub fn as_raw(&self) -> &SCDisplay {
        &self.0
    }
}

#[derive(Clone)]
pub struct Window {
    pub(crate) raw: Retained<SCWindow>,
    displays: Arc<[Display]>,
}

impl Window {
    pub(crate) fn from_raw(window: Retained<SCWindow>, displays: Arc<[Display]>) -> Self {
        Self {
            raw: window,
            displays,
        }
    }

    #[must_use]
    pub fn id(&self) -> CGWindowID {
        // SAFETY: `self` retains a valid SCWindow object.
        unsafe { self.raw.windowID() }
    }

    #[must_use]
    pub fn title(&self) -> Option<String> {
        // SAFETY: `self` retains a valid SCWindow object.
        unsafe { self.raw.title() }.map(|title| title.to_string())
    }

    #[must_use]
    pub fn application(&self) -> Option<Application> {
        // SAFETY: `self` retains a valid SCWindow object.
        unsafe { self.raw.owningApplication() }.map(Application::from_raw)
    }

    #[must_use]
    pub fn is_on_screen(&self) -> bool {
        // SAFETY: `self` retains a valid SCWindow object.
        unsafe { self.raw.isOnScreen() }
    }

    #[must_use]
    pub fn layer(&self) -> isize {
        // SAFETY: `self` retains a valid SCWindow object.
        unsafe { self.raw.windowLayer() }
    }

    #[must_use]
    pub fn logical_size(&self) -> (f64, f64) {
        // SAFETY: `self` retains a valid SCWindow object.
        let frame = unsafe { self.raw.frame() };
        (frame.size.width, frame.size.height)
    }

    #[must_use]
    pub fn frame(&self) -> (f64, f64, f64, f64) {
        // SAFETY: `self` retains a valid SCWindow object.
        let frame = unsafe { self.raw.frame() };
        (
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        )
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        // SAFETY: `self` retains a valid SCWindow object.
        unsafe { self.raw.isActive() }
    }

    #[must_use]
    pub fn display(&self) -> Option<Display> {
        // SAFETY: `self` retains a valid SCWindow object.
        let window_frame = unsafe { self.raw.frame() };
        self.displays
            .iter()
            .find(|display| {
                // SAFETY: Each display is retained from the same shareable-content snapshot.
                let display_frame = unsafe { display.0.frame() };
                CGRectIntersectsRect(window_frame, display_frame)
            })
            .cloned()
    }

    #[must_use]
    pub fn is_application_window(&self) -> bool {
        self.is_on_screen() && self.layer() == 0 && self.application().is_some()
    }

    #[must_use]
    pub fn as_raw(&self) -> &SCWindow {
        &self.raw
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayMode {
    logical_width: usize,
    logical_height: usize,
    pixel_width: usize,
    pixel_height: usize,
    refresh_rate: f64,
    io_id: i32,
    usable_for_desktop: bool,
}

impl DisplayMode {
    fn from_raw(mode: &CGDisplayMode) -> Self {
        Self {
            logical_width: CGDisplayMode::width(Some(mode)),
            logical_height: CGDisplayMode::height(Some(mode)),
            pixel_width: CGDisplayMode::pixel_width(Some(mode)),
            pixel_height: CGDisplayMode::pixel_height(Some(mode)),
            refresh_rate: CGDisplayMode::refresh_rate(Some(mode)),
            io_id: CGDisplayMode::io_display_mode_id(Some(mode)),
            usable_for_desktop: CGDisplayMode::is_usable_for_desktop_gui(Some(mode)),
        }
    }

    #[must_use]
    pub fn logical_size(&self) -> (usize, usize) {
        (self.logical_width, self.logical_height)
    }

    #[must_use]
    pub fn pixel_size(&self) -> (usize, usize) {
        (self.pixel_width, self.pixel_height)
    }

    #[must_use]
    pub fn refresh_rate(&self) -> f64 {
        self.refresh_rate
    }

    #[must_use]
    pub fn io_id(&self) -> i32 {
        self.io_id
    }

    #[must_use]
    pub fn is_usable_for_desktop(&self) -> bool {
        self.usable_for_desktop
    }
}

#[derive(Clone)]
pub struct Application(pub(crate) Retained<SCRunningApplication>);

impl Application {
    pub(crate) fn from_raw(application: Retained<SCRunningApplication>) -> Self {
        Self(application)
    }

    #[must_use]
    pub fn name(&self) -> String {
        // SAFETY: `self` retains a valid SCRunningApplication object.
        unsafe { self.0.applicationName() }.to_string()
    }

    #[must_use]
    pub fn bundle_identifier(&self) -> String {
        // SAFETY: `self` retains a valid SCRunningApplication object.
        unsafe { self.0.bundleIdentifier() }.to_string()
    }

    #[must_use]
    pub fn process_id(&self) -> i32 {
        // SAFETY: `self` retains a valid SCRunningApplication object.
        unsafe { self.0.processID() }
    }

    #[must_use]
    pub fn as_raw(&self) -> &SCRunningApplication {
        &self.0
    }
}

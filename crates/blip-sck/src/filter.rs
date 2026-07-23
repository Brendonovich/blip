use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::NSArray;
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCShareableContent, SCWindow,
};

use crate::{Application, Display, Window};

pub struct CaptureFilter(Retained<SCContentFilter>);

impl CaptureFilter {
    #[must_use]
    pub fn display(display: Display) -> DisplayFilterBuilder {
        DisplayFilterBuilder {
            display,
            excluded_windows: Vec::new(),
            include_menu_bar: None,
        }
    }

    #[must_use]
    pub fn window(window: &Window) -> Self {
        // SAFETY: The retained window remains valid for the initializer call.
        let filter = unsafe {
            SCContentFilter::initWithDesktopIndependentWindow(
                SCContentFilter::alloc(),
                window.as_raw(),
            )
        };
        Self(filter)
    }

    #[must_use]
    pub fn applications(
        display: Display,
        applications: impl IntoIterator<Item = Application>,
    ) -> ApplicationFilterBuilder {
        ApplicationFilterBuilder {
            display,
            applications: applications.into_iter().collect(),
            excepted_windows: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_raw(filter: Retained<SCContentFilter>) -> Self {
        Self(filter)
    }

    #[must_use]
    pub fn as_raw(&self) -> &SCContentFilter {
        &self.0
    }

    /// Returns the pixel dimensions that should be requested when capturing this filter.
    #[must_use]
    pub fn capture_size(&self) -> Option<(usize, usize)> {
        // SAFETY: The retained filter is valid, and ScreenCaptureKit returns immutable
        // geometry describing that filter's current content.
        let info = unsafe { SCShareableContent::infoForFilter(self.as_raw()) };
        // SAFETY: `info` is retained for both property reads.
        let content_rect = unsafe { info.contentRect() };
        // SAFETY: `info` is retained for this property read.
        let scale = f64::from(unsafe { info.pointPixelScale() });
        dimensions_from_f64(
            content_rect.size.width * scale,
            content_rect.size.height * scale,
        )
    }

    pub(crate) fn point_pixel_scale(&self) -> f64 {
        // SAFETY: The filter remains valid for the immutable geometry query.
        let info = unsafe { SCShareableContent::infoForFilter(self.as_raw()) };
        // SAFETY: `info` is retained for this property read.
        f64::from(unsafe { info.pointPixelScale() })
    }
}

fn dimensions_from_f64(width: f64, height: f64) -> Option<(usize, usize)> {
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

impl From<Display> for CaptureFilter {
    fn from(display: Display) -> Self {
        Self::display(display).build()
    }
}

impl From<Window> for CaptureFilter {
    fn from(window: Window) -> Self {
        Self::window(&window)
    }
}

impl From<&Window> for CaptureFilter {
    fn from(window: &Window) -> Self {
        Self::window(window)
    }
}

pub struct DisplayFilterBuilder {
    display: Display,
    excluded_windows: Vec<Window>,
    include_menu_bar: Option<bool>,
}

impl DisplayFilterBuilder {
    #[must_use]
    pub fn excluding_windows(mut self, windows: impl IntoIterator<Item = Window>) -> Self {
        self.excluded_windows.extend(windows);
        self
    }

    #[must_use]
    pub fn include_menu_bar(mut self, include_menu_bar: bool) -> Self {
        self.include_menu_bar = Some(include_menu_bar);
        self
    }

    #[must_use]
    pub fn build(self) -> CaptureFilter {
        let windows: Vec<Retained<SCWindow>> = self
            .excluded_windows
            .into_iter()
            .map(|window| window.raw)
            .collect();
        let windows = NSArray::from_retained_slice(&windows);
        // SAFETY: The retained display and exclusion array remain valid for the initializer call.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                self.display.as_raw(),
                &windows,
            )
        };
        if let Some(include_menu_bar) = self.include_menu_bar {
            // SAFETY: `filter` is a retained and initialized SCContentFilter.
            unsafe { filter.setIncludeMenuBar(include_menu_bar) };
        }
        CaptureFilter(filter)
    }
}

pub struct ApplicationFilterBuilder {
    display: Display,
    applications: Vec<Application>,
    excepted_windows: Vec<Window>,
}

impl ApplicationFilterBuilder {
    #[must_use]
    pub fn excepting_windows(mut self, windows: impl IntoIterator<Item = Window>) -> Self {
        self.excepted_windows.extend(windows);
        self
    }

    #[must_use]
    pub fn build(self) -> CaptureFilter {
        let applications: Vec<Retained<SCRunningApplication>> = self
            .applications
            .into_iter()
            .map(|application| application.0)
            .collect();
        let windows: Vec<Retained<SCWindow>> = self
            .excepted_windows
            .into_iter()
            .map(|window| window.raw)
            .collect();
        let applications = NSArray::from_retained_slice(&applications);
        let windows = NSArray::from_retained_slice(&windows);
        // SAFETY: All retained targets and arrays remain valid for the initializer call.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                self.display.as_raw(),
                &applications,
                &windows,
            )
        };
        CaptureFilter(filter)
    }
}

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

#[must_use]
pub fn has_permission() -> bool {
    CGPreflightScreenCaptureAccess()
}

#[must_use]
pub fn request_permission() -> bool {
    CGRequestScreenCaptureAccess()
}

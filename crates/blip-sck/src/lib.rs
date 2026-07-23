#![cfg(target_os = "macos")]

use std::fmt;

mod capture;
mod config;
mod content;
mod filter;
mod permission;
mod platform;
mod target;
mod video_frame;

pub use capture::{Capturer, CapturerBuilder};
pub use config::{PixelFormat, StreamConfig, StreamConfigBuilder};
pub use content::ShareableContent;
pub use filter::{ApplicationFilterBuilder, CaptureFilter, DisplayFilterBuilder};
pub use permission::{has_permission, request_permission};
pub use target::{Application, Display, DisplayMode, Window};
pub use video_frame::{FrameGeometry, FrameRect, VideoFrame};

#[derive(Debug)]
pub enum CaptureError {
    PermissionDenied,
    NoDisplay,
    Timeout(&'static str),
    InvalidConfiguration(String),
    Framework(String),
    InvalidFrame(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied => write!(
                f,
                "screen recording permission is required; grant it in System Settings and retry"
            ),
            Self::NoDisplay => write!(f, "the main display is not available to ScreenCaptureKit"),
            Self::Timeout(operation) => write!(f, "timed out while {operation}"),
            Self::InvalidConfiguration(message)
            | Self::Framework(message)
            | Self::InvalidFrame(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CaptureError {}

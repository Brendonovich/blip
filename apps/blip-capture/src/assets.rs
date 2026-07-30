use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

pub(crate) const PLAY: &str = "icons/play.svg";
pub(crate) const PAUSE: &str = "icons/pause.svg";
pub(crate) const PLAYBACK_BACK: &str = "icons/playback-back.svg";
pub(crate) const PLAYBACK_FORWARD: &str = "icons/playback-forward.svg";
pub(crate) const CORNER_CIRCLE: &str = "icons/corner-circle.svg";
pub(crate) const CORNER_SQUIRCLE: &str = "icons/corner-squircle.svg";
pub(crate) const SHAPE_FRAME: &str = "icons/shape-frame.svg";
pub(crate) const CLOSE: &str = "icons/close.svg";
pub(crate) const CHECK: &str = "icons/check.svg";
pub(crate) const CHEVRON_DOWN: &str = "icons/chevron-down.svg";

pub(crate) struct CaptureAssets;

impl AssetSource for CaptureAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            PLAY => Some(include_bytes!("../assets/icons/play.svg")),
            PAUSE => Some(include_bytes!("../assets/icons/pause.svg")),
            PLAYBACK_BACK => Some(include_bytes!("../assets/icons/playback-back.svg")),
            PLAYBACK_FORWARD => Some(include_bytes!("../assets/icons/playback-forward.svg")),
            CORNER_CIRCLE => Some(include_bytes!("../assets/icons/corner-circle.svg")),
            CORNER_SQUIRCLE => Some(include_bytes!("../assets/icons/corner-squircle.svg")),
            SHAPE_FRAME => Some(include_bytes!("../assets/icons/shape-frame.svg")),
            CLOSE => Some(include_bytes!("../assets/icons/close.svg")),
            CHECK => Some(include_bytes!("../assets/icons/check.svg")),
            CHEVRON_DOWN => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok([
            PLAY,
            PAUSE,
            PLAYBACK_BACK,
            PLAYBACK_FORWARD,
            CORNER_CIRCLE,
            CORNER_SQUIRCLE,
            SHAPE_FRAME,
            CLOSE,
            CHECK,
            CHEVRON_DOWN,
        ]
        .into_iter()
        .filter(|asset| asset.starts_with(path))
        .map(SharedString::from)
        .collect())
    }
}

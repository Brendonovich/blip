use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

pub(crate) const PLAY: &str = "icons/play.svg";
pub(crate) const PAUSE: &str = "icons/pause.svg";
pub(crate) const PLAYBACK_BACK: &str = "icons/playback-back.svg";
pub(crate) const PLAYBACK_FORWARD: &str = "icons/playback-forward.svg";

pub(crate) struct CaptureAssets;

impl AssetSource for CaptureAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            PLAY => Some(include_bytes!("../assets/icons/play.svg")),
            PAUSE => Some(include_bytes!("../assets/icons/pause.svg")),
            PLAYBACK_BACK => Some(include_bytes!("../assets/icons/playback-back.svg")),
            PLAYBACK_FORWARD => Some(include_bytes!("../assets/icons/playback-forward.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok([PLAY, PAUSE, PLAYBACK_BACK, PLAYBACK_FORWARD]
            .into_iter()
            .filter(|asset| asset.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

pub(crate) const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub(crate) const GRIP_VERTICAL: &str = "icons/grip-vertical.svg";

pub(crate) struct StudioAssets;

impl AssetSource for StudioAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            CHEVRON_DOWN => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            GRIP_VERTICAL => Some(include_bytes!("../assets/icons/grip-vertical.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok([CHEVRON_DOWN, GRIP_VERTICAL]
            .into_iter()
            .filter(|asset| asset.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

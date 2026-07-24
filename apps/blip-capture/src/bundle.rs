use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

pub(crate) const MANIFEST_FILE: &str = "manifest.json";
const BUNDLE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BlipBundle {
    pub(crate) version: u32,
    pub(crate) created_at: DateTime<Local>,
    pub(crate) inputs: Vec<BundleInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BundleInput {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) media: PathBuf,
}

impl BlipBundle {
    pub(crate) fn create(path: &Path) -> Result<Self, String> {
        fs::create_dir(path).map_err(|error| format!("failed to create Blip Bundle: {error}"))?;
        let inputs_dir = path.join("inputs");
        if let Err(error) = fs::create_dir(&inputs_dir) {
            fs::remove_dir_all(path).ok();
            return Err(format!("failed to create bundle inputs folder: {error}"));
        }
        let bundle = Self {
            version: BUNDLE_VERSION,
            created_at: Local::now(),
            inputs: vec![BundleInput {
                id: "screen".into(),
                name: "Screen".into(),
                media: PathBuf::from("inputs/screen.mp4"),
            }],
        };
        if let Err(error) = bundle.save(path) {
            fs::remove_dir_all(path).ok();
            return Err(error);
        }
        Ok(bundle)
    }

    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let contents = fs::read_to_string(path.join(MANIFEST_FILE))
            .map_err(|error| format!("failed to read Blip Bundle: {error}"))?;
        serde_json::from_str(&contents)
            .map_err(|error| format!("failed to decode Blip Bundle: {error}"))
    }

    pub(crate) fn media_path(&self, bundle_path: &Path) -> Result<PathBuf, String> {
        self.inputs
            .first()
            .map(|input| bundle_path.join(&input.media))
            .ok_or_else(|| "Blip Bundle has no recording inputs".into())
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| format!("failed to encode Blip Bundle: {error}"))?;
        fs::write(path.join(MANIFEST_FILE), contents)
            .map_err(|error| format!("failed to save Blip Bundle: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::BlipBundle;

    #[test]
    fn creates_a_manifest_and_separate_screen_input() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-bundle-test-{suffix}.blip"));
        let result = BlipBundle::create(&path);
        assert!(result.is_ok(), "bundle should be created");
        let Some(bundle) = result.ok() else {
            return;
        };

        assert!(path.join("manifest.json").is_file());
        assert_eq!(
            bundle.media_path(&path).ok(),
            Some(path.join("inputs/screen.mp4"))
        );
        assert_eq!(
            BlipBundle::load(&path)
                .ok()
                .map(|bundle| bundle.inputs.len()),
            Some(1)
        );

        std::fs::remove_dir_all(path).ok();
    }
}

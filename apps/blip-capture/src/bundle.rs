use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

pub(crate) const MANIFEST_FILE: &str = "manifest.json";
pub(crate) const PROJECT_CONFIG_FILE: &str = "project-config.json";
const BUNDLE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BlipBundle {
    pub(crate) version: u32,
    pub(crate) created_at: DateTime<Local>,
    pub(crate) inputs: Vec<BundleInput>,
    #[serde(default, skip_serializing)]
    pub(crate) zoom_segments: Vec<ZoomSegment>,
    #[serde(default, skip_serializing)]
    pub(crate) video_segments: Option<Vec<VideoSegment>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProjectConfig {
    #[serde(default)]
    zoom_segments: Vec<ZoomSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    video_segments: Option<Vec<VideoSegment>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BundleInput {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) media: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct VideoSegment {
    pub(crate) id: u64,
    pub(crate) source_start_secs: f64,
    pub(crate) source_end_secs: f64,
}

impl VideoSegment {
    pub(crate) fn duration_secs(&self) -> f64 {
        (self.source_end_secs - self.source_start_secs).max(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ZoomTransitionSpeed {
    Slow,
    Medium,
    Fast,
}

impl ZoomTransitionSpeed {
    pub(crate) const fn duration_secs(self) -> f64 {
        match self {
            Self::Slow => 0.75,
            Self::Medium => 0.5,
            Self::Fast => 0.25,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ZoomSegment {
    pub(crate) id: u64,
    pub(crate) start_secs: f64,
    pub(crate) end_secs: f64,
    pub(crate) target: [f32; 2],
    pub(crate) amount: f32,
    pub(crate) transition: ZoomTransitionSpeed,
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
            zoom_segments: Vec::new(),
            video_segments: None,
        };
        if let Err(error) = bundle.save_manifest(path) {
            fs::remove_dir_all(path).ok();
            return Err(error);
        }
        Ok(bundle)
    }

    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let contents = fs::read_to_string(path.join(MANIFEST_FILE))
            .map_err(|error| format!("failed to read Blip Bundle: {error}"))?;
        let mut bundle: Self = serde_json::from_str(&contents)
            .map_err(|error| format!("failed to decode Blip Bundle: {error}"))?;
        match fs::read_to_string(path.join(PROJECT_CONFIG_FILE)) {
            Ok(contents) => {
                let config: ProjectConfig = serde_json::from_str(&contents)
                    .map_err(|error| format!("failed to decode project config: {error}"))?;
                bundle.zoom_segments = config.zoom_segments;
                bundle.video_segments = config.video_segments;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("failed to read project config: {error}")),
        }
        Ok(bundle)
    }

    pub(crate) fn media_path(&self, bundle_path: &Path) -> Result<PathBuf, String> {
        self.inputs
            .first()
            .map(|input| bundle_path.join(&input.media))
            .ok_or_else(|| "Blip Bundle has no recording inputs".into())
    }

    fn save_manifest(&self, path: &Path) -> Result<(), String> {
        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| format!("failed to encode Blip Bundle: {error}"))?;
        fs::write(path.join(MANIFEST_FILE), contents)
            .map_err(|error| format!("failed to save Blip Bundle: {error}"))
    }

    pub(crate) fn save_project_config(&self, path: &Path) -> Result<(), String> {
        let config = ProjectConfig {
            zoom_segments: self.zoom_segments.clone(),
            video_segments: self.video_segments.clone(),
        };
        let contents = serde_json::to_string_pretty(&config)
            .map_err(|error| format!("failed to encode project config: {error}"))?;
        fs::write(path.join(PROJECT_CONFIG_FILE), contents)
            .map_err(|error| format!("failed to save project config: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{BlipBundle, VideoSegment};

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
        assert!(!path.join("project-config.json").exists());
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
        assert!(bundle.zoom_segments.is_empty());
        assert!(bundle.video_segments.is_none());

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn loads_manifests_without_zoom_segments() {
        let manifest = r#"{
            "version": 1,
            "created_at": "2026-07-28T12:00:00-07:00",
            "inputs": []
        }"#;
        let bundle: BlipBundle = serde_json::from_str(manifest).expect("decode legacy manifest");
        assert!(bundle.zoom_segments.is_empty());
        assert!(bundle.video_segments.is_none());
    }

    #[test]
    fn preserves_an_explicitly_empty_video_timeline() {
        let manifest = r#"{
            "version": 1,
            "created_at": "2026-07-28T12:00:00-07:00",
            "inputs": [],
            "video_segments": []
        }"#;
        let bundle: BlipBundle = serde_json::from_str(manifest).expect("decode edited manifest");
        assert_eq!(
            bundle
                .video_segments
                .as_deref()
                .map(|segments| segments.len()),
            Some(0)
        );
    }

    #[test]
    fn saves_edits_without_changing_the_manifest() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-config-test-{suffix}.blip"));
        let mut bundle = BlipBundle::create(&path).expect("create bundle");
        let manifest = std::fs::read(path.join("manifest.json")).expect("read manifest");
        bundle.video_segments = Some(vec![VideoSegment {
            id: 1,
            source_start_secs: 1.0,
            source_end_secs: 2.0,
        }]);

        bundle
            .save_project_config(&path)
            .expect("save project config");

        assert_eq!(
            std::fs::read(path.join("manifest.json")).expect("read unchanged manifest"),
            manifest
        );
        assert!(path.join("project-config.json").is_file());
        assert_eq!(
            BlipBundle::load(&path)
                .expect("load bundle")
                .video_segments
                .as_deref()
                .map(|segments| segments.len()),
            Some(1)
        );

        std::fs::remove_dir_all(path).ok();
    }
}

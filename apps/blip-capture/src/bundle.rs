use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

pub(crate) const MANIFEST_FILE: &str = "manifest.json";
pub(crate) const PROJECT_CONFIG_FILE: &str = "project-config.json";
const BUNDLE_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BlipBundle {
    pub(crate) version: u32,
    pub(crate) created_at: DateTime<Local>,
    pub(crate) inputs: Vec<BundleInput>,
    #[serde(default, skip_serializing)]
    pub(crate) zoom_segments: Vec<ZoomSegment>,
    #[serde(default, skip_serializing)]
    pub(crate) video_segments: Option<Vec<VideoSegment>>,
    #[serde(default, skip_serializing)]
    pub(crate) video_segment_resize_mode: VideoSegmentResizeMode,
    #[serde(default, skip_serializing)]
    pub(crate) camera_layout: CameraLayout,
    #[serde(default, skip_serializing)]
    pub(crate) output_aspect_ratio: OutputAspectRatio,
    #[serde(default, skip_serializing)]
    pub(crate) screen_crop: Option<ScreenCrop>,
    #[serde(default, skip_serializing)]
    pub(crate) appearance: ProjectAppearance,
    #[serde(default, skip_serializing)]
    pub(crate) export_settings: ExportSettings,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProjectConfig {
    #[serde(default)]
    zoom_segments: Vec<ZoomSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    video_segments: Option<Vec<VideoSegment>>,
    #[serde(default)]
    video_segment_resize_mode: VideoSegmentResizeMode,
    #[serde(default)]
    camera_layout: CameraLayout,
    #[serde(default)]
    output_aspect_ratio: OutputAspectRatio,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    screen_crop: Option<ScreenCrop>,
    #[serde(default)]
    appearance: ProjectAppearance,
    #[serde(default)]
    export_settings: ExportSettings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackgroundType {
    Color,
    Image,
    #[default]
    Gradient,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectAppearance {
    #[serde(default)]
    pub(crate) background_type: BackgroundType,
    #[serde(default = "default_background_image")]
    pub(crate) background_image: String,
    #[serde(default = "default_background_padding")]
    pub(crate) padding: f32,
    #[serde(default = "default_border_radius")]
    pub(crate) border_radius: f32,
    #[serde(default = "default_shadow")]
    pub(crate) shadow: f32,
}

fn default_background_image() -> String {
    "tahoe-dusk.jpg".into()
}

const fn default_background_padding() -> f32 {
    8.0
}

const fn default_border_radius() -> f32 {
    8.0
}

const fn default_shadow() -> f32 {
    20.0
}

impl Default for ProjectAppearance {
    fn default() -> Self {
        Self {
            background_type: BackgroundType::default(),
            background_image: default_background_image(),
            padding: default_background_padding(),
            border_radius: default_border_radius(),
            shadow: default_shadow(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportFormat {
    #[default]
    Mp4,
    Mov,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportResolution {
    #[default]
    P1080,
    P720,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExportSettings {
    #[serde(default)]
    pub(crate) format: ExportFormat,
    #[serde(default)]
    pub(crate) resolution: ExportResolution,
    #[serde(default = "default_export_fps")]
    pub(crate) fps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) destination: Option<PathBuf>,
}

const fn default_export_fps() -> u32 {
    30
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: ExportFormat::default(),
            resolution: ExportResolution::default(),
            fps: default_export_fps(),
            destination: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScreenCrop {
    pub(crate) position: [f32; 2],
    pub(crate) size: [f32; 2],
}

impl ScreenCrop {
    pub(crate) const FULL: Self = Self {
        position: [0.0, 0.0],
        size: [1.0, 1.0],
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputAspectRatio {
    Auto,
    #[default]
    Wide,
    Vertical,
    Square,
    Classic,
    Tall,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BundleInput {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) media: PathBuf,
    #[serde(default)]
    pub(crate) kind: BundleInputKind,
    #[serde(default)]
    pub(crate) start_offset_secs: f64,
    #[serde(default = "default_audio_gain")]
    pub(crate) gain: f32,
}

const fn default_audio_gain() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BundleInputKind {
    #[default]
    Video,
    Audio,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct VideoSegment {
    pub(crate) id: u64,
    pub(crate) source_start_secs: f64,
    pub(crate) source_end_secs: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VideoSegmentResizeMode {
    #[default]
    Ghost,
    Live,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CameraPosition {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    Center,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    #[default]
    BottomRight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CameraCrop {
    Circle,
    Squircle,
    #[default]
    Squirectangle,
}

impl CameraCrop {
    pub(crate) const fn from_atomic(value: u8) -> Self {
        match value {
            0 => Self::Circle,
            1 => Self::Squircle,
            _ => Self::Squirectangle,
        }
    }

    pub(crate) const fn atomic_value(self) -> u8 {
        match self {
            Self::Circle => 0,
            Self::Squircle => 1,
            Self::Squirectangle => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CameraLayout {
    pub(crate) size: f32,
    pub(crate) position: CameraPosition,
    pub(crate) edge_padding: f32,
    pub(crate) zoom_size_reduction: f32,
    #[serde(default = "default_camera_shadow")]
    pub(crate) shadow: f32,
    #[serde(default)]
    pub(crate) crop: CameraCrop,
}

const fn default_camera_shadow() -> f32 {
    20.0
}

impl Default for CameraLayout {
    fn default() -> Self {
        Self {
            size: 28.0,
            position: CameraPosition::BottomRight,
            edge_padding: 3.0,
            zoom_size_reduction: 15.0,
            shadow: 20.0,
            crop: CameraCrop::Squirectangle,
        }
    }
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
    pub(crate) fn create(
        path: &Path,
        include_camera: bool,
        include_system_audio: bool,
    ) -> Result<Self, String> {
        fs::create_dir(path).map_err(|error| format!("failed to create Blip Bundle: {error}"))?;
        let inputs_dir = path.join("inputs");
        if let Err(error) = fs::create_dir(&inputs_dir) {
            fs::remove_dir_all(path).ok();
            return Err(format!("failed to create bundle inputs folder: {error}"));
        }
        let mut inputs = vec![BundleInput {
            id: "screen".into(),
            name: "Screen".into(),
            media: PathBuf::from("inputs/screen.mp4"),
            kind: BundleInputKind::Video,
            start_offset_secs: 0.0,
            gain: 1.0,
        }];
        if include_camera {
            inputs.push(BundleInput {
                id: "camera".into(),
                name: "Camera".into(),
                media: PathBuf::from("inputs/camera.mp4"),
                kind: BundleInputKind::Video,
                start_offset_secs: 0.0,
                gain: 1.0,
            });
        }
        if include_system_audio {
            inputs.push(BundleInput {
                id: "system_audio".into(),
                name: "System Audio".into(),
                media: PathBuf::from("inputs/system-audio.m4a"),
                kind: BundleInputKind::Audio,
                start_offset_secs: 0.0,
                gain: 1.0,
            });
        }
        let bundle = Self {
            version: BUNDLE_VERSION,
            created_at: Local::now(),
            inputs,
            zoom_segments: Vec::new(),
            video_segments: None,
            video_segment_resize_mode: VideoSegmentResizeMode::default(),
            camera_layout: CameraLayout::default(),
            output_aspect_ratio: OutputAspectRatio::default(),
            screen_crop: None,
            appearance: ProjectAppearance::default(),
            export_settings: ExportSettings::default(),
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
                bundle.video_segment_resize_mode = config.video_segment_resize_mode;
                bundle.camera_layout = config.camera_layout;
                bundle.output_aspect_ratio = config.output_aspect_ratio;
                bundle.screen_crop = config.screen_crop;
                bundle.appearance = config.appearance;
                bundle.export_settings = config.export_settings;
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

    pub(crate) fn input_media_path(&self, bundle_path: &Path, id: &str) -> Option<PathBuf> {
        self.inputs
            .iter()
            .find(|input| input.id == id)
            .map(|input| bundle_path.join(&input.media))
    }

    pub(crate) fn set_input_start_offset(
        &mut self,
        bundle_path: &Path,
        id: &str,
        start_offset_secs: f64,
    ) -> Result<(), String> {
        let input = self
            .inputs
            .iter_mut()
            .find(|input| input.id == id)
            .ok_or_else(|| format!("Blip Bundle has no {id} input"))?;
        input.start_offset_secs = start_offset_secs;
        self.save_manifest(bundle_path)
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
            video_segment_resize_mode: self.video_segment_resize_mode,
            camera_layout: self.camera_layout,
            output_aspect_ratio: self.output_aspect_ratio,
            screen_crop: self.screen_crop,
            appearance: self.appearance.clone(),
            export_settings: self.export_settings.clone(),
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

    use super::{
        BackgroundType, BlipBundle, BundleInputKind, CameraCrop, CameraLayout, CameraPosition,
        ExportFormat, ExportResolution, ExportSettings, OutputAspectRatio, PROJECT_CONFIG_FILE,
        ProjectAppearance, ScreenCrop, VideoSegment, VideoSegmentResizeMode,
    };

    #[test]
    fn creates_a_manifest_and_separate_screen_input() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-bundle-test-{suffix}.blip"));
        let result = BlipBundle::create(&path, false, false);
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
        assert_eq!(
            bundle.video_segment_resize_mode,
            VideoSegmentResizeMode::Ghost
        );
        assert_eq!(bundle.camera_layout, CameraLayout::default());
        assert_eq!(bundle.output_aspect_ratio, OutputAspectRatio::Wide);
        assert_eq!(bundle.screen_crop, None);
        assert_eq!(bundle.appearance, ProjectAppearance::default());
        assert_eq!(bundle.export_settings, ExportSettings::default());

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn creates_camera_input_and_persists_its_alignment() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-camera-test-{suffix}.blip"));
        let mut bundle = BlipBundle::create(&path, true, false).expect("create camera bundle");

        assert_eq!(
            bundle.input_media_path(&path, "camera"),
            Some(path.join("inputs/camera.mp4"))
        );
        bundle
            .set_input_start_offset(&path, "camera", -0.125)
            .expect("save camera alignment");

        let loaded = BlipBundle::load(&path).expect("load camera bundle");
        assert_eq!(loaded.inputs.len(), 2);
        assert_eq!(loaded.inputs[1].start_offset_secs, -0.125);

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn creates_system_audio_input_and_persists_its_alignment() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-audio-test-{suffix}.blip"));
        let mut bundle = BlipBundle::create(&path, false, true).expect("create audio bundle");

        assert_eq!(
            bundle.input_media_path(&path, "system_audio"),
            Some(path.join("inputs/system-audio.m4a"))
        );
        assert_eq!(bundle.inputs[1].kind, BundleInputKind::Audio);
        bundle
            .set_input_start_offset(&path, "system_audio", 0.125)
            .expect("save audio alignment");

        let loaded = BlipBundle::load(&path).expect("load audio bundle");
        assert_eq!(loaded.inputs[1].kind, BundleInputKind::Audio);
        assert_eq!(loaded.inputs[1].start_offset_secs, 0.125);

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn loads_manifests_without_zoom_segments() {
        let manifest = r#"{
            "version": 1,
            "created_at": "2026-07-28T12:00:00-07:00",
            "inputs": [{"id":"screen","name":"Screen","media":"inputs/screen.mp4"}]
        }"#;
        let bundle: BlipBundle = serde_json::from_str(manifest).expect("decode legacy manifest");
        assert!(bundle.zoom_segments.is_empty());
        assert!(bundle.video_segments.is_none());
        assert_eq!(bundle.inputs[0].start_offset_secs, 0.0);
        assert_eq!(bundle.inputs[0].gain, 1.0);
        assert_eq!(bundle.inputs[0].kind, BundleInputKind::Video);
        assert_eq!(bundle.camera_layout, CameraLayout::default());
        assert_eq!(bundle.output_aspect_ratio, OutputAspectRatio::Wide);
        assert_eq!(bundle.screen_crop, None);
        assert_eq!(bundle.appearance, ProjectAppearance::default());
        assert_eq!(bundle.export_settings, ExportSettings::default());
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
    fn loads_project_configs_without_editor_settings() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-legacy-config-test-{suffix}.blip"));
        BlipBundle::create(&path, false, false).expect("create bundle");
        std::fs::write(path.join(PROJECT_CONFIG_FILE), "{}").expect("write legacy config");

        let loaded = BlipBundle::load(&path).expect("load legacy project config");

        assert_eq!(loaded.appearance, ProjectAppearance::default());
        assert_eq!(loaded.export_settings, ExportSettings::default());

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn saves_edits_without_changing_the_manifest() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("blip-config-test-{suffix}.blip"));
        let mut bundle = BlipBundle::create(&path, false, false).expect("create bundle");
        let manifest = std::fs::read(path.join("manifest.json")).expect("read manifest");
        bundle.video_segments = Some(vec![VideoSegment {
            id: 1,
            source_start_secs: 1.0,
            source_end_secs: 2.0,
        }]);
        bundle.video_segment_resize_mode = VideoSegmentResizeMode::Live;
        bundle.camera_layout = CameraLayout {
            size: 32.0,
            position: CameraPosition::TopLeft,
            edge_padding: 5.0,
            zoom_size_reduction: 20.0,
            shadow: 25.0,
            crop: CameraCrop::Circle,
        };
        bundle.output_aspect_ratio = OutputAspectRatio::Auto;
        bundle.screen_crop = Some(ScreenCrop {
            position: [0.1, 0.2],
            size: [0.7, 0.6],
        });
        bundle.appearance = ProjectAppearance {
            background_type: BackgroundType::Image,
            background_image: "sonoma-river.jpg".into(),
            padding: 12.0,
            border_radius: 16.0,
            shadow: 24.0,
        };
        bundle.export_settings = ExportSettings {
            format: ExportFormat::Mov,
            resolution: ExportResolution::P720,
            fps: 60,
            destination: Some(path.join("export.mov")),
        };

        bundle
            .save_project_config(&path)
            .expect("save project config");

        assert_eq!(
            std::fs::read(path.join("manifest.json")).expect("read unchanged manifest"),
            manifest
        );
        assert!(path.join("project-config.json").is_file());
        let loaded = BlipBundle::load(&path).expect("load bundle");
        assert_eq!(
            loaded
                .video_segments
                .as_deref()
                .map(|segments| segments.len()),
            Some(1)
        );
        assert_eq!(
            loaded.video_segment_resize_mode,
            VideoSegmentResizeMode::Live
        );
        assert_eq!(loaded.camera_layout, bundle.camera_layout);
        assert_eq!(loaded.output_aspect_ratio, OutputAspectRatio::Auto);
        assert_eq!(loaded.screen_crop, bundle.screen_crop);
        assert_eq!(loaded.appearance, bundle.appearance);
        assert_eq!(loaded.export_settings, bundle.export_settings);

        std::fs::remove_dir_all(path).ok();
    }
}

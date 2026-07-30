#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division
)]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use blip_ui::{
    DROPDOWN_ANIMATION_DURATION, DropdownStyle, dropdown_chevron, dropdown_menu, dropdown_option,
    dropdown_trigger,
};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::CFString,
};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Context, CursorStyle, Div, FocusHandle,
    FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    ObjectFit, PinchEvent, Pixels, Render, ScrollWheelEvent, SharedString, TextAlign, TextRun,
    TitlebarOptions, Window, WindowOptions, canvas, deferred, div, fill, img, point, prelude::*,
    px, relative, rgb, rgba, size, surface, svg,
};

use crate::{
    assets::{CHECK, CHEVRON_DOWN, PAUSE, PLAY, PLAYBACK_BACK, PLAYBACK_FORWARD, SHAPE_FRAME},
    bundle::{
        BackgroundType, BlipBundle, BundleInputKind, CameraCrop, CameraLayout, CameraPosition,
        ExportFormat, ExportResolution, OutputAspectRatio, ScreenCrop, VideoSegment,
        VideoSegmentResizeMode, ZoomSegment, ZoomTransitionSpeed,
    },
    theme,
};

const ACCENT: u32 = 0x00ff_4f58;
const CROP_SELECTION: u32 = 0x00ff_ffff;
const CROP_ACTION: u32 = 0x00d2_d2d2;
const PREVIEW_HANDLE: u32 = 0x007f_7f7f;
const SIDEBAR_WIDTH: Pixels = px(280.0);
const PREVIEW_LOADING_DURATION: Duration = Duration::from_millis(1_400);
const PREVIEW_FADE_DURATION: Duration = Duration::from_millis(240);
const TIMELINE_RESIZE_DURATION: Duration = Duration::from_millis(180);
const TIMELINE_TRAILING_SPACE_SECS: f64 = 3.0;
const ANIMATION_TICK_INTERVAL: Duration = Duration::from_millis(16);
const MIN_VIDEO_SEGMENT_DURATION_SECS: f64 = 1.0;
const BUNDLED_BACKGROUNDS: [(&str, &[u8], &[u8]); 15] = [
    (
        "tahoe-dusk.jpg",
        include_bytes!("../assets/backgrounds/tahoe-dusk-min.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/tahoe-dusk-min.jpg"),
    ),
    (
        "tahoe-dawn.jpg",
        include_bytes!("../assets/backgrounds/tahoe-dawn-min.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/tahoe-dawn-min.jpg"),
    ),
    (
        "tahoe-day.jpg",
        include_bytes!("../assets/backgrounds/tahoe-day-min.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/tahoe-day-min.jpg"),
    ),
    (
        "tahoe-night.jpg",
        include_bytes!("../assets/backgrounds/tahoe-night-min.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/tahoe-night-min.jpg"),
    ),
    (
        "tahoe-dark.jpg",
        include_bytes!("../assets/backgrounds/tahoe-dark.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/tahoe-dark.jpg"),
    ),
    (
        "tahoe-light.jpg",
        include_bytes!("../assets/backgrounds/tahoe-light.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/tahoe-light.jpg"),
    ),
    (
        "sequoia-dark.jpg",
        include_bytes!("../assets/backgrounds/sequoia-dark.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sequoia-dark.jpg"),
    ),
    (
        "sequoia-light.jpg",
        include_bytes!("../assets/backgrounds/sequoia-light.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sequoia-light.jpg"),
    ),
    (
        "sonoma-clouds.jpg",
        include_bytes!("../assets/backgrounds/sonoma-clouds.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-clouds.jpg"),
    ),
    (
        "sonoma-fromabove.jpg",
        include_bytes!("../assets/backgrounds/sonoma-fromabove.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-fromabove.jpg"),
    ),
    (
        "sonoma-evening.jpg",
        include_bytes!("../assets/backgrounds/sonoma-evening.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-evening.jpg"),
    ),
    (
        "sonoma-horizon.jpg",
        include_bytes!("../assets/backgrounds/sonoma-horizon.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-horizon.jpg"),
    ),
    (
        "sonoma-river.jpg",
        include_bytes!("../assets/backgrounds/sonoma-river.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-river.jpg"),
    ),
    (
        "sonoma-dark.jpg",
        include_bytes!("../assets/backgrounds/sonoma-dark.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-dark.jpg"),
    ),
    (
        "sonoma-light.jpg",
        include_bytes!("../assets/backgrounds/sonoma-light.jpg"),
        include_bytes!("../assets/backgrounds/thumbnails/sonoma-light.jpg"),
    ),
];

struct BackgroundImage {
    name: String,
    image: PathBuf,
    thumbnail: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Screen,
    Camera,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
        }
    }

    fn file_type(self) -> blip_avfoundation::VideoFileType {
        match self {
            Self::Mp4 => blip_avfoundation::VideoFileType::Mp4,
            Self::Mov => blip_avfoundation::VideoFileType::Mov,
        }
    }
}

impl ExportResolution {
    fn short_edge(self) -> usize {
        match self {
            Self::P1080 => 1080,
            Self::P720 => 720,
        }
    }
}

enum ExportEvent {
    Progress(f32),
    Finished(Result<PathBuf, String>),
}

#[derive(Clone)]
struct ExportJob {
    output: PathBuf,
    file_type: blip_avfoundation::VideoFileType,
    short_edge: usize,
    aspect_ratio: OutputAspectRatio,
    fps: u32,
    duration_secs: f64,
    bundle: BlipBundle,
    decoder_inputs: Vec<(PathBuf, bool, f64)>,
    wallpaper: Option<PathBuf>,
    background_type: BackgroundType,
    padding: f32,
    border_radius: f32,
    shadow: f32,
    audio_mixer: blip_audio::AudioMixer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SliderKind {
    Padding,
    Radius,
    Shadow,
    CameraSize,
    CameraPadding,
    CameraZoomReduction,
    CameraShadow,
    ZoomAmount,
    TimelineZoom,
}

#[derive(Clone, Copy)]
enum ZoomSegmentEdge {
    Start,
    End,
}

#[derive(Clone, Copy)]
enum ZoomSegmentDragKind {
    Move,
    Resize(ZoomSegmentEdge),
}

#[derive(Clone, Copy)]
struct ZoomSegmentDrag {
    id: u64,
    kind: ZoomSegmentDragKind,
    pointer_start_x: Pixels,
    original_start_secs: f64,
    original_end_secs: f64,
    draft_start_secs: f64,
    draft_end_secs: f64,
    minimum_start_secs: f64,
    maximum_end_secs: f64,
}

#[derive(Clone, Copy)]
enum VideoSegmentEdge {
    Start,
    End,
}

#[derive(Clone, Copy)]
enum VideoCropIndicatorAction {
    SegmentEdge(u64, VideoSegmentEdge),
    Cut(u64),
}

#[derive(Clone, Copy)]
enum VideoCropIndicatorAlignment {
    Start,
    Center,
    End,
}

#[derive(Clone)]
struct VideoSegmentDrag {
    id: u64,
    edge: VideoSegmentEdge,
    pointer_start_x: Pixels,
    timeline_start_secs: f64,
    timeline_view_duration_secs: f64,
    original_source_start_secs: f64,
    original_source_end_secs: f64,
    draft_source_start_secs: f64,
    draft_source_end_secs: f64,
    minimum_source_start_secs: f64,
    maximum_source_end_secs: f64,
    original_zoom_segments: Vec<ZoomSegment>,
}

#[derive(Clone)]
struct VideoTimelineResizeAnimation {
    from_ranges: Vec<(u64, f64, f64)>,
    from_zoom_ranges: Vec<(u64, f64, f64)>,
    ghost_range: Option<(u64, VideoSegmentEdge, f64, f64)>,
    playback_range: Option<(f32, f32)>,
    view_start_secs: f64,
    view_duration_secs: f64,
    target_view_start_secs: f64,
    target_view_duration_secs: f64,
    generation: u64,
}

#[derive(Clone, Copy)]
struct PreviewRequest {
    playback_generation: u64,
    timeline_time_secs: f64,
    time_secs: f64,
    background_type: BackgroundType,
    background_preset: usize,
    padding: f32,
    aspect_ratio: OutputAspectRatio,
    border_radius: f32,
    shadow: f32,
    zoom: blip_compositor::OutputTransform,
    camera_layout: CameraLayout,
    screen_crop: Option<ScreenCrop>,
}

struct PreviewFrame(core_video::pixel_buffer::CVPixelBuffer);

struct PreviewResult {
    playback_generation: u64,
    timeline_time_secs: f64,
    frame: PreviewFrame,
    screen: PreviewFrame,
}

struct ContentFrame {
    screen: core_video::pixel_buffer::CVPixelBuffer,
    camera: Option<core_video::pixel_buffer::CVPixelBuffer>,
    dimensions: (usize, usize),
}

#[derive(Clone, Copy)]
enum CropDrag {
    Create {
        start: [f32; 2],
    },
    Move {
        pointer_start: [f32; 2],
        original: ScreenCrop,
    },
    Resize {
        handle: CropResizeHandle,
        original: ScreenCrop,
    },
}

#[derive(Clone, Copy)]
enum CropResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

// CVPixelBuffers have thread-safe Core Foundation ownership and are immutable after composition.
unsafe impl Send for PreviewFrame {}

pub(crate) struct BundleEditor {
    focus_handle: FocusHandle,
    path: PathBuf,
    bundle: BlipBundle,
    selected_input: usize,
    selected_zoom: Option<u64>,
    selected_video_segment: Option<u64>,
    cut_mode: bool,
    preview_requests: async_channel::Sender<PreviewRequest>,
    zoom_target_requests: async_channel::Sender<f64>,
    duration_secs: f64,
    source_start_secs: f64,
    source_end_secs: f64,
    timeline_fps: u32,
    timeline_view_start_secs: f64,
    timeline_view_duration_secs: f64,
    current_time_secs: f64,
    is_playing: bool,
    playback_started_at: Option<(Instant, f64)>,
    playback_generation: u64,
    cursor_time_secs: Option<f64>,
    zoom_hover_range: Option<(f64, f64)>,
    current_frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    last_playback_preview_time_secs: Option<f64>,
    current_screen_frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    timeline_bounds: Rc<Cell<Bounds<Pixels>>>,
    timeline_rows_bounds: Rc<Cell<Bounds<Pixels>>>,
    zoom_target_bounds: Rc<Cell<Bounds<Pixels>>>,
    background_preset: usize,
    background_images: Vec<BackgroundImage>,
    padding_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    radius_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    shadow_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    camera_size_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    camera_padding_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    camera_zoom_reduction_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    camera_shadow_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    zoom_amount_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    timeline_zoom_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    active_slider: Option<SliderKind>,
    dragging_zoom_target: bool,
    zoom_segment_drag: Option<ZoomSegmentDrag>,
    video_segment_drag: Option<VideoSegmentDrag>,
    video_timeline_resize_animation: Option<VideoTimelineResizeAnimation>,
    video_timeline_resize_generation: u64,
    sidebar_tab: SidebarTab,
    settings_dialog_open: bool,
    aspect_ratio_dropdown_open: bool,
    aspect_ratio_dropdown_visible: bool,
    aspect_ratio_dropdown_transition: u64,
    crop_dialog_open: bool,
    crop_draft: ScreenCrop,
    crop_drag: Option<CropDrag>,
    crop_preview_bounds: Rc<Cell<Bounds<Pixels>>>,
    crop_frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    export_dialog_open: bool,
    export_progress: f32,
    exporting: bool,
    export_error: Option<String>,
    exported_path: Option<PathBuf>,
    audio_sources: Vec<blip_audio::AudioSource>,
    audio_playback: Option<blip_audio::AudioPlayback>,
}

impl BundleEditor {
    pub(crate) fn open(path: PathBuf, cx: &mut App) -> Result<(), String> {
        let mut bundle = BlipBundle::load(&path)?;
        tracing::info!(
            path = %path.display(),
            inputs = bundle.inputs.len(),
            "Opening bundle editor"
        );
        let mut decoder_inputs = Vec::new();
        let mut source_ranges = Vec::new();
        let mut audio_sources = Vec::new();
        let mut timeline_fps = None;
        for (index, input) in bundle.inputs.iter().enumerate() {
            if input.kind != BundleInputKind::Video {
                continue;
            }
            let media_path = path.join(&input.media);
            match blip_avfoundation::VideoDecoder::open(&media_path) {
                Ok(decoder) => {
                    let dur = decoder.duration().as_secs_f64();
                    timeline_fps = include_timeline_fps(timeline_fps, decoder.nominal_fps());
                    source_ranges.push((input.start_offset_secs, input.start_offset_secs + dur));
                    decoder_inputs.push((
                        media_path,
                        input_is_camera(input, index),
                        input.start_offset_secs,
                    ));
                }
                Err(error) => {
                    tracing::error!(
                        path = %media_path.display(),
                        error = %error,
                        "Failed to open video decoder"
                    );
                    eprintln!(
                        "blip-capture: failed to open video decoder for {}: {error}",
                        media_path.display()
                    );
                }
            }
        }
        for input in bundle
            .inputs
            .iter()
            .filter(|input| input.kind == BundleInputKind::Audio)
        {
            let media_path = path.join(&input.media);
            match blip_avfoundation::decode_audio(&media_path).and_then(|samples| {
                blip_audio::AudioSource::new(samples, input.start_offset_secs, input.gain).map_err(
                    |error| blip_avfoundation::DecoderError::DecodeAudio(error.to_string()),
                )
            }) {
                Ok(source) => audio_sources.push(source),
                Err(error) => tracing::error!(
                    path = %media_path.display(),
                    error = %error,
                    "Failed to decode audio input"
                ),
            }
        }
        let (source_start_secs, source_end_secs) =
            shared_source_range(source_ranges).unwrap_or((0.0, 0.0));
        if bundle.video_segments.is_none() {
            bundle.video_segments = Some(if source_end_secs > source_start_secs {
                vec![VideoSegment {
                    id: 1,
                    source_start_secs,
                    source_end_secs,
                }]
            } else {
                Vec::new()
            });
        } else {
            clamp_video_timeline_to_source_range(&mut bundle, source_start_secs, source_end_secs);
        }
        let timeline_duration = video_timeline_duration(&bundle);
        bundle.screen_crop = normalize_screen_crop(bundle.screen_crop);
        let crop_draft = bundle.screen_crop.unwrap_or(ScreenCrop::FULL);
        let background_images = bundled_backgrounds();
        let background_preset = background_images
            .iter()
            .position(|background| background.name == bundle.appearance.background_image)
            .unwrap_or(0);
        let preview_backgrounds = background_images
            .iter()
            .map(|background| background.image.clone())
            .collect();
        let screen_input = decoder_inputs
            .iter()
            .find(|(_, is_camera, _)| !*is_camera)
            .map(|(path, _, _)| path.clone());
        let (preview_requests, preview_request_receiver) = async_channel::bounded(1);
        let (preview_results, preview_result_receiver) = async_channel::unbounded();
        let (zoom_target_requests, zoom_target_request_receiver) = async_channel::bounded(1);
        let (zoom_target_results, zoom_target_result_receiver) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("bundle-preview".into())
            .spawn(move || {
                preview_worker(
                    decoder_inputs,
                    preview_backgrounds,
                    preview_request_receiver,
                    preview_results,
                );
            })
            .map_err(|error| format!("failed to start preview worker: {error}"))?;
        std::thread::Builder::new()
            .name("zoom-target-preview".into())
            .spawn(move || {
                zoom_target_worker(
                    screen_input,
                    zoom_target_request_receiver,
                    zoom_target_results,
                );
            })
            .map_err(|error| format!("failed to start zoom target worker: {error}"))?;

        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Blip Bundle")
            .to_owned();
        crate::show_in_dock();
        let result = cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.0), px(12.0))),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title(&title);
                let editor = cx.new(|cx| {
                    let editor_entity: gpui::WeakEntity<Self> = cx.weak_entity();
                    cx.spawn(async move |_, cx| {
                        while let Ok(result) = preview_result_receiver.recv().await {
                            if editor_entity
                                .update(cx, |editor, cx| {
                                    if result.playback_generation != editor.playback_generation
                                        || (editor.is_playing
                                            && editor.last_playback_preview_time_secs.is_some_and(
                                                |time_secs| result.timeline_time_secs < time_secs,
                                            ))
                                    {
                                        return;
                                    }
                                    if editor.is_playing {
                                        editor.last_playback_preview_time_secs =
                                            Some(result.timeline_time_secs);
                                    }
                                    editor.current_frame = Some(result.frame.0);
                                    editor.crop_frame = Some(result.screen.0);
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                    let editor_entity: gpui::WeakEntity<Self> = cx.weak_entity();
                    cx.spawn(async move |_, cx| {
                        while let Ok((time_secs, frame)) = zoom_target_result_receiver.recv().await
                        {
                            if editor_entity
                                .update(cx, |editor, cx| {
                                    if editor.selected_zoom().is_some_and(|segment| {
                                        source_time_at(&editor.bundle, segment.start_secs)
                                            .is_some_and(|expected| expected == time_secs)
                                    }) {
                                        editor.current_screen_frame = Some(frame.0);
                                        cx.notify();
                                    }
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();

                    let editor = Self {
                        focus_handle: cx.focus_handle(),
                        path,
                        bundle,
                        selected_input: 0,
                        selected_zoom: None,
                        selected_video_segment: None,
                        cut_mode: false,
                        preview_requests,
                        zoom_target_requests,
                        duration_secs: timeline_duration,
                        source_start_secs,
                        source_end_secs,
                        timeline_fps: timeline_fps.unwrap_or(60),
                        timeline_view_start_secs: 0.0,
                        timeline_view_duration_secs: timeline_extent_secs(timeline_duration),
                        current_time_secs: 0.0,
                        is_playing: false,
                        playback_started_at: None,
                        playback_generation: 0,
                        cursor_time_secs: None,
                        zoom_hover_range: None,
                        current_frame: None,
                        last_playback_preview_time_secs: None,
                        current_screen_frame: None,
                        timeline_bounds: Rc::new(Cell::new(Bounds::default())),
                        timeline_rows_bounds: Rc::new(Cell::new(Bounds::default())),
                        zoom_target_bounds: Rc::new(Cell::new(Bounds::default())),
                        background_preset,
                        background_images,
                        padding_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        radius_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        shadow_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        camera_size_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        camera_padding_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        camera_zoom_reduction_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        camera_shadow_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        zoom_amount_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        timeline_zoom_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        active_slider: None,
                        dragging_zoom_target: false,
                        zoom_segment_drag: None,
                        video_segment_drag: None,
                        video_timeline_resize_animation: None,
                        video_timeline_resize_generation: 0,
                        sidebar_tab: SidebarTab::Screen,
                        settings_dialog_open: false,
                        aspect_ratio_dropdown_open: false,
                        aspect_ratio_dropdown_visible: false,
                        aspect_ratio_dropdown_transition: 0,
                        crop_dialog_open: false,
                        crop_draft,
                        crop_drag: None,
                        crop_preview_bounds: Rc::new(Cell::new(Bounds::default())),
                        crop_frame: None,
                        export_dialog_open: false,
                        export_progress: 0.0,
                        exporting: false,
                        export_error: None,
                        exported_path: None,
                        audio_sources,
                        audio_playback: None,
                    };
                    editor.request_preview();
                    editor
                });
                let focus_handle = editor.read(cx).focus_handle.clone();
                window.focus(&focus_handle, cx);
                editor
            },
        );
        if let Err(error) = result {
            crate::refresh_activation_policy(cx);
            return Err(format!("failed to open bundle editor: {error}"));
        }
        cx.activate(true);
        Ok(())
    }

    fn selected_zoom(&self) -> Option<&ZoomSegment> {
        let id = self.selected_zoom?;
        self.bundle
            .zoom_segments
            .iter()
            .find(|segment| segment.id == id)
    }

    fn selected_zoom_mut(&mut self) -> Option<&mut ZoomSegment> {
        let id = self.selected_zoom?;
        self.bundle
            .zoom_segments
            .iter_mut()
            .find(|segment| segment.id == id)
    }

    fn selected_video_segment(&self) -> Option<&VideoSegment> {
        let id = self.selected_video_segment?;
        self.bundle
            .video_segments
            .as_ref()?
            .iter()
            .find(|segment| segment.id == id)
    }

    fn select_video_segment(&mut self, id: u64, cx: &mut Context<Self>) {
        if self
            .bundle
            .video_segments
            .as_ref()
            .is_some_and(|segments| segments.iter().any(|segment| segment.id == id))
        {
            self.selected_video_segment = Some(id);
            self.selected_zoom = None;
            cx.notify();
        }
    }

    fn save_bundle(&self) {
        if let Err(error) = self.bundle.save_project_config(&self.path) {
            tracing::error!(error, "Failed to save project config");
        }
    }

    fn choose_export_destination(&mut self, cx: &mut Context<Self>) {
        let directory = self
            .bundle
            .export_settings
            .destination
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|path| path.join("Movies"))
            })
            .unwrap_or_else(std::env::temp_dir);
        let stem = self
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Blip Export");
        let suggested_name = format!("{stem}.{}", self.bundle.export_settings.format.extension());
        let receiver = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn(async move |editor, cx| match receiver.await {
            Ok(Ok(Some(path))) => {
                let _ = editor.update(cx, |editor, cx| {
                    editor.bundle.export_settings.destination = Some(path);
                    editor.save_bundle();
                    editor.export_error = None;
                    editor.exported_path = None;
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = editor.update(cx, |editor, cx| {
                    editor.export_error = Some(format!("Could not choose destination: {error}"));
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn start_export(&mut self, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(destination) = self.bundle.export_settings.destination.clone() else {
            self.export_error = Some("Choose a destination before exporting.".into());
            cx.notify();
            return;
        };
        if self.duration_secs <= 0.0 {
            self.export_error = Some("There is no video on the timeline to export.".into());
            cx.notify();
            return;
        }
        let output = destination.with_extension(self.bundle.export_settings.format.extension());
        let decoder_inputs = self
            .bundle
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, input)| input.kind == BundleInputKind::Video)
            .map(|(index, input)| {
                (
                    self.path.join(&input.media),
                    input_is_camera(input, index),
                    input.start_offset_secs,
                )
            })
            .collect();
        let job = ExportJob {
            output: output.clone(),
            file_type: self.bundle.export_settings.format.file_type(),
            short_edge: self.bundle.export_settings.resolution.short_edge(),
            aspect_ratio: self.bundle.output_aspect_ratio,
            fps: self.bundle.export_settings.fps,
            duration_secs: self.duration_secs,
            bundle: self.bundle.clone(),
            decoder_inputs,
            wallpaper: (self.bundle.appearance.background_type == BackgroundType::Image)
                .then(|| self.background_images.get(self.background_preset))
                .flatten()
                .map(|background| background.image.clone()),
            background_type: self.bundle.appearance.background_type,
            padding: self.bundle.appearance.padding,
            border_radius: self.bundle.appearance.border_radius,
            shadow: self.bundle.appearance.shadow,
            audio_mixer: audio_mixer(&self.audio_sources, &self.bundle),
        };
        let (events, event_receiver) = async_channel::unbounded();
        if let Err(error) = std::thread::Builder::new()
            .name("bundle-export".into())
            .spawn(move || export_worker(job, events))
        {
            self.export_error = Some(format!("Could not start export: {error}"));
            cx.notify();
            return;
        }
        self.exporting = true;
        self.export_progress = 0.0;
        self.export_error = None;
        self.exported_path = None;
        self.bundle.export_settings.destination = Some(output);
        self.save_bundle();
        cx.notify();

        cx.spawn(async move |editor, cx| {
            while let Ok(event) = event_receiver.recv().await {
                let finished = matches!(event, ExportEvent::Finished(_));
                if editor
                    .update(cx, |editor, cx| {
                        match event {
                            ExportEvent::Progress(progress) => editor.export_progress = progress,
                            ExportEvent::Finished(result) => {
                                editor.exporting = false;
                                match result {
                                    Ok(path) => {
                                        editor.export_progress = 1.0;
                                        editor.exported_path = Some(path);
                                    }
                                    Err(error) => editor.export_error = Some(error),
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                    || finished
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn toggle_cut_mode(&mut self, cx: &mut Context<Self>) {
        self.cut_mode = !self.cut_mode;
        cx.notify();
    }

    fn activate_video_segment(&mut self, id: u64, timeline_time_secs: f64, cx: &mut Context<Self>) {
        if self.cut_mode {
            self.split_video_segment(id, timeline_time_secs, cx);
        } else {
            self.select_video_segment(id, cx);
            self.set_playback_time(timeline_time_secs);
        }
    }

    fn split_video_segment(&mut self, id: u64, timeline_time_secs: f64, cx: &mut Context<Self>) {
        let Some(segments) = self.bundle.video_segments.as_mut() else {
            return;
        };
        let Some(index) = segments.iter().position(|segment| segment.id == id) else {
            return;
        };
        let timeline_start = segments
            .iter()
            .take(index)
            .map(VideoSegment::duration_secs)
            .sum::<f64>();
        let Some(segment) = segments.get(index) else {
            return;
        };
        let offset = timeline_time_secs - timeline_start;
        let duration = segment.duration_secs();
        if !can_split_video_segment(duration, offset) {
            return;
        }
        let new_id = segments.iter().map(|segment| segment.id).max().unwrap_or(0) + 1;
        let source_split = segment.source_start_secs + offset;
        let source_end = segment.source_end_secs;
        let Some(segment) = segments.get_mut(index) else {
            return;
        };
        segment.source_end_secs = source_split;
        segments.insert(
            index + 1,
            VideoSegment {
                id: new_id,
                source_start_secs: source_split,
                source_end_secs: source_end,
            },
        );
        self.selected_video_segment = Some(new_id);
        self.selected_zoom = None;
        self.current_time_secs = timeline_time_secs;
        self.save_bundle();
        self.request_preview();
        cx.notify();
    }

    fn begin_video_segment_drag(
        &mut self,
        id: u64,
        edge: VideoSegmentEdge,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(segments) = self.bundle.video_segments.as_ref() else {
            return;
        };
        let Some(index) = segments.iter().position(|segment| segment.id == id) else {
            return;
        };
        let segment = &segments[index];
        self.video_segment_drag = Some(VideoSegmentDrag {
            id,
            edge,
            pointer_start_x: event.position.x,
            timeline_start_secs: segments
                .iter()
                .take(index)
                .map(VideoSegment::duration_secs)
                .sum(),
            timeline_view_duration_secs: self.timeline_view_duration_secs,
            original_source_start_secs: segment.source_start_secs,
            original_source_end_secs: segment.source_end_secs,
            draft_source_start_secs: segment.source_start_secs,
            draft_source_end_secs: segment.source_end_secs,
            minimum_source_start_secs: index
                .checked_sub(1)
                .and_then(|index| segments.get(index))
                .map(|segment| segment.source_end_secs)
                .unwrap_or(self.source_start_secs),
            maximum_source_end_secs: segments
                .get(index + 1)
                .map(|segment| segment.source_start_secs)
                .unwrap_or(self.source_end_secs),
            original_zoom_segments: self.bundle.zoom_segments.clone(),
        });
        self.video_timeline_resize_generation += 1;
        self.video_timeline_resize_animation = None;
        self.selected_video_segment = Some(id);
        self.selected_zoom = None;
        self.cursor_time_secs = None;
        cx.notify();
    }

    fn drag_video_segment(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.video_segment_drag.clone() else {
            return false;
        };
        if !event.dragging() {
            return true;
        }
        let bounds = self.timeline_bounds.get();
        if bounds.size.width <= Pixels::ZERO {
            return true;
        }

        let delta_fraction = (event.position.x - drag.pointer_start_x) / bounds.size.width;
        let delta_secs = f64::from(delta_fraction) * drag.timeline_view_duration_secs;
        let (source_start_secs, source_end_secs) = resize_video_segment_range(
            drag.edge,
            drag.original_source_start_secs,
            drag.original_source_end_secs,
            drag.minimum_source_start_secs,
            drag.maximum_source_end_secs,
            delta_secs,
        );
        if let Some(drag) = self.video_segment_drag.as_mut() {
            drag.draft_source_start_secs = source_start_secs;
            drag.draft_source_end_secs = source_end_secs;
        }
        if let Some(time_secs) = self.timeline_time_at(event.position.x) {
            self.set_playback_time(time_secs);
        }
        cx.notify();
        true
    }

    fn finish_video_segment_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.video_segment_drag.take() else {
            return;
        };
        let original_duration = drag.original_source_end_secs - drag.original_source_start_secs;
        let new_duration = drag.draft_source_end_secs - drag.draft_source_start_secs;
        let mut timeline_start = 0.0;
        let from_ranges = self
            .bundle
            .video_segments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|segment| {
                let duration = segment.duration_secs();
                let edge = (segment.id == drag.id).then_some(drag.edge);
                let (display_start, display_end, next_timeline_start) = video_segment_drag_layout(
                    self.bundle.video_segment_resize_mode,
                    edge,
                    timeline_start,
                    duration,
                    new_duration,
                );
                timeline_start = next_timeline_start;
                (segment.id, display_start, display_end)
            })
            .collect();
        let animation_view_start_secs = self.timeline_view_start_secs;
        let animation_view_duration_secs = self.timeline_view_duration_secs;
        let from_zoom_ranges = drag
            .original_zoom_segments
            .iter()
            .map(|segment| (segment.id, segment.start_secs, segment.end_secs))
            .collect();
        let ghost_range = (
            drag.id,
            drag.edge,
            drag.timeline_start_secs,
            drag.timeline_start_secs + original_duration,
        );
        let Some(segment) = self
            .bundle
            .video_segments
            .as_mut()
            .and_then(|segments| segments.iter_mut().find(|segment| segment.id == drag.id))
        else {
            return;
        };
        segment.source_start_secs = drag.draft_source_start_secs;
        segment.source_end_secs = drag.draft_source_end_secs;

        self.bundle.zoom_segments = drag.original_zoom_segments;
        if new_duration < original_duration {
            let removed_at_secs = match drag.edge {
                VideoSegmentEdge::Start => drag.timeline_start_secs,
                VideoSegmentEdge::End => drag.timeline_start_secs + new_duration,
            };
            ripple_delete_ranges(
                &mut self.bundle.zoom_segments,
                removed_at_secs,
                original_duration - new_duration,
            );
        } else {
            let inserted_at_secs = match drag.edge {
                VideoSegmentEdge::Start => drag.timeline_start_secs,
                VideoSegmentEdge::End => drag.timeline_start_secs + original_duration,
            };
            ripple_insert_ranges(
                &mut self.bundle.zoom_segments,
                inserted_at_secs,
                new_duration - original_duration,
            );
        }
        self.duration_secs = video_timeline_duration(&self.bundle);
        self.current_time_secs = self.current_time_secs.min(self.duration_secs);
        let timeline_extent_secs = timeline_extent_secs(self.duration_secs);
        let target_view_duration_secs = self
            .timeline_view_duration_secs
            .min(timeline_extent_secs)
            .max(0.0);
        let target_view_start_secs = clamp_timeline_view_start(
            self.timeline_view_start_secs,
            target_view_duration_secs,
            timeline_extent_secs,
        );
        self.request_preview();
        self.request_zoom_target();
        self.save_bundle();
        let ghost_range = (self.bundle.video_segment_resize_mode == VideoSegmentResizeMode::Ghost)
            .then_some(ghost_range);
        self.animate_video_timeline_resize(
            from_ranges,
            from_zoom_ranges,
            ghost_range,
            None,
            animation_view_start_secs,
            animation_view_duration_secs,
            target_view_start_secs,
            target_view_duration_secs,
            cx,
        );
    }

    fn animate_video_timeline_resize(
        &mut self,
        from_ranges: Vec<(u64, f64, f64)>,
        from_zoom_ranges: Vec<(u64, f64, f64)>,
        ghost_range: Option<(u64, VideoSegmentEdge, f64, f64)>,
        playback_range: Option<(f32, f32)>,
        view_start_secs: f64,
        view_duration_secs: f64,
        target_view_start_secs: f64,
        target_view_duration_secs: f64,
        cx: &mut Context<Self>,
    ) {
        self.video_timeline_resize_generation += 1;
        let generation = self.video_timeline_resize_generation;
        self.video_timeline_resize_animation = Some(VideoTimelineResizeAnimation {
            from_ranges,
            from_zoom_ranges,
            ghost_range,
            playback_range,
            view_start_secs,
            view_duration_secs,
            target_view_start_secs,
            target_view_duration_secs,
            generation,
        });
        cx.notify();

        if cx.reduce_motion() {
            self.timeline_view_start_secs = target_view_start_secs;
            self.timeline_view_duration_secs = target_view_duration_secs;
            self.video_timeline_resize_animation = None;
            cx.notify();
            return;
        }
        cx.spawn(async move |editor, cx| {
            let started_at = Instant::now();
            loop {
                cx.background_executor()
                    .timer(ANIMATION_TICK_INTERVAL)
                    .await;
                let elapsed = started_at.elapsed().as_secs_f64();
                let progress = (elapsed / TIMELINE_RESIZE_DURATION.as_secs_f64()).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - progress).powi(5);
                let keep_animating = editor
                    .update(cx, |editor, cx| {
                        if editor.video_timeline_resize_generation != generation {
                            return false;
                        }
                        editor.timeline_view_start_secs =
                            view_start_secs + (target_view_start_secs - view_start_secs) * eased;
                        editor.timeline_view_duration_secs = view_duration_secs
                            + (target_view_duration_secs - view_duration_secs) * eased;
                        if progress >= 1.0 {
                            editor.video_timeline_resize_animation = None;
                        }
                        cx.notify();
                        progress < 1.0
                    })
                    .unwrap_or(false);
                if !keep_animating {
                    break;
                }
            }
        })
        .detach();
    }

    fn can_delete_video_segment(&self) -> bool {
        self.bundle
            .video_segments
            .as_ref()
            .is_some_and(|segments| segments.len() > 1)
    }

    fn delete_selected_video_segment(&mut self, cx: &mut Context<Self>) {
        if !self.can_delete_video_segment() {
            return;
        }
        let Some(id) = self.selected_video_segment else {
            return;
        };
        let Some(segments) = self.bundle.video_segments.as_mut() else {
            return;
        };
        let Some(index) = segments.iter().position(|segment| segment.id == id) else {
            return;
        };
        let timeline_start = segments
            .iter()
            .take(index)
            .map(VideoSegment::duration_secs)
            .sum::<f64>();
        let Some(removed_duration) = segments.get(index).map(VideoSegment::duration_secs) else {
            return;
        };
        segments.remove(index);
        self.selected_video_segment = segments
            .get(index)
            .or_else(|| index.checked_sub(1).and_then(|index| segments.get(index)))
            .map(|segment| segment.id);
        ripple_delete_ranges(
            &mut self.bundle.zoom_segments,
            timeline_start,
            removed_duration,
        );
        self.duration_secs = video_timeline_duration(&self.bundle);
        let timeline_extent_secs = timeline_extent_secs(self.duration_secs);
        self.timeline_view_duration_secs = self
            .timeline_view_duration_secs
            .min(timeline_extent_secs)
            .max(0.0);
        self.timeline_view_start_secs = clamp_timeline_view_start(
            self.timeline_view_start_secs,
            self.timeline_view_duration_secs,
            timeline_extent_secs,
        );
        self.current_time_secs = timeline_start.min(self.duration_secs);
        self.cursor_time_secs = None;
        if self.duration_secs == 0.0 {
            self.current_frame = None;
        }
        self.save_bundle();
        self.request_preview();
        cx.notify();
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_zoom {
            let segment_count = self.bundle.zoom_segments.len();
            self.bundle.zoom_segments.retain(|segment| segment.id != id);
            if self.bundle.zoom_segments.len() == segment_count {
                return;
            }
            self.selected_zoom = None;
            self.zoom_segment_drag = None;
            self.dragging_zoom_target = false;
            self.current_screen_frame = None;
            self.save_bundle();
            self.request_preview();
            cx.notify();
            return;
        }

        self.delete_selected_video_segment(cx);
    }

    fn activate_video_cut(&mut self, right_id: u64, cx: &mut Context<Self>) {
        let old_duration = self.duration_secs;
        let old_timeline_extent_secs = timeline_extent_secs(old_duration);
        let mut timeline_start_secs = 0.0;
        let from_ranges = self
            .bundle
            .video_segments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|segment| {
                let start_secs = timeline_start_secs;
                timeline_start_secs += segment.duration_secs();
                (segment.id, start_secs, timeline_start_secs)
            })
            .collect();
        let from_zoom_ranges = self
            .bundle
            .zoom_segments
            .iter()
            .map(|segment| (segment.id, segment.start_secs, segment.end_secs))
            .collect();
        let animation_view_start_secs = self.timeline_view_start_secs;
        let animation_view_duration_secs = self.timeline_view_duration_secs;
        let from_playback_fraction = timeline_time_fraction(
            self.current_time_secs,
            animation_view_start_secs,
            animation_view_duration_secs,
        );
        let Some(edit) = self
            .bundle
            .video_segments
            .as_mut()
            .and_then(|segments| edit_video_cut(segments, right_id))
        else {
            return;
        };

        let should_animate = edit.inserted_duration_secs > 0.0;
        if should_animate {
            ripple_insert_ranges(
                &mut self.bundle.zoom_segments,
                edit.timeline_start_secs,
                edit.inserted_duration_secs,
            );
            if self.current_time_secs >= edit.timeline_start_secs {
                self.current_time_secs += edit.inserted_duration_secs;
            }
        }
        if self.selected_video_segment == edit.removed_id {
            self.selected_video_segment = Some(edit.left_id);
        }

        self.duration_secs = video_timeline_duration(&self.bundle);
        let timeline_extent_secs = timeline_extent_secs(self.duration_secs);
        let (target_view_start_secs, target_view_duration_secs) = if (self.timeline_view_start_secs
            <= f64::EPSILON
            && self.timeline_view_duration_secs >= old_timeline_extent_secs - f64::EPSILON)
            || self.timeline_view_duration_secs > timeline_extent_secs
        {
            (0.0, timeline_extent_secs)
        } else {
            (
                clamp_timeline_view_start(
                    self.timeline_view_start_secs,
                    self.timeline_view_duration_secs,
                    timeline_extent_secs,
                ),
                self.timeline_view_duration_secs,
            )
        };
        self.cursor_time_secs = None;
        self.save_bundle();
        self.request_preview();
        self.request_zoom_target();
        if should_animate {
            let playback_range = from_playback_fraction.zip(timeline_time_fraction(
                self.current_time_secs,
                target_view_start_secs,
                target_view_duration_secs,
            ));
            self.animate_video_timeline_resize(
                from_ranges,
                from_zoom_ranges,
                None,
                playback_range,
                animation_view_start_secs,
                animation_view_duration_secs,
                target_view_start_secs,
                target_view_duration_secs,
                cx,
            );
        } else {
            self.video_timeline_resize_generation += 1;
            self.video_timeline_resize_animation = None;
            self.timeline_view_start_secs = target_view_start_secs;
            self.timeline_view_duration_secs = target_view_duration_secs;
            cx.notify();
        }
    }

    fn activate_video_segment_edge(
        &mut self,
        id: u64,
        edge: VideoSegmentEdge,
        cx: &mut Context<Self>,
    ) {
        let old_duration = self.duration_secs;
        let old_timeline_extent_secs = timeline_extent_secs(old_duration);
        let mut timeline_start_secs = 0.0;
        let from_ranges = self
            .bundle
            .video_segments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|segment| {
                let start_secs = timeline_start_secs;
                timeline_start_secs += segment.duration_secs();
                (segment.id, start_secs, timeline_start_secs)
            })
            .collect();
        let from_zoom_ranges = self
            .bundle
            .zoom_segments
            .iter()
            .map(|segment| (segment.id, segment.start_secs, segment.end_secs))
            .collect();
        let animation_view_start_secs = self.timeline_view_start_secs;
        let animation_view_duration_secs = self.timeline_view_duration_secs;
        let from_playback_fraction = timeline_time_fraction(
            self.current_time_secs,
            animation_view_start_secs,
            animation_view_duration_secs,
        );
        let Some(segments) = self.bundle.video_segments.as_mut() else {
            return;
        };
        let Some(index) = segments.iter().position(|segment| segment.id == id) else {
            return;
        };
        let timeline_start_secs = segments
            .iter()
            .take(index)
            .map(VideoSegment::duration_secs)
            .sum::<f64>();
        let segment = &mut segments[index];
        let (inserted_at_secs, inserted_duration_secs) = match edge {
            VideoSegmentEdge::Start => {
                let inserted_duration_secs =
                    (segment.source_start_secs - self.source_start_secs).max(0.0);
                if inserted_duration_secs <= f64::EPSILON {
                    return;
                }
                segment.source_start_secs = self.source_start_secs;
                (timeline_start_secs, inserted_duration_secs)
            }
            VideoSegmentEdge::End => {
                let inserted_at_secs = timeline_start_secs + segment.duration_secs();
                let inserted_duration_secs =
                    (self.source_end_secs - segment.source_end_secs).max(0.0);
                if inserted_duration_secs <= f64::EPSILON {
                    return;
                }
                segment.source_end_secs = self.source_end_secs;
                (inserted_at_secs, inserted_duration_secs)
            }
        };

        ripple_insert_ranges(
            &mut self.bundle.zoom_segments,
            inserted_at_secs,
            inserted_duration_secs,
        );
        if self.current_time_secs >= inserted_at_secs {
            self.current_time_secs += inserted_duration_secs;
        }
        self.duration_secs = video_timeline_duration(&self.bundle);
        let mut target_view_start_secs = self.timeline_view_start_secs;
        let mut target_view_duration_secs = self.timeline_view_duration_secs;
        if self.timeline_view_start_secs <= f64::EPSILON
            && self.timeline_view_duration_secs >= old_timeline_extent_secs - f64::EPSILON
        {
            target_view_start_secs = 0.0;
            target_view_duration_secs = timeline_extent_secs(self.duration_secs);
        }
        self.cursor_time_secs = None;
        self.save_bundle();
        self.request_preview();
        self.request_zoom_target();
        let playback_range = from_playback_fraction.zip(timeline_time_fraction(
            self.current_time_secs,
            target_view_start_secs,
            target_view_duration_secs,
        ));
        self.animate_video_timeline_resize(
            from_ranges,
            from_zoom_ranges,
            None,
            playback_range,
            animation_view_start_secs,
            animation_view_duration_secs,
            target_view_start_secs,
            target_view_duration_secs,
            cx,
        );
    }

    fn add_zoom(&mut self, start_secs: f64, end_secs: f64, cx: &mut Context<Self>) {
        if end_secs <= start_secs {
            return;
        }
        let id = self
            .bundle
            .zoom_segments
            .iter()
            .map(|segment| segment.id)
            .max()
            .unwrap_or(0)
            + 1;
        self.bundle.zoom_segments.push(ZoomSegment {
            id,
            start_secs,
            end_secs,
            target: [0.5, 0.5],
            amount: 2.0,
            transition: ZoomTransitionSpeed::Medium,
        });
        self.selected_zoom = Some(id);
        self.selected_video_segment = None;
        self.zoom_hover_range = None;
        self.current_screen_frame = None;
        self.save_bundle();
        self.request_preview();
        self.request_zoom_target();
        cx.notify();
    }

    fn begin_zoom_segment_drag(
        &mut self,
        id: u64,
        kind: ZoomSegmentDragKind,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(segment) = self
            .bundle
            .zoom_segments
            .iter()
            .find(|segment| segment.id == id)
        else {
            return;
        };
        let start_secs = segment.start_secs;
        let end_secs = segment.end_secs;
        let minimum_start_secs = self
            .bundle
            .zoom_segments
            .iter()
            .filter(|segment| segment.id != id && segment.end_secs <= start_secs)
            .map(|segment| segment.end_secs)
            .max_by(f64::total_cmp)
            .unwrap_or(0.0);
        let maximum_end_secs = self
            .bundle
            .zoom_segments
            .iter()
            .filter(|segment| segment.id != id && segment.start_secs >= end_secs)
            .map(|segment| segment.start_secs)
            .min_by(f64::total_cmp)
            .unwrap_or(self.duration_secs);
        self.zoom_segment_drag = Some(ZoomSegmentDrag {
            id,
            kind,
            pointer_start_x: event.position.x,
            original_start_secs: start_secs,
            original_end_secs: end_secs,
            draft_start_secs: start_secs,
            draft_end_secs: end_secs,
            minimum_start_secs,
            maximum_end_secs,
        });
        if self.selected_zoom != Some(id) {
            self.current_screen_frame = None;
        }
        self.selected_zoom = Some(id);
        self.selected_video_segment = None;
        self.cursor_time_secs = None;
        self.request_preview();
        self.request_zoom_target();
        cx.notify();
    }

    fn drag_zoom_segment(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.zoom_segment_drag else {
            return false;
        };
        if !event.dragging() {
            return true;
        }
        let bounds = self.timeline_bounds.get();
        if bounds.size.width <= Pixels::ZERO {
            return true;
        }
        let delta_fraction = (event.position.x - drag.pointer_start_x) / bounds.size.width;
        let delta_secs = f64::from(delta_fraction) * self.timeline_view_duration_secs;
        let (start_secs, end_secs) = match drag.kind {
            ZoomSegmentDragKind::Move => {
                let duration_secs = (drag.original_end_secs - drag.original_start_secs).max(0.0);
                let maximum_start_secs =
                    (drag.maximum_end_secs - duration_secs).max(drag.minimum_start_secs);
                let start_secs = (drag.original_start_secs + delta_secs)
                    .clamp(drag.minimum_start_secs, maximum_start_secs);
                (start_secs, start_secs + duration_secs)
            }
            ZoomSegmentDragKind::Resize(edge) => resize_zoom_segment_range(
                edge,
                drag.original_start_secs,
                drag.original_end_secs,
                drag.minimum_start_secs,
                drag.maximum_end_secs,
                delta_secs,
            ),
        };
        let playback_time_secs = match drag.kind {
            ZoomSegmentDragKind::Move => self.timeline_time_at(event.position.x),
            ZoomSegmentDragKind::Resize(ZoomSegmentEdge::Start) => Some(start_secs),
            ZoomSegmentDragKind::Resize(ZoomSegmentEdge::End) => Some(end_secs),
        };
        let start_changed = drag.original_start_secs != start_secs;
        match drag.kind {
            ZoomSegmentDragKind::Move => {
                let Some(segment) = self
                    .bundle
                    .zoom_segments
                    .iter_mut()
                    .find(|segment| segment.id == drag.id)
                else {
                    self.zoom_segment_drag = None;
                    return true;
                };
                segment.start_secs = start_secs;
                segment.end_secs = end_secs;
            }
            ZoomSegmentDragKind::Resize(_) => {
                if let Some(drag) = self.zoom_segment_drag.as_mut() {
                    drag.draft_start_secs = start_secs;
                    drag.draft_end_secs = end_secs;
                }
            }
        }
        if let Some(time_secs) = playback_time_secs {
            self.set_playback_time(time_secs);
        }
        if start_changed && matches!(drag.kind, ZoomSegmentDragKind::Move) {
            self.request_zoom_target();
        }
        cx.notify();
        true
    }

    fn finish_zoom_segment_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.zoom_segment_drag.take() else {
            return;
        };
        if matches!(drag.kind, ZoomSegmentDragKind::Resize(_))
            && (drag.draft_start_secs != drag.original_start_secs
                || drag.draft_end_secs != drag.original_end_secs)
        {
            let Some(segment) = self
                .bundle
                .zoom_segments
                .iter_mut()
                .find(|segment| segment.id == drag.id)
            else {
                return;
            };
            segment.start_secs = drag.draft_start_secs;
            segment.end_secs = drag.draft_end_secs;
            self.request_preview();
            self.request_zoom_target();
        }
        self.save_bundle();
        cx.notify();
    }

    fn set_zoom_amount(&mut self, amount: f32, cx: &mut Context<Self>) {
        let amount = amount.clamp(1.0, 5.0);
        if let Some(segment) = self.selected_zoom_mut() {
            if segment.amount == amount {
                return;
            }
            segment.amount = amount;
            self.request_preview();
            cx.notify();
        }
    }

    fn set_zoom_transition(&mut self, transition: ZoomTransitionSpeed, cx: &mut Context<Self>) {
        if let Some(segment) = self.selected_zoom_mut() {
            segment.transition = transition;
            self.save_bundle();
            self.request_preview();
            cx.notify();
        }
    }

    fn set_zoom_target(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.zoom_target_bounds.get();
        if bounds.size.width <= Pixels::ZERO || bounds.size.height <= Pixels::ZERO {
            return;
        }
        let target = [
            ((position.x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0),
            ((position.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0),
        ];
        if let Some(segment) = self.selected_zoom_mut() {
            segment.target = target;
            self.request_preview();
            cx.notify();
        }
    }

    fn open_crop_dialog(&mut self, cx: &mut Context<Self>) {
        self.stop_playback();
        self.close_aspect_ratio_dropdown(cx);
        self.crop_draft = self.bundle.screen_crop.unwrap_or(ScreenCrop::FULL);
        self.crop_drag = None;
        self.crop_dialog_open = true;
        self.request_preview();
        cx.notify();
    }

    fn toggle_aspect_ratio_dropdown(&mut self, cx: &mut Context<Self>) {
        if self.aspect_ratio_dropdown_open {
            self.close_aspect_ratio_dropdown(cx);
            return;
        }
        self.aspect_ratio_dropdown_transition =
            self.aspect_ratio_dropdown_transition.saturating_add(1);
        self.aspect_ratio_dropdown_visible = true;
        self.aspect_ratio_dropdown_open = true;
        cx.notify();
    }

    fn close_aspect_ratio_dropdown(&mut self, cx: &mut Context<Self>) {
        if !self.aspect_ratio_dropdown_visible || !self.aspect_ratio_dropdown_open {
            return;
        }
        self.aspect_ratio_dropdown_transition =
            self.aspect_ratio_dropdown_transition.saturating_add(1);
        let transition = self.aspect_ratio_dropdown_transition;
        self.aspect_ratio_dropdown_open = false;
        let editor = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(DROPDOWN_ANIMATION_DURATION)
                .await;
            let _ = editor.update(cx, |editor, cx| {
                if !editor.aspect_ratio_dropdown_open
                    && editor.aspect_ratio_dropdown_transition == transition
                {
                    editor.aspect_ratio_dropdown_visible = false;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn crop_position_at(&self, position: gpui::Point<Pixels>) -> Option<[f32; 2]> {
        let bounds = self.crop_preview_bounds.get();
        if bounds.size.width <= Pixels::ZERO || bounds.size.height <= Pixels::ZERO {
            return None;
        }
        Some([
            ((position.x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0),
            ((position.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0),
        ])
    }

    fn begin_screen_crop(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(start) = self.crop_position_at(position) else {
            return;
        };
        let original = self.crop_draft;
        self.crop_drag = Some(
            if let Some(handle) =
                crop_handle_at(position, self.crop_preview_bounds.get(), self.crop_draft)
            {
                CropDrag::Resize { handle, original }
            } else if crop_contains(self.crop_draft, start) {
                CropDrag::Move {
                    pointer_start: start,
                    original,
                }
            } else {
                self.crop_draft = ScreenCrop {
                    position: start,
                    size: [0.01_f32.min(1.0 - start[0]), 0.01_f32.min(1.0 - start[1])],
                };
                CropDrag::Create { start }
            },
        );
        cx.notify();
    }

    fn drag_screen_crop(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(operation) = self.crop_drag else {
            return false;
        };
        let Some(current) = self.crop_position_at(position) else {
            return true;
        };
        self.crop_draft = match operation {
            CropDrag::Create { start } => ScreenCrop {
                position: [start[0].min(current[0]), start[1].min(current[1])],
                size: [
                    (start[0] - current[0]).abs().max(0.01),
                    (start[1] - current[1]).abs().max(0.01),
                ],
            },
            CropDrag::Move {
                pointer_start,
                original,
            } => ScreenCrop {
                position: [
                    (original.position[0] + current[0] - pointer_start[0])
                        .clamp(0.0, 1.0 - original.size[0]),
                    (original.position[1] + current[1] - pointer_start[1])
                        .clamp(0.0, 1.0 - original.size[1]),
                ],
                size: original.size,
            },
            CropDrag::Resize { handle, original } => resized_screen_crop(original, handle, current),
        };
        self.crop_draft = normalize_screen_crop(Some(self.crop_draft)).unwrap_or(ScreenCrop::FULL);
        cx.notify();
        true
    }

    fn apply_screen_crop(&mut self, cx: &mut Context<Self>) {
        self.bundle.screen_crop = normalize_screen_crop(Some(self.crop_draft));
        self.crop_dialog_open = false;
        self.crop_drag = None;
        self.save_bundle();
        self.request_preview();
        cx.notify();
    }

    fn close_crop_dialog(&mut self, cx: &mut Context<Self>) {
        self.crop_dialog_open = false;
        self.crop_drag = None;
        cx.notify();
    }

    fn set_playback_time(&mut self, time_secs: f64) {
        self.stop_playback();
        self.current_time_secs = time_secs.clamp(0.0, self.duration_secs);
        self.request_preview();
    }

    fn stop_playback(&mut self) {
        self.is_playing = false;
        self.playback_started_at = None;
        self.playback_generation = self.playback_generation.wrapping_add(1);
        self.last_playback_preview_time_secs = None;
        self.audio_playback = None;
    }

    fn toggle_playback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_playing {
            self.stop_playback();
            cx.notify();
            return;
        }
        if self.duration_secs <= 0.0 {
            return;
        }

        if let Some(cursor_time_secs) = self.cursor_time_secs {
            self.current_time_secs = cursor_time_secs.clamp(0.0, self.duration_secs);
        } else if self.current_time_secs >= self.duration_secs {
            self.current_time_secs = 0.0;
        }
        self.cursor_time_secs = None;
        self.zoom_hover_range = None;
        self.is_playing = true;
        self.playback_generation = self.playback_generation.wrapping_add(1);
        self.last_playback_preview_time_secs = None;
        let generation = self.playback_generation;
        self.playback_started_at = Some((Instant::now(), self.current_time_secs));
        let mixer = audio_mixer(&self.audio_sources, &self.bundle);
        if mixer.has_sources() {
            match blip_audio::AudioPlayback::start(mixer, self.current_time_secs) {
                Ok(playback) => self.audio_playback = Some(playback),
                Err(error) => tracing::error!(error, "Failed to start preview audio"),
            }
        }
        self.request_preview();
        cx.notify();
        self.schedule_playback_frame(generation, window, cx);
    }

    fn schedule_playback_frame(
        &mut self,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.on_next_frame(window, move |editor, window, cx| {
            if !editor.is_playing || editor.playback_generation != generation {
                return;
            }
            let Some((started_at, start_time_secs)) = editor.playback_started_at else {
                return;
            };
            let (time_secs, finished) = playback_position(
                start_time_secs,
                started_at.elapsed().as_secs_f64(),
                editor.duration_secs,
            );
            editor.current_time_secs = time_secs;
            editor.request_preview();
            if finished {
                editor.stop_playback();
            } else {
                editor.schedule_playback_frame(generation, window, cx);
            }
            cx.notify();
        });
    }

    fn timeline_time_at(&self, x: Pixels) -> Option<f64> {
        let bounds = self.timeline_bounds.get();
        if bounds.size.width <= Pixels::ZERO {
            return None;
        }
        let fraction = ((x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0);
        Some(self.timeline_view_start_secs + f64::from(fraction) * self.timeline_view_duration_secs)
    }

    fn zoom_timeline(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let bounds = self.timeline_bounds.get();
        if self.duration_secs <= 0.0
            || bounds.size.width <= Pixels::ZERO
            || event.position.x < bounds.left()
            || event.position.x > bounds.right()
        {
            return;
        }

        let focal_fraction =
            f64::from((event.position.x - bounds.left()) / bounds.size.width).clamp(0.0, 1.0);
        let (start_secs, duration_secs) = zoomed_timeline_view(
            self.timeline_view_start_secs,
            self.timeline_view_duration_secs,
            timeline_extent_secs(self.duration_secs),
            focal_fraction,
            event.delta,
        );
        if start_secs != self.timeline_view_start_secs
            || duration_secs != self.timeline_view_duration_secs
        {
            self.video_timeline_resize_generation += 1;
            self.video_timeline_resize_animation = None;
            self.timeline_view_start_secs = start_secs;
            self.timeline_view_duration_secs = duration_secs;
            self.cursor_time_secs = Some(start_secs + focal_fraction * duration_secs);
            self.request_preview();
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn pan_timeline(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let bounds = self.timeline_bounds.get();
        if !event.delta.precise()
            || bounds.size.width <= Pixels::ZERO
            || event.position.x < bounds.left()
            || event.position.x > bounds.right()
        {
            return;
        }

        let delta = event.delta.pixel_delta(px(1.0));
        let start_secs = panned_timeline_view(
            self.timeline_view_start_secs,
            self.timeline_view_duration_secs,
            timeline_extent_secs(self.duration_secs),
            f64::from(delta.x / bounds.size.width),
        );
        if start_secs != self.timeline_view_start_secs {
            self.video_timeline_resize_generation += 1;
            self.video_timeline_resize_animation = None;
            self.timeline_view_start_secs = start_secs;
            self.cursor_time_secs = self.timeline_time_at(event.position.x);
            self.request_preview();
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn update_timeline_hover(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        if self.is_playing {
            return;
        }
        let timeline = self.timeline_bounds.get();
        let rows = self.timeline_rows_bounds.get();
        let hovering = self.zoom_segment_drag.is_none()
            && self.video_segment_drag.is_none()
            && timeline.size.width > Pixels::ZERO
            && rows.size.height > Pixels::ZERO
            && position.x >= timeline.origin.x
            && position.x <= timeline.origin.x + timeline.size.width
            && position.y >= rows.origin.y
            && position.y <= rows.origin.y + rows.size.height;
        let cursor_time_secs = hovering.then(|| {
            self.timeline_view_start_secs
                + f64::from((position.x - timeline.origin.x) / timeline.size.width)
                    * self.timeline_view_duration_secs
        });
        let zoom_hover_range = cursor_time_secs.and_then(|time_secs| {
            (position.y >= timeline.top()
                && position.y <= timeline.bottom()
                && !self.bundle.zoom_segments.iter().any(|segment| {
                    time_secs >= segment.start_secs && time_secs <= segment.end_secs
                }))
            .then(|| {
                zoom_segment_range_at(time_secs, self.duration_secs, &self.bundle.zoom_segments)
            })
            .flatten()
        });
        if self.cursor_time_secs != cursor_time_secs || self.zoom_hover_range != zoom_hover_range {
            self.cursor_time_secs = cursor_time_secs;
            self.zoom_hover_range = zoom_hover_range;
            self.request_preview();
            cx.notify();
        }
    }

    fn request_preview(&self) {
        let time_secs = self.cursor_time_secs.unwrap_or(self.current_time_secs);
        let Some(source_time_secs) = source_time_at(&self.bundle, time_secs) else {
            return;
        };
        let _ = self.preview_requests.force_send(PreviewRequest {
            playback_generation: self.playback_generation,
            timeline_time_secs: time_secs,
            time_secs: source_time_secs,
            background_type: self.bundle.appearance.background_type,
            background_preset: self.background_preset,
            padding: self.bundle.appearance.padding,
            aspect_ratio: self.bundle.output_aspect_ratio,
            border_radius: self.bundle.appearance.border_radius,
            shadow: self.bundle.appearance.shadow,
            zoom: zoom_transform_at(&self.bundle.zoom_segments, time_secs),
            camera_layout: self.bundle.camera_layout,
            screen_crop: self.bundle.screen_crop,
        });
    }

    fn request_zoom_target(&self) {
        if let Some(segment) = self.selected_zoom() {
            let time_secs =
                source_time_at(&self.bundle, segment.start_secs).unwrap_or(segment.start_secs);
            let _ = self.zoom_target_requests.force_send(time_secs);
        }
    }

    fn set_slider_value(&mut self, kind: SliderKind, position: Pixels, cx: &mut Context<Self>) {
        let bounds = match kind {
            SliderKind::Padding => self.padding_slider_bounds.get(),
            SliderKind::Radius => self.radius_slider_bounds.get(),
            SliderKind::Shadow => self.shadow_slider_bounds.get(),
            SliderKind::CameraSize => self.camera_size_slider_bounds.get(),
            SliderKind::CameraPadding => self.camera_padding_slider_bounds.get(),
            SliderKind::CameraZoomReduction => self.camera_zoom_reduction_slider_bounds.get(),
            SliderKind::CameraShadow => self.camera_shadow_slider_bounds.get(),
            SliderKind::ZoomAmount => self.zoom_amount_slider_bounds.get(),
            SliderKind::TimelineZoom => self.timeline_zoom_slider_bounds.get(),
        };
        if bounds.size.width <= Pixels::ZERO {
            return;
        }
        let fraction = ((position - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0);
        match kind {
            SliderKind::Padding => self.bundle.appearance.padding = (fraction * 50.0).round(),
            SliderKind::Radius => self.bundle.appearance.border_radius = (fraction * 50.0).round(),
            SliderKind::Shadow => self.bundle.appearance.shadow = (fraction * 50.0).round(),
            SliderKind::CameraSize => self.bundle.camera_layout.size = (fraction * 50.0).round(),
            SliderKind::CameraPadding => {
                self.bundle.camera_layout.edge_padding = (fraction * 50.0).round();
            }
            SliderKind::CameraZoomReduction => {
                self.bundle.camera_layout.zoom_size_reduction = (fraction * 50.0).round();
            }
            SliderKind::CameraShadow => {
                self.bundle.camera_layout.shadow = (fraction * 50.0).round();
            }
            SliderKind::ZoomAmount => {
                self.set_zoom_amount(1.0 + (fraction * 40.0).round() / 10.0, cx);
                return;
            }
            SliderKind::TimelineZoom => {
                let timeline_duration_secs = timeline_extent_secs(self.duration_secs);
                let zoom = 1.0 + f64::from(fraction) * 9.0;
                let view_duration_secs = timeline_duration_secs / zoom;
                let view_center_secs =
                    self.timeline_view_start_secs + self.timeline_view_duration_secs * 0.5;
                self.timeline_view_duration_secs = view_duration_secs;
                self.timeline_view_start_secs = clamp_timeline_view_start(
                    view_center_secs - view_duration_secs * 0.5,
                    view_duration_secs,
                    timeline_duration_secs,
                );
                self.video_timeline_resize_generation += 1;
                self.video_timeline_resize_animation = None;
                cx.notify();
                return;
            }
        }
        self.request_preview();
        cx.notify();
    }

    fn drag_slider(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.dragging() && self.drag_screen_crop(event.position, cx) {
            return;
        }
        if self.drag_video_segment(event, cx) {
            return;
        }
        if self.drag_zoom_segment(event, cx) {
            return;
        }
        if event.dragging() && self.dragging_zoom_target {
            self.set_zoom_target(event.position, cx);
            return;
        }
        if event.dragging()
            && let Some(kind) = self.active_slider
        {
            self.set_slider_value(kind, event.position.x, cx);
        }
    }

    fn percentage_slider(
        &self,
        label: &'static str,
        value: f32,
        kind: SliderKind,
        bounds: Rc<Cell<Bounds<Pixels>>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let measured_bounds = Rc::clone(&bounds);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(div().text_color(rgb(theme::TEXT_MUTED)).child(label))
                    .child(
                        div()
                            .text_color(rgb(theme::TEXT))
                            .child(format!("{value:.0}%")),
                    ),
            )
            .child(
                div()
                    .id(format!("background-slider-{label}"))
                    .h(px(18.0))
                    .relative()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                            editor.active_slider = Some(kind);
                            editor.set_slider_value(kind, event.position.x, cx);
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                measured_bounds.set(bounds);
                                bounds
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(7.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::CONTROL_BACKGROUND)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(7.0))
                            .w(relative(value / 50.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(value / 50.0))
                            .top(px(3.0))
                            .ml(px(-6.0))
                            .size(px(12.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT))
                            .border_1()
                            .border_color(rgb(theme::APP_BACKGROUND)),
                    ),
            )
    }

    fn background_option(
        &self,
        label: &'static str,
        value: BackgroundType,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.bundle.appearance.background_type == value;
        div()
            .id(format!("background-type-{label}"))
            .flex_1()
            .h(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .bg(rgb(if selected {
                theme::CONTROL_ACTIVE
            } else {
                theme::CONTROL_BACKGROUND
            }))
            .border_1()
            .border_color(rgb(if selected {
                theme::BORDER
            } else {
                theme::BORDER_SUBTLE
            }))
            .hover(|option| option.bg(rgb(theme::CONTROL_HOVER)))
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(if selected {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            }))
            .on_click(cx.listener(move |editor, _, _, cx| {
                editor.bundle.appearance.background_type = value;
                editor.save_bundle();
                editor.request_preview();
                cx.notify();
            }))
            .child(label)
    }

    fn background_preset(&self, index: usize, path: PathBuf, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.bundle.appearance.background_type == BackgroundType::Image
            && self.background_preset == index;
        let name = self.background_images[index].name.clone();
        div()
            .id(format!("background-preset-{index}"))
            .size(px(43.0))
            .rounded_md()
            .overflow_hidden()
            .bg(rgb(theme::CONTROL_BACKGROUND))
            .border_2()
            .border_color(rgb(if selected {
                theme::TEXT
            } else {
                theme::BORDER_SUBTLE
            }))
            .cursor_pointer()
            .on_click(cx.listener(move |editor, _, _, cx| {
                editor.bundle.appearance.background_type = BackgroundType::Image;
                editor.bundle.appearance.background_image = name.clone();
                editor.background_preset = index;
                editor.save_bundle();
                editor.request_preview();
                cx.notify();
            }))
            .child(img(path).size_full().object_fit(ObjectFit::Cover))
            .into_any_element()
    }

    fn background_presets(&self, cx: &mut Context<Self>) -> Div {
        let mut rows = Vec::new();
        for (row_index, backgrounds) in self.background_images.chunks(5).enumerate() {
            let mut row = div().flex().gap_1();
            for (column_index, background) in backgrounds.iter().enumerate() {
                let index = row_index * 5 + column_index;
                row = row.child(self.background_preset(index, background.thumbnail.clone(), cx));
            }
            rows.push(row);
        }
        div().flex().flex_col().gap_1().children(rows)
    }

    fn zoom_value_option(
        &self,
        label: &'static str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut BundleEditor, &mut Context<BundleEditor>) + 'static,
    ) -> AnyElement {
        div()
            .id(format!("zoom-option-{label}"))
            .flex_1()
            .h(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .bg(rgb(if selected {
                theme::CONTROL_ACTIVE
            } else {
                theme::CONTROL_BACKGROUND
            }))
            .border_1()
            .border_color(rgb(if selected {
                theme::SELECTION
            } else {
                theme::BORDER_SUBTLE
            }))
            .hover(|option| option.bg(rgb(theme::CONTROL_HOVER)))
            .cursor_pointer()
            .text_xs()
            .on_click(cx.listener(move |editor, _, _, cx| on_click(editor, cx)))
            .child(label)
            .into_any_element()
    }

    fn camera_position_option(&self, value: CameraPosition, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.bundle.camera_layout.position == value;
        div()
            .id(format!("camera-position-{value:?}"))
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .hover(|option| option.bg(rgb(theme::CONTROL_HOVER)))
            .cursor_pointer()
            .on_click(cx.listener(move |editor, _, _, cx| {
                editor.bundle.camera_layout.position = value;
                editor.save_bundle();
                editor.request_preview();
                cx.notify();
            }))
            .child(
                div()
                    .size(px(if selected { 10.0 } else { 7.0 }))
                    .rounded_full()
                    .bg(rgb(if selected {
                        theme::SELECTION
                    } else {
                        theme::TEXT_MUTED
                    })),
            )
            .into_any_element()
    }

    fn camera_crop_option(
        &self,
        label: &'static str,
        value: CameraCrop,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.zoom_value_option(
            label,
            self.bundle.camera_layout.crop == value,
            cx,
            move |editor, cx| {
                editor.bundle.camera_layout.crop = value;
                editor.save_bundle();
                editor.request_preview();
                cx.notify();
            },
        )
    }

    fn zoom_amount_slider(&self, amount: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let measured_bounds = Rc::clone(&self.zoom_amount_slider_bounds);
        let fraction = (amount.clamp(1.0, 5.0) - 1.0) / 4.0;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(div().text_color(rgb(theme::TEXT_MUTED)).child("Amount"))
                    .child(
                        div()
                            .text_color(rgb(theme::TEXT))
                            .child(format!("{amount:.1}x")),
                    ),
            )
            .child(
                div()
                    .id("zoom-amount-slider")
                    .h(px(18.0))
                    .relative()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                            editor.active_slider = Some(SliderKind::ZoomAmount);
                            editor.set_slider_value(SliderKind::ZoomAmount, event.position.x, cx);
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                measured_bounds.set(bounds);
                                bounds
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(7.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::CONTROL_BACKGROUND)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(7.0))
                            .w(relative(fraction))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(fraction))
                            .top(px(3.0))
                            .ml(px(-6.0))
                            .size(px(12.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT))
                            .border_1()
                            .border_color(rgb(theme::APP_BACKGROUND)),
                    ),
            )
    }

    fn timeline_zoom_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let measured_bounds = Rc::clone(&self.timeline_zoom_slider_bounds);
        let timeline_duration_secs = timeline_extent_secs(self.duration_secs);
        let zoom = if self.timeline_view_duration_secs > 0.0 {
            (timeline_duration_secs / self.timeline_view_duration_secs).clamp(1.0, 10.0)
        } else {
            1.0
        };
        let fraction = ((zoom - 1.0) / 9.0) as f32;
        let timeline_width = f64::from(self.timeline_bounds.get().size.width);
        let density_label = if timeline_width > 0.0 {
            let seconds_per_hundred_pixels =
                self.timeline_view_duration_secs / timeline_width * 100.0;
            if seconds_per_hundred_pixels >= 10.0 {
                format!("{seconds_per_hundred_pixels:.0}s")
            } else if seconds_per_hundred_pixels >= 1.0 {
                format!("{seconds_per_hundred_pixels:.1}s")
            } else {
                format!("{seconds_per_hundred_pixels:.2}s")
            }
        } else {
            "--s".to_owned()
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("timeline-zoom-slider")
                    .w(px(96.0))
                    .h(px(18.0))
                    .relative()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                            editor.active_slider = Some(SliderKind::TimelineZoom);
                            editor.set_slider_value(SliderKind::TimelineZoom, event.position.x, cx);
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                measured_bounds.set(bounds);
                                bounds
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(7.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::CONTROL_BACKGROUND)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(7.0))
                            .w(relative(fraction))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT_MUTED)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(fraction))
                            .top(px(3.0))
                            .ml(px(-6.0))
                            .size(px(12.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT))
                            .border_1()
                            .border_color(rgb(theme::APP_BACKGROUND)),
                    ),
            )
            .child(
                div()
                    .w(px(34.0))
                    .text_xs()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(density_label),
            )
    }

    fn zoom_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let segment = self.selected_zoom().expect("selected zoom should exist");
        let target = segment.target;
        let amount = segment.amount;
        let transition = segment.transition;
        let aspect_ratio = self
            .current_screen_frame
            .as_ref()
            .map(|frame| frame.get_width() as f32 / frame.get_height() as f32)
            .unwrap_or(16.0 / 9.0);
        let measured_bounds = Rc::clone(&self.zoom_target_bounds);
        let target_preview = div()
            .id("zoom-target-preview")
            .w_full()
            .aspect_ratio(aspect_ratio)
            .relative()
            .overflow_hidden()
            .rounded_md()
            .bg(rgb(theme::CANVAS_BACKGROUND))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .cursor_crosshair()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                    editor.dragging_zoom_target = true;
                    editor.set_zoom_target(event.position, cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, _, _| {
                        measured_bounds.set(bounds);
                        bounds
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when_some(self.current_screen_frame.clone(), |preview, frame| {
                preview.child(surface(frame).size_full().object_fit(ObjectFit::Contain))
            })
            .child(
                div()
                    .absolute()
                    .left(relative(target[0]))
                    .top(relative(target[1]))
                    .ml(px(-8.0))
                    .mt(px(-8.0))
                    .size(px(16.0))
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(theme::TEXT))
                    .bg(rgba(0xff4f_5860))
                    .child(
                        div()
                            .absolute()
                            .left(px(5.0))
                            .top(px(5.0))
                            .size(px(2.0))
                            .rounded_full()
                            .bg(rgb(theme::TEXT)),
                    ),
            );

        div()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .p_4()
            .flex()
            .flex_col()
            .gap_6()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_l_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Zoom"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Target"),
                    )
                    .child(target_preview)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_DIM))
                            .child("Click or drag to position the zoom."),
                    ),
            )
            .child(self.zoom_amount_slider(amount, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Transition"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(self.zoom_value_option(
                                "Slow",
                                transition == ZoomTransitionSpeed::Slow,
                                cx,
                                |editor, cx| {
                                    editor.set_zoom_transition(ZoomTransitionSpeed::Slow, cx);
                                },
                            ))
                            .child(self.zoom_value_option(
                                "Medium",
                                transition == ZoomTransitionSpeed::Medium,
                                cx,
                                |editor, cx| {
                                    editor.set_zoom_transition(ZoomTransitionSpeed::Medium, cx);
                                },
                            ))
                            .child(self.zoom_value_option(
                                "Fast",
                                transition == ZoomTransitionSpeed::Fast,
                                cx,
                                |editor, cx| {
                                    editor.set_zoom_transition(ZoomTransitionSpeed::Fast, cx);
                                },
                            )),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("delete-zoom-segment")
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(rgba(0xff4f_5820))
                    .border_1()
                    .border_color(rgb(ACCENT))
                    .hover(|button| button.bg(rgba(0xff4f_5838)))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(theme::TEXT))
                    .on_click(cx.listener(|editor, _, _, cx| {
                        editor.delete_selected(cx);
                    }))
                    .child("Delete Segment"),
            )
    }

    fn video_segment_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let segment = self
            .selected_video_segment()
            .expect("selected video segment should exist");
        let can_delete = self.can_delete_video_segment();
        let source_start = segment.source_start_secs;
        let source_end = segment.source_end_secs;
        let duration = segment.duration_secs();

        div()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .p_4()
            .flex()
            .flex_col()
            .gap_6()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_l_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Video Segment"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(segment_detail("Source start", source_start))
                    .child(segment_detail("Source end", source_end))
                    .child(segment_detail("Duration", duration)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("delete-video-segment")
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(rgba(0xff4f_5820))
                    .border_1()
                    .border_color(rgb(ACCENT))
                    .opacity(if can_delete { 1.0 } else { 0.55 })
                    .text_xs()
                    .text_color(rgb(theme::TEXT))
                    .when(can_delete, |button| {
                        button
                            .hover(|button| button.bg(rgba(0xff4f_5838)))
                            .cursor_pointer()
                            .on_click(cx.listener(|editor, _, _, cx| {
                                editor.delete_selected_video_segment(cx);
                            }))
                    })
                    .child("Delete Segment"),
            )
    }

    fn sidebar_tab(
        &self,
        label: &'static str,
        tab: SidebarTab,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let selected = self.sidebar_tab == tab;
        div()
            .id(format!("sidebar-tab-{label}"))
            .flex_1()
            .h(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .bg(rgb(if selected {
                theme::CONTROL_ACTIVE
            } else {
                theme::PANEL_BACKGROUND
            }))
            .text_xs()
            .text_color(rgb(if selected {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            }))
            .hover(|tab| tab.bg(rgb(theme::CONTROL_HOVER)))
            .cursor_pointer()
            .on_click(cx.listener(move |editor, _, _, cx| {
                editor.sidebar_tab = tab;
                cx.notify();
            }))
            .child(label)
    }

    fn screen_sidebar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Background"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Type"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(self.background_option("Color", BackgroundType::Color, cx))
                            .child(self.background_option("Image", BackgroundType::Image, cx))
                            .child(self.background_option(
                                "Gradient",
                                BackgroundType::Gradient,
                                cx,
                            )),
                    )
                    .when(
                        self.bundle.appearance.background_type == BackgroundType::Image,
                        |section| section.child(self.background_presets(cx)),
                    ),
            )
            .child(self.percentage_slider(
                "Padding",
                self.bundle.appearance.padding,
                SliderKind::Padding,
                Rc::clone(&self.padding_slider_bounds),
                cx,
            ))
            .child(self.percentage_slider(
                "Border radius",
                self.bundle.appearance.border_radius,
                SliderKind::Radius,
                Rc::clone(&self.radius_slider_bounds),
                cx,
            ))
            .child(self.percentage_slider(
                "Shadow",
                self.bundle.appearance.shadow,
                SliderKind::Shadow,
                Rc::clone(&self.shadow_slider_bounds),
                cx,
            ))
    }

    fn camera_sidebar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.percentage_slider(
                "Size",
                self.bundle.camera_layout.size,
                SliderKind::CameraSize,
                Rc::clone(&self.camera_size_slider_bounds),
                cx,
            ))
            .child(self.percentage_slider(
                "Edge padding",
                self.bundle.camera_layout.edge_padding,
                SliderKind::CameraPadding,
                Rc::clone(&self.camera_padding_slider_bounds),
                cx,
            ))
            .child(self.percentage_slider(
                "Zoom reduction",
                self.bundle.camera_layout.zoom_size_reduction,
                SliderKind::CameraZoomReduction,
                Rc::clone(&self.camera_zoom_reduction_slider_bounds),
                cx,
            ))
            .child(self.percentage_slider(
                "Shadow",
                self.bundle.camera_layout.shadow,
                SliderKind::CameraShadow,
                Rc::clone(&self.camera_shadow_slider_bounds),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Shape"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(self.camera_crop_option("Circle", CameraCrop::Circle, cx))
                            .child(self.camera_crop_option("Square", CameraCrop::Squircle, cx))
                            .child(self.camera_crop_option("Full", CameraCrop::Squirectangle, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Position"),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(96.0))
                            .flex()
                            .flex_col()
                            .p_1()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(theme::BORDER_SUBTLE))
                            .bg(rgb(theme::CONTROL_BACKGROUND))
                            .children(
                                [
                                    [
                                        CameraPosition::TopLeft,
                                        CameraPosition::TopCenter,
                                        CameraPosition::TopRight,
                                    ],
                                    [
                                        CameraPosition::MiddleLeft,
                                        CameraPosition::Center,
                                        CameraPosition::MiddleRight,
                                    ],
                                    [
                                        CameraPosition::BottomLeft,
                                        CameraPosition::BottomCenter,
                                        CameraPosition::BottomRight,
                                    ],
                                ]
                                .map(|row| {
                                    div().flex_1().flex().children(
                                        row.map(|position| {
                                            self.camera_position_option(position, cx)
                                        }),
                                    )
                                }),
                            ),
                    ),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.selected_zoom().is_some() {
            return self.zoom_sidebar(cx).into_any_element();
        }
        if self.selected_video_segment().is_some() {
            return self.video_segment_sidebar(cx).into_any_element();
        }
        let has_camera = self
            .bundle
            .inputs
            .iter()
            .enumerate()
            .any(|(index, input)| input_is_camera(input, index));
        let active_tab = if !has_camera && self.sidebar_tab == SidebarTab::Camera {
            SidebarTab::Screen
        } else {
            self.sidebar_tab
        };
        let content = match active_tab {
            SidebarTab::Screen => self.screen_sidebar(cx).into_any_element(),
            SidebarTab::Camera => self.camera_sidebar(cx).into_any_element(),
        };

        div()
            .id("editor-sidebar")
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_l_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .child(
                div()
                    .p_2()
                    .flex()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(theme::BORDER_SUBTLE))
                    .child(self.sidebar_tab("Screen", SidebarTab::Screen, cx))
                    .when(has_camera, |tabs| {
                        tabs.child(self.sidebar_tab("Camera", SidebarTab::Camera, cx))
                    }),
            )
            .child(
                div()
                    .id("editor-sidebar-content")
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }

    fn export_option(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut BundleEditor, &mut Context<BundleEditor>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex_1()
            .h(px(34.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(if selected {
                theme::CONTROL_ACTIVE
            } else {
                theme::CONTROL_BACKGROUND
            }))
            .border_1()
            .border_color(rgb(if selected {
                theme::SELECTION
            } else {
                theme::BORDER_SUBTLE
            }))
            .hover(|option| option.bg(rgb(theme::CONTROL_HOVER)))
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(if selected {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            }))
            .on_click(cx.listener(move |editor, _, _, cx| on_click(editor, cx)))
            .child(label)
            .into_any_element()
    }

    fn aspect_ratio_dropdown_option(
        &self,
        label: &'static str,
        value: OutputAspectRatio,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let selected = self.bundle.output_aspect_ratio == value;
        dropdown_option(
            format!("aspect-ratio-dropdown-{label}"),
            selected,
            Self::aspect_ratio_dropdown_style(),
        )
        .justify_between()
        .on_click(cx.listener(move |editor, _, _, cx| {
            editor.bundle.output_aspect_ratio = value;
            editor.close_aspect_ratio_dropdown(cx);
            editor.save_bundle();
            editor.request_preview();
            cx.notify();
        }))
        .child(label)
        .when(selected, |item| {
            item.child(
                svg()
                    .size(px(14.0))
                    .path(CHECK)
                    .text_color(rgb(theme::TEXT_MUTED)),
            )
        })
    }

    fn aspect_ratio_dropdown_style() -> DropdownStyle {
        DropdownStyle {
            control_background: theme::CONTROL_BACKGROUND,
            control_hover: theme::CONTROL_HOVER,
            control_active: theme::CONTROL_ACTIVE,
            border_subtle: theme::BORDER_SUBTLE,
            border: theme::BORDER,
            text_muted: theme::TEXT_MUTED,
            trigger_full_width: true,
            trigger_height: px(26.0),
            menu_top: px(30.0),
            option_height: px(24.0),
            menu_shadow: true,
            ..DropdownStyle::default()
        }
    }

    fn aspect_ratio_dropdown(&self, cx: &mut Context<Self>) -> Div {
        let current = match self.bundle.output_aspect_ratio {
            OutputAspectRatio::Auto => "Auto",
            OutputAspectRatio::Wide => "16:9",
            OutputAspectRatio::Vertical => "9:16",
            OutputAspectRatio::Square => "1:1",
            OutputAspectRatio::Classic => "4:3",
            OutputAspectRatio::Tall => "3:4",
        };
        let style = Self::aspect_ratio_dropdown_style();
        let opening = self.aspect_ratio_dropdown_open;
        let transition = self.aspect_ratio_dropdown_transition;
        let trigger = dropdown_trigger("aspect-ratio-dropdown", style)
            .px_3()
            .rounded_md()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_color(rgb(theme::BORDER))
            .shadow_sm()
            .text_xs()
            .on_click(cx.listener(|editor, _, _, cx| {
                editor.toggle_aspect_ratio_dropdown(cx);
            }))
            .child(
                svg()
                    .size(px(14.0))
                    .path(SHAPE_FRAME)
                    .text_color(rgb(theme::TEXT_MUTED)),
            )
            .child(div().text_color(rgb(theme::TEXT_MUTED)).child(current))
            .child(dropdown_chevron(CHEVRON_DOWN, opening, transition, style));
        let mut dropdown = div().relative().w(px(104.0)).child(trigger);
        if self.aspect_ratio_dropdown_visible {
            let menu = dropdown_menu(
                "aspect-ratio-dropdown-menu",
                opening,
                transition,
                style,
                [
                    self.aspect_ratio_dropdown_option("Auto", OutputAspectRatio::Auto, cx)
                        .into_any_element(),
                    self.aspect_ratio_dropdown_option("16:9", OutputAspectRatio::Wide, cx)
                        .into_any_element(),
                    self.aspect_ratio_dropdown_option("9:16", OutputAspectRatio::Vertical, cx)
                        .into_any_element(),
                    self.aspect_ratio_dropdown_option("1:1", OutputAspectRatio::Square, cx)
                        .into_any_element(),
                    self.aspect_ratio_dropdown_option("4:3", OutputAspectRatio::Classic, cx)
                        .into_any_element(),
                    self.aspect_ratio_dropdown_option("3:4", OutputAspectRatio::Tall, cx)
                        .into_any_element(),
                ],
            );
            dropdown = dropdown.child(deferred(menu).priority(2));
        }
        dropdown
    }

    fn settings_dialog(&self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0000_00a8))
            .child(
                div()
                    .id("editor-settings-dialog")
                    .w(px(430.0))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .rounded_lg()
                    .bg(rgb(theme::PANEL_BACKGROUND))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Editor settings"),
                            )
                            .child(
                                div()
                                    .id("close-editor-settings")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.settings_dialog_open = false;
                                        cx.notify();
                                    }))
                                    .child("Close"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Timeline"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(theme::TEXT_MUTED))
                                            .child("Resize behavior"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .child(self.zoom_value_option(
                                                "Ghost",
                                                self.bundle.video_segment_resize_mode
                                                    == VideoSegmentResizeMode::Ghost,
                                                cx,
                                                |editor, cx| {
                                                    editor.bundle.video_segment_resize_mode =
                                                        VideoSegmentResizeMode::Ghost;
                                                    editor.save_bundle();
                                                    cx.notify();
                                                },
                                            ))
                                            .child(self.zoom_value_option(
                                                "Live",
                                                self.bundle.video_segment_resize_mode
                                                    == VideoSegmentResizeMode::Live,
                                                cx,
                                                |editor, cx| {
                                                    editor.bundle.video_segment_resize_mode =
                                                        VideoSegmentResizeMode::Live;
                                                    editor.save_bundle();
                                                    cx.notify();
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(theme::TEXT_DIM))
                                            .child(
                                                "Choose whether adjacent clips move while resizing or after the edit.",
                                            ),
                                    ),
                            ),
                    ),
            )
    }

    fn crop_dialog(&self, cx: &mut Context<Self>) -> Div {
        let crop = self.crop_draft;
        let measured_bounds = Rc::clone(&self.crop_preview_bounds);
        let preview = if let Some(frame) = &self.crop_frame {
            let aspect_ratio = frame.get_width() as f32 / frame.get_height().max(1) as f32;
            let (preview_width, preview_height) = fit_dimensions(aspect_ratio, 760.0, 440.0);
            div()
                .w(px(preview_width + 20.0))
                .h(px(preview_height + 20.0))
                .flex_none()
                .relative()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                        editor.begin_screen_crop(event.position, cx);
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(10.0))
                        .top(px(10.0))
                        .w(px(preview_width))
                        .h(px(preview_height))
                        .rounded_md()
                        .cursor_crosshair()
                        .child(surface(frame.clone()).absolute().size_full())
                        .child(
                            canvas(
                                move |bounds, _, _| measured_bounds.set(bounds),
                                |_, (), _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .child(crop_mask(0.0, 0.0, 1.0, crop.position[1]))
                        .child(crop_mask(
                            0.0,
                            crop.position[1] + crop.size[1],
                            1.0,
                            1.0 - crop.position[1] - crop.size[1],
                        ))
                        .child(crop_mask(
                            0.0,
                            crop.position[1],
                            crop.position[0],
                            crop.size[1],
                        ))
                        .child(crop_mask(
                            crop.position[0] + crop.size[0],
                            crop.position[1],
                            1.0 - crop.position[0] - crop.size[0],
                            crop.size[1],
                        ))
                        .child(
                            div()
                                .absolute()
                                .left(relative(crop.position[0]))
                                .top(relative(crop.position[1]))
                                .w(relative(crop.size[0]))
                                .h(relative(crop.size[1]))
                                .border_2()
                                .border_color(rgb(CROP_SELECTION))
                                .cursor(CursorStyle::OpenHand),
                        ),
                )
                .children(crop_resize_handles(crop, preview_width, preview_height))
        } else {
            div()
                .w_full()
                .h(px(320.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(rgb(theme::CANVAS_BACKGROUND))
                .text_sm()
                .text_color(rgb(theme::TEXT_MUTED))
                .child("Loading screen frame...")
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0000_00a8))
            .child(
                div()
                    .id("crop-dialog")
                    .w(px(820.0))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .rounded_lg()
                    .bg(rgb(theme::PANEL_BACKGROUND))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Crop screen"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(theme::TEXT_MUTED))
                                            .child("Drag to select, then move or resize the crop."),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-crop-dialog")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.close_crop_dialog(cx);
                                    }))
                                    .child("Close"),
                            ),
                    )
                    .child(div().flex().justify_center().child(preview))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                crop_dialog_button("full-crop", "Reset").on_click(cx.listener(
                                    |editor, _, _, cx| {
                                        editor.crop_draft = ScreenCrop::FULL;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(crop_dialog_button("cancel-crop", "Cancel").on_click(
                                        cx.listener(|editor, _, _, cx| {
                                            editor.close_crop_dialog(cx);
                                        }),
                                    ))
                                    .child(
                                        crop_dialog_button("apply-crop", "Apply")
                                            .bg(rgb(CROP_ACTION))
                                            .text_color(rgb(0x0017_1717))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .on_click(cx.listener(|editor, _, _, cx| {
                                                editor.apply_screen_crop(cx);
                                            })),
                                    ),
                            ),
                    ),
            )
    }

    fn export_dialog(&self, cx: &mut Context<Self>) -> Div {
        let destination = self
            .bundle
            .export_settings
            .destination
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Choose a file...")
            .to_owned();
        let can_export = self.bundle.export_settings.destination.is_some() && !self.exporting;
        let exported_path = self.exported_path.clone();

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0000_00a8))
            .child(
                div()
                    .id("export-dialog")
                    .w(px(430.0))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .rounded_lg()
                    .bg(rgb(theme::PANEL_BACKGROUND))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Export video"),
                            )
                            .child(
                                div()
                                    .id("close-export-dialog")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.export_dialog_open = false;
                                        cx.notify();
                                    }))
                                    .child("Close"),
                            ),
                    )
                    .child(export_field(
                        "Format",
                        div()
                            .flex()
                            .gap_2()
                            .child(self.export_option(
                                "export-format-mp4",
                                "MP4 (H.264)",
                                self.bundle.export_settings.format == ExportFormat::Mp4,
                                cx,
                                |editor, cx| {
                                    editor.bundle.export_settings.format = ExportFormat::Mp4;
                                    editor.exported_path = None;
                                    editor.save_bundle();
                                    cx.notify();
                                },
                            ))
                            .child(self.export_option(
                                "export-format-mov",
                                "MOV (H.264)",
                                self.bundle.export_settings.format == ExportFormat::Mov,
                                cx,
                                |editor, cx| {
                                    editor.bundle.export_settings.format = ExportFormat::Mov;
                                    editor.exported_path = None;
                                    editor.save_bundle();
                                    cx.notify();
                                },
                            )),
                    ))
                    .child(export_field(
                        "Resolution",
                        div()
                            .flex()
                            .gap_2()
                            .child(self.export_option(
                                "export-resolution-1080",
                                "1080p",
                                self.bundle.export_settings.resolution == ExportResolution::P1080,
                                cx,
                                |editor, cx| {
                                    editor.bundle.export_settings.resolution =
                                        ExportResolution::P1080;
                                    editor.exported_path = None;
                                    editor.save_bundle();
                                    cx.notify();
                                },
                            ))
                            .child(self.export_option(
                                "export-resolution-720",
                                "720p",
                                self.bundle.export_settings.resolution == ExportResolution::P720,
                                cx,
                                |editor, cx| {
                                    editor.bundle.export_settings.resolution =
                                        ExportResolution::P720;
                                    editor.exported_path = None;
                                    editor.save_bundle();
                                    cx.notify();
                                },
                            )),
                    ))
                    .child(export_field(
                        "Frame rate",
                        div()
                            .flex()
                            .gap_2()
                            .child(self.export_option(
                                "export-fps-30",
                                "30 fps",
                                self.bundle.export_settings.fps == 30,
                                cx,
                                |editor, cx| {
                                    editor.bundle.export_settings.fps = 30;
                                    editor.exported_path = None;
                                    editor.save_bundle();
                                    cx.notify();
                                },
                            ))
                            .child(self.export_option(
                                "export-fps-60",
                                "60 fps",
                                self.bundle.export_settings.fps == 60,
                                cx,
                                |editor, cx| {
                                    editor.bundle.export_settings.fps = 60;
                                    editor.exported_path = None;
                                    editor.save_bundle();
                                    cx.notify();
                                },
                            )),
                    ))
                    .child(export_field(
                        "Save to",
                        div()
                            .id("export-destination")
                            .h(px(36.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .rounded_md()
                            .bg(rgb(theme::CONTROL_BACKGROUND))
                            .border_1()
                            .border_color(rgb(theme::BORDER_SUBTLE))
                            .cursor_pointer()
                            .on_click(cx.listener(|editor, _, _, cx| {
                                if !editor.exporting {
                                    editor.choose_export_destination(cx);
                                }
                            }))
                            .child(
                                div()
                                    .max_w(px(300.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_sm()
                                    .child(destination),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .child("Choose"),
                            ),
                    ))
                    .when(self.exporting, |dialog| {
                        dialog.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .text_xs()
                                        .text_color(rgb(theme::TEXT_MUTED))
                                        .child("Rendering")
                                        .child(format!("{:.0}%", self.export_progress * 100.0)),
                                )
                                .child(
                                    div()
                                        .h(px(5.0))
                                        .rounded_full()
                                        .bg(rgb(theme::CONTROL_BACKGROUND))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(self.export_progress))
                                                .rounded_full()
                                                .bg(rgb(ACCENT)),
                                        ),
                                ),
                        )
                    })
                    .when_some(self.export_error.clone(), |dialog, error| {
                        dialog.child(div().text_xs().text_color(rgb(ACCENT)).child(error))
                    })
                    .when_some(exported_path, |dialog, path| {
                        dialog.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_xs()
                                .text_color(rgb(theme::TEXT_MUTED))
                                .child("Export complete")
                                .child(
                                    div()
                                        .id("reveal-export")
                                        .cursor_pointer()
                                        .text_color(rgb(theme::TEXT))
                                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                                        .child("Show in Finder"),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("start-export")
                            .h(px(38.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgb(if can_export {
                                ACCENT
                            } else {
                                theme::CONTROL_BACKGROUND
                            }))
                            .when(can_export, |button| {
                                button
                                    .hover(|button| button.bg(rgba(0xff4f_58dd)))
                                    .cursor_pointer()
                            })
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|editor, _, _, cx| editor.start_export(cx)))
                            .child(if self.exporting {
                                "Exporting..."
                            } else {
                                "Export"
                            }),
                    ),
            )
    }

    fn preview(&self, selected: Option<&crate::bundle::BundleInput>) -> Div {
        let aspect_ratio = self
            .current_frame
            .as_ref()
            .map(|frame| frame.get_width() as f32 / frame.get_height() as f32)
            .unwrap_or(16.0 / 9.0);
        if let Some(frame) = &self.current_frame {
            div()
                .w_full()
                .max_w(px(1280.0))
                .max_h(px(720.0))
                .aspect_ratio(aspect_ratio)
                .rounded_md()
                .overflow_hidden()
                .child(
                    div()
                        .size_full()
                        .child(
                            surface(frame.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                        .with_animation(
                            "initial-preview-fade-in",
                            Animation::new(PREVIEW_FADE_DURATION),
                            |preview, delta| preview.opacity(delta),
                        ),
                )
        } else {
            let scan = div()
                .absolute()
                .top(px(4.0))
                .bottom(px(4.0))
                .w(px(1.0))
                .bg(rgb(ACCENT))
                .child(
                    div()
                        .absolute()
                        .left(px(-2.0))
                        .bottom(px(-1.0))
                        .size(px(5.0))
                        .rounded_full()
                        .bg(rgb(ACCENT)),
                )
                .with_animation(
                    "initial-preview-scan",
                    Animation::new(PREVIEW_LOADING_DURATION).repeat(),
                    |scan, delta| {
                        let progress = if delta < 0.5 {
                            delta * 2.0
                        } else {
                            (1.0 - delta) * 2.0
                        };
                        scan.left(px(5.0 + progress * 76.0))
                    },
                );
            div()
                .w_full()
                .max_w(px(720.0))
                .aspect_ratio(aspect_ratio)
                .rounded_lg()
                .bg(rgb(theme::CANVAS_BACKGROUND))
                .border_1()
                .border_color(rgb(theme::BORDER_SUBTLE))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .relative()
                        .w(px(88.0))
                        .h(px(50.0))
                        .overflow_hidden()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .child(
                            div()
                                .absolute()
                                .left(px(8.0))
                                .right(px(8.0))
                                .top(px(15.0))
                                .h(px(1.0))
                                .bg(rgb(theme::BORDER_SUBTLE)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(8.0))
                                .right(px(24.0))
                                .top(px(25.0))
                                .h(px(1.0))
                                .bg(rgb(theme::BORDER_SUBTLE)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(8.0))
                                .right(px(38.0))
                                .top(px(35.0))
                                .h(px(1.0))
                                .bg(rgb(theme::BORDER_SUBTLE)),
                        )
                        .child(scan),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(if selected.is_some() {
                            "Rendering first frame"
                        } else {
                            "No input selected"
                        }),
                )
        }
    }

    fn timeline(&self, cx: &mut Context<Self>) -> Div {
        let bounds_cell = Rc::clone(&self.timeline_bounds);
        let rows_bounds_cell = Rc::clone(&self.timeline_rows_bounds);
        let view_start_secs = self.timeline_view_start_secs;
        let view_duration_secs = self.timeline_view_duration_secs;
        let playback_animation =
            self.video_timeline_resize_animation
                .as_ref()
                .and_then(|animation| {
                    let (from, to) = animation.playback_range?;
                    Some((from, to, animation.generation))
                });
        let playback_fraction = playback_animation.map(|(_, to, _)| to).or_else(|| {
            timeline_time_fraction(self.current_time_secs, view_start_secs, view_duration_secs)
        });
        let preview_fraction = self.cursor_time_secs.and_then(|time_secs| {
            timeline_time_fraction(time_secs, view_start_secs, view_duration_secs)
        });

        let mut tracks = div().relative().flex().flex_col().gap(px(8.0)).child(
            canvas(
                move |bounds, _, _| {
                    rows_bounds_cell.set(bounds);
                    bounds
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        let mut cut_bubbles = div().absolute().inset_0();
        let video_track_top = 0.0;
        if let Some((index, _)) = self.bundle.inputs.iter().enumerate().next() {
            let bounds_cell = Rc::clone(&bounds_cell);
            let uses_ghost_resize =
                self.bundle.video_segment_resize_mode == VideoSegmentResizeMode::Ghost;
            let mut video_track = div().h(px(48.0)).flex_1().relative();
            let mut timeline_start = 0.0;
            let mut cuts = Vec::new();
            let mut previous_source_end = None;
            let mut previous_ghost_end_fraction = None;
            let mut video_segment_layer = div().absolute().size_full().overflow_hidden();
            let video_segments = self.bundle.video_segments.as_deref().unwrap_or_default();
            for (segment_index, segment) in video_segments.iter().enumerate() {
                let id = segment.id;
                let segment_duration = segment.duration_secs();
                let segment_drag = self
                    .video_segment_drag
                    .as_ref()
                    .filter(|drag| drag.id == id);
                let (draft_source_start_secs, draft_source_end_secs) = segment_drag
                    .map(|drag| (drag.draft_source_start_secs, drag.draft_source_end_secs))
                    .unwrap_or((segment.source_start_secs, segment.source_end_secs));
                let draft_duration = draft_source_end_secs - draft_source_start_secs;
                let (display_start, display_end, next_timeline_start) = video_segment_drag_layout(
                    self.bundle.video_segment_resize_mode,
                    segment_drag.map(|drag| drag.edge),
                    timeline_start,
                    segment_duration,
                    draft_duration,
                );
                let ghost_indicator = if uses_ghost_resize
                    && draft_duration != segment_duration
                    && let Some(drag) = segment_drag
                {
                    let time_secs = match drag.edge {
                        VideoSegmentEdge::Start => timeline_start,
                        VideoSegmentEdge::End => timeline_start + segment_duration,
                    };
                    timeline_time_fraction(time_secs, view_start_secs, view_duration_secs)
                        .map(|fraction| (drag.edge, fraction))
                } else if uses_ghost_resize
                    && segment_drag.is_none()
                    && let Some(animation) = &self.video_timeline_resize_animation
                    && let Some((ghost_id, edge, ghost_start, ghost_end)) = animation.ghost_range
                    && ghost_id == id
                {
                    let time_secs = match edge {
                        VideoSegmentEdge::Start => ghost_start,
                        VideoSegmentEdge::End => ghost_end,
                    };
                    timeline_time_fraction(
                        time_secs,
                        animation.view_start_secs,
                        animation.view_duration_secs,
                    )
                    .map(|fraction| (edge, fraction))
                } else {
                    None
                };
                let ghost_start_fraction =
                    ghost_indicator.and_then(|(edge, fraction)| match edge {
                        VideoSegmentEdge::Start => Some(fraction),
                        VideoSegmentEdge::End => None,
                    });
                let ghost_end_fraction = ghost_indicator.and_then(|(edge, fraction)| match edge {
                    VideoSegmentEdge::Start => None,
                    VideoSegmentEdge::End => Some(fraction),
                });
                if let Some(source_end_secs) = previous_source_end {
                    let ghost_cut_animation = if let Some(from) = previous_ghost_end_fraction
                        && let Some(animation) = &self.video_timeline_resize_animation
                        && let Some((_, VideoSegmentEdge::End, _, ghost_end_secs)) =
                            animation.ghost_range
                        && timeline_start < ghost_end_secs
                        && let Some(to) = timeline_time_fraction(
                            timeline_start,
                            animation.target_view_start_secs,
                            animation.target_view_duration_secs,
                        ) {
                        Some((from, to, animation.generation))
                    } else if let Some(from) = ghost_start_fraction
                        && let Some(animation) = &self.video_timeline_resize_animation
                        && let Some((ghost_id, VideoSegmentEdge::Start, _, _)) =
                            animation.ghost_range
                        && ghost_id == id
                        && let Some(to) = timeline_time_fraction(
                            timeline_start,
                            animation.target_view_start_secs,
                            animation.target_view_duration_secs,
                        )
                    {
                        Some((from, to, animation.generation))
                    } else {
                        None
                    };
                    let resize_cut_animation = self
                        .video_timeline_resize_animation
                        .as_ref()
                        .and_then(|animation| {
                            let (_, from_start_secs, _) = animation
                                .from_ranges
                                .iter()
                                .find(|(animation_id, _, _)| *animation_id == id)?;
                            Some((
                                timeline_time_fraction(
                                    *from_start_secs,
                                    animation.view_start_secs,
                                    animation.view_duration_secs,
                                )?,
                                timeline_time_fraction(
                                    timeline_start,
                                    animation.target_view_start_secs,
                                    animation.target_view_duration_secs,
                                )?,
                                animation.generation,
                            ))
                        });
                    let cut_animation = if previous_ghost_end_fraction.is_some()
                        || ghost_start_fraction.is_some()
                    {
                        ghost_cut_animation
                    } else {
                        resize_cut_animation
                    };
                    let fraction = cut_animation
                        .map(|(_, to, _)| to)
                        .or(previous_ghost_end_fraction)
                        .or(ghost_start_fraction)
                        .or_else(|| {
                            timeline_time_fraction(
                                timeline_start,
                                view_start_secs,
                                view_duration_secs,
                            )
                        });
                    if let Some(fraction) = fraction {
                        cuts.push((
                            VideoCropIndicatorAction::Cut(id),
                            VideoCropIndicatorAlignment::Center,
                            fraction,
                            video_cut_gap_secs(source_end_secs, draft_source_start_secs),
                            cut_animation,
                        ));
                    }
                }
                let visible_range = timeline_segment_range_fraction(
                    display_start,
                    display_end,
                    view_start_secs,
                    view_duration_secs,
                );
                let segment_animation =
                    self.video_timeline_resize_animation
                        .as_ref()
                        .and_then(|animation| {
                            let (_, from_start_secs, from_end_secs) = animation
                                .from_ranges
                                .iter()
                                .find(|(animation_id, _, _)| *animation_id == id)?;
                            Some((*from_start_secs, *from_end_secs, animation))
                        });
                if segment_index == 0
                    && draft_source_start_secs - self.source_start_secs > f64::EPSILON
                    && let Some(fraction) = ghost_start_fraction.or_else(|| {
                        timeline_time_fraction(display_start, view_start_secs, view_duration_secs)
                    })
                {
                    let ghost_crop_animation = if let Some(from) = ghost_start_fraction
                        && let Some(animation) = &self.video_timeline_resize_animation
                        && let Some((ghost_id, VideoSegmentEdge::Start, _, _)) =
                            animation.ghost_range
                        && ghost_id == id
                        && let Some(to) = timeline_time_fraction(
                            display_start,
                            animation.target_view_start_secs,
                            animation.target_view_duration_secs,
                        ) {
                        Some((from, to, animation.generation))
                    } else {
                        None
                    };
                    let resize_crop_animation =
                        segment_animation.and_then(|(from_start_secs, _, animation)| {
                            Some((
                                timeline_time_fraction(
                                    from_start_secs,
                                    animation.view_start_secs,
                                    animation.view_duration_secs,
                                )?,
                                timeline_time_fraction(
                                    display_start,
                                    animation.target_view_start_secs,
                                    animation.target_view_duration_secs,
                                )?,
                                animation.generation,
                            ))
                        });
                    let crop_animation = if ghost_start_fraction.is_some() {
                        ghost_crop_animation
                    } else {
                        resize_crop_animation
                    };
                    let fraction = crop_animation
                        .map(|(_, to, _)| to)
                        .or(ghost_start_fraction)
                        .unwrap_or(fraction);
                    cuts.push((
                        VideoCropIndicatorAction::SegmentEdge(id, VideoSegmentEdge::Start),
                        VideoCropIndicatorAlignment::Start,
                        fraction,
                        (draft_source_start_secs - self.source_start_secs).max(0.0),
                        crop_animation,
                    ));
                }
                if segment_index + 1 == video_segments.len()
                    && self.source_end_secs - draft_source_end_secs > f64::EPSILON
                {
                    let ghost_crop_animation = if segment_drag.is_none()
                        && let Some(from) = ghost_end_fraction
                        && let Some(animation) = &self.video_timeline_resize_animation
                        && let Some((ghost_id, VideoSegmentEdge::End, _, ghost_end_secs)) =
                            animation.ghost_range
                        && ghost_id == id
                        && display_end < ghost_end_secs
                        && let Some(to) = timeline_time_fraction(
                            display_end,
                            animation.target_view_start_secs,
                            animation.target_view_duration_secs,
                        ) {
                        Some((from, to, animation.generation))
                    } else {
                        None
                    };
                    let crop_animation = ghost_crop_animation.or_else(|| {
                        segment_animation.and_then(|(_, from_end_secs, animation)| {
                            Some((
                                timeline_time_fraction(
                                    from_end_secs,
                                    animation.view_start_secs,
                                    animation.view_duration_secs,
                                )?,
                                timeline_time_fraction(
                                    display_end,
                                    animation.target_view_start_secs,
                                    animation.target_view_duration_secs,
                                )?,
                                animation.generation,
                            ))
                        })
                    });
                    let crop_animation =
                        if ghost_end_fraction.is_some() && ghost_crop_animation.is_none() {
                            None
                        } else {
                            crop_animation
                        };
                    let fraction = crop_animation
                        .map(|(_, to, _)| to)
                        .or(ghost_end_fraction)
                        .or_else(|| {
                            timeline_time_fraction(display_end, view_start_secs, view_duration_secs)
                        });
                    if let Some(fraction) = fraction {
                        cuts.push((
                            VideoCropIndicatorAction::SegmentEdge(id, VideoSegmentEdge::End),
                            VideoCropIndicatorAlignment::End,
                            fraction,
                            (self.source_end_secs - draft_source_end_secs).max(0.0),
                            crop_animation,
                        ));
                    }
                }
                let segment_selected = self.selected_video_segment == Some(id);
                let duration_label = duration_label(draft_duration);
                if uses_ghost_resize
                    && segment_drag.is_some()
                    && draft_duration != segment_duration
                    && let Some((start, width)) = timeline_segment_range_fraction(
                        timeline_start,
                        timeline_start + segment_duration,
                        view_start_secs,
                        view_duration_secs,
                    )
                {
                    video_segment_layer = video_segment_layer.child(
                        div()
                            .absolute()
                            .left(relative(start))
                            .w(relative(width))
                            .top_0()
                            .bottom_0()
                            .min_w(px(2.0))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(if segment_index > 0 { 2.0 } else { 0.0 }))
                                    .right(px(if segment_index + 1 < video_segments.len() {
                                        2.0
                                    } else {
                                        0.0
                                    }))
                                    .top_0()
                                    .bottom_0()
                                    .min_w(px(1.0))
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgba(0xffff_ff70))
                                    .bg(rgba(0xffff_ff18)),
                            ),
                    );
                }
                if uses_ghost_resize
                    && segment_drag.is_none()
                    && let Some(animation) = &self.video_timeline_resize_animation
                    && let Some(ghost_range) = animation.ghost_range
                    && ghost_range.0 == id
                    && let Some((start, width)) = timeline_segment_range_fraction(
                        ghost_range.2,
                        ghost_range.3,
                        animation.view_start_secs,
                        animation.view_duration_secs,
                    )
                {
                    video_segment_layer = video_segment_layer.child(
                        div()
                            .absolute()
                            .left(relative(start))
                            .w(relative(width))
                            .top_0()
                            .bottom_0()
                            .min_w(px(2.0))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(if segment_index > 0 { 2.0 } else { 0.0 }))
                                    .right(px(if segment_index + 1 < video_segments.len() {
                                        2.0
                                    } else {
                                        0.0
                                    }))
                                    .top_0()
                                    .bottom_0()
                                    .min_w(px(1.0))
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgba(0xffff_ff70))
                                    .bg(rgba(0xffff_ff18)),
                            )
                            .with_animation(
                                format!("video-segment-ghost-{}-{}", id, animation.generation),
                                Animation::new(TIMELINE_RESIZE_DURATION)
                                    .with_easing(gpui::ease_out_quint()),
                                |ghost, delta| ghost.opacity(1.0 - delta),
                            ),
                    );
                }
                if let Some((start, width)) = visible_range {
                    let hover_group = format!("video-segment-hover-{index}-{id}");
                    let segment_element = div()
                        .id(format!("video-segment-{index}-{id}"))
                        .group(hover_group.clone())
                        .absolute()
                        .left(relative(start))
                        .w(relative(width))
                        .top_0()
                        .bottom_0()
                        .min_w(px(2.0))
                        .when(
                            self.cut_mode
                                && segment_duration >= MIN_VIDEO_SEGMENT_DURATION_SECS * 2.0,
                            |segment| segment.cursor_crosshair(),
                        )
                        .when(
                            self.cut_mode
                                && segment_duration < MIN_VIDEO_SEGMENT_DURATION_SECS * 2.0,
                            |segment| segment.cursor_not_allowed(),
                        )
                        .when(!self.cut_mode, |segment| segment.cursor_pointer())
                        .child(
                            div()
                                .absolute()
                                .left(px(if segment_index > 0 { 2.0 } else { 0.0 }))
                                .right(px(if segment_index + 1 < video_segments.len() {
                                    2.0
                                } else {
                                    0.0
                                }))
                                .top_0()
                                .bottom_0()
                                .min_w(px(1.0))
                                .overflow_hidden()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(if segment_selected {
                                    theme::SELECTION
                                } else {
                                    theme::BORDER
                                }))
                                .bg(rgb(if segment_selected {
                                    theme::SELECTION_FILL
                                } else {
                                    0x0029_313d
                                }))
                                .text_sm()
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(duration_label),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                                editor.selected_input = index;
                                if let Some(time_secs) = editor.timeline_time_at(event.position.x) {
                                    editor.activate_video_segment(id, time_secs, cx);
                                }
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .id(format!("video-segment-start-{index}-{id}"))
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(12.0))
                                .cursor_ew_resize()
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(if segment_index > 0 { 8.0 } else { 6.0 }))
                                        .top(px(6.0))
                                        .bottom(px(6.0))
                                        .w(px(4.0))
                                        .rounded_full()
                                        .bg(rgba(0xffff_ff80))
                                        .invisible()
                                        .group_hover(hover_group.clone(), |handle| {
                                            handle.visible()
                                        }),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |editor, event, _, cx| {
                                        editor.begin_video_segment_drag(
                                            id,
                                            VideoSegmentEdge::Start,
                                            event,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id(format!("video-segment-end-{index}-{id}"))
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(12.0))
                                .cursor_ew_resize()
                                .child(
                                    div()
                                        .absolute()
                                        .right(px(if segment_index + 1 < video_segments.len() {
                                            8.0
                                        } else {
                                            6.0
                                        }))
                                        .top(px(6.0))
                                        .bottom(px(6.0))
                                        .w(px(4.0))
                                        .rounded_full()
                                        .bg(rgba(0xffff_ff80))
                                        .invisible()
                                        .group_hover(hover_group, |handle| handle.visible()),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |editor, event, _, cx| {
                                        editor.begin_video_segment_drag(
                                            id,
                                            VideoSegmentEdge::End,
                                            event,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }),
                                ),
                        );
                    let segment_element = if segment_drag.is_none()
                        && let Some(animation) = &self.video_timeline_resize_animation
                        && let Some((_, from_start_secs, from_end_secs)) = animation
                            .from_ranges
                            .iter()
                            .find(|(animation_id, _, _)| *animation_id == id)
                        && let Some((from_start, from_width)) = timeline_segment_range_fraction(
                            *from_start_secs,
                            *from_end_secs,
                            animation.view_start_secs,
                            animation.view_duration_secs,
                        )
                        && let Some((to_start, to_width)) = timeline_segment_range_fraction(
                            display_start,
                            display_end,
                            animation.target_view_start_secs,
                            animation.target_view_duration_secs,
                        ) {
                        let generation = animation.generation;
                        segment_element
                            .with_animation(
                                format!("video-segment-resize-{index}-{id}-{generation}"),
                                Animation::new(TIMELINE_RESIZE_DURATION)
                                    .with_easing(gpui::ease_out_quint()),
                                move |segment, delta| {
                                    segment
                                        .left(relative(
                                            from_start + (to_start - from_start) * delta,
                                        ))
                                        .w(relative(from_width + (to_width - from_width) * delta))
                                },
                            )
                            .into_any_element()
                    } else {
                        segment_element.into_any_element()
                    };
                    video_segment_layer = video_segment_layer.child(segment_element);
                }
                timeline_start = next_timeline_start;
                previous_source_end = Some(draft_source_end_secs);
                previous_ghost_end_fraction = ghost_end_fraction;
            }
            video_track = video_track.child(video_segment_layer);
            video_track = video_track.child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(bounds);
                        bounds
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
            for (action, alignment, fraction, gap_secs, cut_animation) in cuts {
                let indicator_id = match action {
                    VideoCropIndicatorAction::SegmentEdge(id, VideoSegmentEdge::Start) => {
                        format!("video-crop-start-{index}-{id}")
                    }
                    VideoCropIndicatorAction::SegmentEdge(id, VideoSegmentEdge::End) => {
                        format!("video-crop-end-{index}-{id}")
                    }
                    VideoCropIndicatorAction::Cut(id) => format!("video-cut-{index}-{id}"),
                };
                let cut_line = div()
                    .absolute()
                    .left(relative(fraction))
                    .top(px(-2.0))
                    .bottom_0()
                    .w(px(1.0))
                    .bg(rgb(ACCENT));
                let bubble = div()
                    .id(indicator_id)
                    .absolute()
                    .top_0()
                    .min_w(px(33.0))
                    .h(px(16.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(ACCENT))
                    .hover(|bubble| bubble.bg(rgb(theme::SELECTION)))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(theme::APP_BACKGROUND))
                    .on_click(cx.listener(move |editor, _, _, cx| {
                        match action {
                            VideoCropIndicatorAction::SegmentEdge(id, edge) => {
                                editor.activate_video_segment_edge(id, edge, cx);
                            }
                            VideoCropIndicatorAction::Cut(right_id) => {
                                editor.activate_video_cut(right_id, cx);
                            }
                        }
                        cx.stop_propagation();
                    }))
                    .child(duration_label(gap_secs));
                let bubble = match alignment {
                    VideoCropIndicatorAlignment::Start => bubble.left_0().rounded_r_full(),
                    VideoCropIndicatorAlignment::Center => bubble.left(px(-16.0)).rounded_full(),
                    VideoCropIndicatorAlignment::End => bubble.right_0().rounded_l_full(),
                };
                let cut_bubble = div()
                    .absolute()
                    .left(relative(fraction))
                    .top(px(video_track_top - 18.0))
                    .w(px(1.0))
                    .child(bubble);
                if let Some((from, to, generation)) = cut_animation {
                    let animation_id = match action {
                        VideoCropIndicatorAction::SegmentEdge(id, VideoSegmentEdge::Start) => {
                            format!("start-{id}")
                        }
                        VideoCropIndicatorAction::SegmentEdge(id, VideoSegmentEdge::End) => {
                            format!("end-{id}")
                        }
                        VideoCropIndicatorAction::Cut(id) => format!("cut-{id}"),
                    };
                    video_track = video_track.child(
                        cut_line.with_animation(
                            format!("video-crop-line-{index}-{animation_id}-{generation}"),
                            Animation::new(TIMELINE_RESIZE_DURATION)
                                .with_easing(gpui::ease_out_quint()),
                            move |cut, delta| cut.left(relative(from + (to - from) * delta)),
                        ),
                    );
                    cut_bubbles = cut_bubbles.child(
                        cut_bubble.with_animation(
                            format!("video-crop-bubble-{index}-{animation_id}-{generation}"),
                            Animation::new(TIMELINE_RESIZE_DURATION)
                                .with_easing(gpui::ease_out_quint()),
                            move |cut, delta| cut.left(relative(from + (to - from) * delta)),
                        ),
                    );
                } else {
                    video_track = video_track.child(cut_line);
                    cut_bubbles = cut_bubbles.child(cut_bubble);
                }
            }
            tracks = tracks.child(div().h(px(48.0)).flex().child(video_track));
        }
        let zoom_track_bounds = Rc::clone(&bounds_cell);
        let mut zoom_segments = div().h(px(48.0)).flex_1().relative().overflow_hidden();
        for segment in &self.bundle.zoom_segments {
            let id = segment.id;
            let resize_drag = self.zoom_segment_drag.filter(|drag| {
                drag.id == id && matches!(drag.kind, ZoomSegmentDragKind::Resize(_))
            });
            let (segment_start_secs, segment_end_secs) = resize_drag
                .map(|drag| (drag.draft_start_secs, drag.draft_end_secs))
                .unwrap_or((segment.start_secs, segment.end_secs));
            let visible_range = timeline_segment_range_fraction(
                segment_start_secs,
                segment_end_secs,
                view_start_secs,
                view_duration_secs,
            );
            let selected = self.selected_zoom == Some(id);
            if let Some((start, width)) = visible_range {
                let duration_label = duration_label(segment_end_secs - segment_start_secs);
                let hover_group = format!("zoom-segment-hover-{id}");
                let segment_element = div()
                    .id(("zoom-segment", id))
                    .group(hover_group.clone())
                    .absolute()
                    .left(relative(start))
                    .w(relative(width))
                    .top(px(3.0))
                    .bottom(px(3.0))
                    .min_w(px(12.0))
                    .cursor_move()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .left_0()
                            .rounded_lg()
                            .overflow_hidden()
                            .bg(rgba(if selected { 0xff4f_58cc } else { 0xff4f_5870 }))
                            .border_1()
                            .border_color(rgb(if selected {
                                theme::SELECTION
                            } else {
                                theme::BORDER_SUBTLE
                            }))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .px_2()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .child(duration_label)
                                    .child(format!("{}x", segment.amount as u32)),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                            editor.begin_zoom_segment_drag(
                                id,
                                ZoomSegmentDragKind::Move,
                                event,
                                cx,
                            );
                            if let Some(time_secs) = editor.timeline_time_at(event.position.x) {
                                editor.set_playback_time(time_secs);
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id(("zoom-segment-start", id))
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(12.0))
                            .cursor_ew_resize()
                            .child(
                                div()
                                    .absolute()
                                    .left(px(6.0))
                                    .top(px(6.0))
                                    .bottom(px(6.0))
                                    .w(px(4.0))
                                    .rounded_full()
                                    .bg(rgba(0xffff_ff80))
                                    .invisible()
                                    .group_hover(hover_group.clone(), |handle| handle.visible()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |editor, event, _, cx| {
                                    editor.begin_zoom_segment_drag(
                                        id,
                                        ZoomSegmentDragKind::Resize(ZoomSegmentEdge::Start),
                                        event,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id(("zoom-segment-end", id))
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(12.0))
                            .cursor_ew_resize()
                            .child(
                                div()
                                    .absolute()
                                    .right(px(6.0))
                                    .top(px(6.0))
                                    .bottom(px(6.0))
                                    .w(px(4.0))
                                    .rounded_full()
                                    .bg(rgba(0xffff_ff80))
                                    .invisible()
                                    .group_hover(hover_group, |handle| handle.visible()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |editor, event, _, cx| {
                                    editor.begin_zoom_segment_drag(
                                        id,
                                        ZoomSegmentDragKind::Resize(ZoomSegmentEdge::End),
                                        event,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            ),
                    );
                let segment_element = if resize_drag.is_none()
                    && let Some(animation) = &self.video_timeline_resize_animation
                    && let Some((_, from_start_secs, from_end_secs)) = animation
                        .from_zoom_ranges
                        .iter()
                        .find(|(animation_id, _, _)| *animation_id == id)
                    && let Some((from_start, from_width)) = timeline_segment_range_fraction(
                        *from_start_secs,
                        *from_end_secs,
                        animation.view_start_secs,
                        animation.view_duration_secs,
                    )
                    && let Some((to_start, to_width)) = timeline_segment_range_fraction(
                        segment_start_secs,
                        segment_end_secs,
                        animation.target_view_start_secs,
                        animation.target_view_duration_secs,
                    ) {
                    let generation = animation.generation;
                    segment_element
                        .with_animation(
                            format!("zoom-segment-resize-{id}-{generation}"),
                            Animation::new(TIMELINE_RESIZE_DURATION)
                                .with_easing(gpui::ease_out_quint()),
                            move |segment, delta| {
                                segment
                                    .left(relative(from_start + (to_start - from_start) * delta))
                                    .w(relative(from_width + (to_width - from_width) * delta))
                            },
                        )
                        .into_any_element()
                } else {
                    segment_element.into_any_element()
                };
                zoom_segments = zoom_segments.child(segment_element);
            }
        }
        zoom_segments = zoom_segments.child(
            canvas(
                move |bounds, _, _| {
                    zoom_track_bounds.set(bounds);
                    bounds
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        if let Some((start_secs, end_secs)) = self.zoom_hover_range
            && let Some((start, width)) = timeline_segment_range_fraction(
                start_secs,
                end_secs,
                view_start_secs,
                view_duration_secs,
            )
        {
            let duration_label = duration_label(end_secs - start_secs);
            zoom_segments = zoom_segments.child(
                div()
                    .absolute()
                    .left(relative(start))
                    .w(relative(width))
                    .top(px(3.0))
                    .bottom(px(3.0))
                    .min_w(px(12.0))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .left_0()
                            .rounded_lg()
                            .overflow_hidden()
                            .bg(rgba(0xff4f_5838))
                            .border_1()
                            .border_color(rgba(0xff4f_5858))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .px_2()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .child(duration_label)
                                    .child("2x"),
                            ),
                    ),
            );
        }
        zoom_segments = zoom_segments.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                let Some(time_secs) = editor.timeline_time_at(event.position.x) else {
                    return;
                };
                if let Some(id) = editor
                    .bundle
                    .zoom_segments
                    .iter()
                    .find(|segment| {
                        time_secs >= segment.start_secs && time_secs <= segment.end_secs
                    })
                    .map(|segment| segment.id)
                {
                    editor.begin_zoom_segment_drag(id, ZoomSegmentDragKind::Move, event, cx);
                    editor.set_playback_time(time_secs);
                    cx.stop_propagation();
                    return;
                }
                let Some((start_secs, end_secs)) = zoom_segment_range_at(
                    time_secs,
                    editor.duration_secs,
                    &editor.bundle.zoom_segments,
                ) else {
                    return;
                };
                editor.add_zoom(start_secs, end_secs, cx);
                cx.stop_propagation();
            }),
        );
        tracks = tracks.child(div().h(px(48.0)).flex().child(zoom_segments));
        tracks = tracks.when_some(preview_fraction, |tracks, fraction| {
            tracks.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(-4.0))
                    .bottom(px(-8.0))
                    .child(
                        div()
                            .absolute()
                            .top(px(-8.0))
                            .bottom_0()
                            .left(relative(fraction))
                            .w(px(2.0))
                            .bg(rgb(PREVIEW_HANDLE))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-5.0))
                                    .left(px(-4.0))
                                    .size(px(10.0))
                                    .rounded_full()
                                    .bg(rgb(PREVIEW_HANDLE)),
                            ),
                    ),
            )
        });
        tracks = tracks.when_some(playback_fraction, |tracks, playback_fraction| {
            let playhead = div()
                .absolute()
                .top(px(-8.0))
                .bottom_0()
                .left(relative(playback_fraction))
                .w(px(2.0))
                .bg(rgb(ACCENT))
                .child(
                    div()
                        .absolute()
                        .top(px(-5.0))
                        .left(px(-4.0))
                        .size(px(10.0))
                        .rounded_full()
                        .bg(rgb(ACCENT)),
                );
            let playhead = if let Some((from, to, generation)) = playback_animation {
                playhead
                    .with_animation(
                        format!("playhead-resize-{generation}"),
                        Animation::new(TIMELINE_RESIZE_DURATION)
                            .with_easing(gpui::ease_out_quint()),
                        move |playhead, delta| playhead.left(relative(from + (to - from) * delta)),
                    )
                    .into_any_element()
            } else {
                playhead.into_any_element()
            };
            tracks.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(-4.0))
                    .bottom(px(-8.0))
                    .child(playhead),
            )
        });
        tracks = tracks.child(cut_bubbles);

        let display_time_secs = self.cursor_time_secs.unwrap_or(self.current_time_secs);
        let playback_time_label = timeline_timecode(display_time_secs, self.timeline_fps);
        let total_time_label = timeline_timecode(self.duration_secs, self.timeline_fps);

        div()
            .pt_4()
            .pb(px(8.0))
            .flex()
            .flex_col()
            .gap_3()
            .border_t_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .bg(rgb(theme::PANEL_BACKGROUND))
            .on_pinch(cx.listener(|editor, event: &PinchEvent, _, cx| {
                editor.zoom_timeline(event, cx);
            }))
            .on_scroll_wheel(cx.listener(|editor, event: &ScrollWheelEvent, _, cx| {
                editor.pan_timeline(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                    editor.selected_zoom = None;
                    editor.selected_video_segment = None;
                    cx.notify();
                    let bounds = editor.timeline_bounds.get();
                    if event.position.x >= bounds.origin.x - px(6.0) {
                        let Some(time_secs) = editor.timeline_time_at(event.position.x) else {
                            return;
                        };
                        editor.set_playback_time(time_secs);
                    }
                }),
            )
            .on_mouse_exit(cx.listener(|editor, _: &MouseExitEvent, _, cx| {
                if editor.cursor_time_secs.is_some() {
                    editor.cursor_time_secs = None;
                    editor.zoom_hover_range = None;
                    editor.request_preview();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(div().w(px(68.0)).text_right().child(playback_time_label))
                            .child(
                                div()
                                    .id("playback-back")
                                    .w(px(32.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .hover(|button| {
                                        button
                                            .bg(rgb(theme::CONTROL_HOVER))
                                            .text_color(rgb(theme::TEXT))
                                    })
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.set_playback_time(0.0);
                                        cx.notify();
                                    }))
                                    .child(
                                        svg()
                                            .size(px(15.0))
                                            .path(PLAYBACK_BACK)
                                            .text_color(rgb(theme::TEXT_MUTED)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("toggle-playback")
                                    .size(px(36.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(rgb(theme::CONTROL_BACKGROUND))
                                    .border_1()
                                    .border_color(rgb(theme::BORDER))
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT))
                                    .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                                    .on_click(cx.listener(|editor, _, window, cx| {
                                        editor.toggle_playback(window, cx);
                                    }))
                                    .child(
                                        svg()
                                            .size(px(15.0))
                                            .path(if self.is_playing { PAUSE } else { PLAY })
                                            .text_color(rgb(theme::TEXT)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("playback-forward")
                                    .w(px(32.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .hover(|button| {
                                        button
                                            .bg(rgb(theme::CONTROL_HOVER))
                                            .text_color(rgb(theme::TEXT))
                                    })
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.set_playback_time(editor.duration_secs);
                                        cx.notify();
                                    }))
                                    .child(
                                        svg()
                                            .size(px(15.0))
                                            .path(PLAYBACK_FORWARD)
                                            .text_color(rgb(theme::TEXT_MUTED)),
                                    ),
                            )
                            .child(div().w(px(68.0)).child(total_time_label)),
                    )
                    .child(self.timeline_zoom_slider(cx))
                    .child(
                        div().w(px(80.0)).flex().justify_end().child(
                            div()
                                .id("cut-mode")
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(rgb(if self.cut_mode {
                                    theme::CONTROL_ACTIVE
                                } else {
                                    theme::CONTROL_BACKGROUND
                                }))
                                .border_1()
                                .border_color(rgb(if self.cut_mode {
                                    theme::SELECTION
                                } else {
                                    theme::BORDER_SUBTLE
                                }))
                                .cursor_pointer()
                                .text_color(rgb(if self.cut_mode {
                                    theme::TEXT
                                } else {
                                    theme::TEXT_MUTED
                                }))
                                .on_click(cx.listener(|editor, _, _, cx| {
                                    editor.toggle_cut_mode(cx);
                                }))
                                .child("Cut (C)"),
                        ),
                    ),
            )
            .child(timeline_ruler(view_start_secs, view_duration_secs))
            .child(div().w_full().pl_4().child(tracks))
    }
}

impl Render for BundleEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.bundle.inputs.get(self.selected_input);
        let bundle_path = self.path.clone();
        div()
            .size_full()
            .relative()
            .key_context("BundleEditor")
            .track_focus(&self.focus_handle)
            .on_action(|_: &crate::CloseWindow, window, _| window.remove_window())
            .on_action(|action: &crate::CloseAllWindows, _, cx| {
                crate::close_all_windows(action, cx);
            })
            .on_action(cx.listener(|editor, _: &crate::ToggleCutMode, _, cx| {
                editor.toggle_cut_mode(cx);
            }))
            .on_action(
                cx.listener(|editor, _: &crate::TogglePlayback, window, cx| {
                    editor.toggle_playback(window, cx);
                }),
            )
            .on_action(cx.listener(|editor, _: &crate::DeleteSelected, _, cx| {
                editor.delete_selected(cx);
            }))
            .on_action(cx.listener(|editor, _: &crate::CloseExportDialog, _, cx| {
                if editor.crop_dialog_open {
                    editor.close_crop_dialog(cx);
                } else if editor.settings_dialog_open {
                    editor.settings_dialog_open = false;
                    cx.notify();
                } else if editor.aspect_ratio_dropdown_open {
                    editor.close_aspect_ratio_dropdown(cx);
                } else if editor.export_dialog_open
                    || editor.selected_zoom.is_some()
                    || editor.selected_video_segment.is_some()
                {
                    editor.export_dialog_open = false;
                    editor.selected_zoom = None;
                    editor.selected_video_segment = None;
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .bg(rgb(theme::APP_BACKGROUND))
            .text_color(rgb(theme::TEXT))
            .on_mouse_move(cx.listener(|editor, event: &MouseMoveEvent, _, cx| {
                editor.drag_slider(event, cx);
                editor.update_timeline_hover(event.position, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|editor, _, _, cx| {
                    if editor.active_slider.is_some_and(|slider| {
                        matches!(
                            slider,
                            SliderKind::Padding
                                | SliderKind::Radius
                                | SliderKind::Shadow
                                | SliderKind::ZoomAmount
                                | SliderKind::CameraSize
                                | SliderKind::CameraPadding
                                | SliderKind::CameraZoomReduction
                                | SliderKind::CameraShadow
                        )
                    }) {
                        editor.save_bundle();
                    }
                    editor.active_slider = None;
                    editor.finish_zoom_segment_drag(cx);
                    editor.finish_video_segment_drag(cx);
                    if editor.dragging_zoom_target {
                        editor.save_bundle();
                    }
                    editor.dragging_zoom_target = false;
                    editor.crop_drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|editor, _, _, cx| {
                    if editor.active_slider.is_some_and(|slider| {
                        matches!(
                            slider,
                            SliderKind::Padding
                                | SliderKind::Radius
                                | SliderKind::Shadow
                                | SliderKind::ZoomAmount
                                | SliderKind::CameraSize
                                | SliderKind::CameraPadding
                                | SliderKind::CameraZoomReduction
                                | SliderKind::CameraShadow
                        )
                    }) {
                        editor.save_bundle();
                    }
                    editor.active_slider = None;
                    editor.finish_zoom_segment_drag(cx);
                    editor.finish_video_segment_drag(cx);
                    if editor.dragging_zoom_target {
                        editor.save_bundle();
                    }
                    editor.dragging_zoom_target = false;
                    editor.crop_drag = None;
                }),
            )
            .child(
                div()
                    .h(px(38.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(theme::BORDER_SUBTLE))
                    .child(
                        div()
                            .id("open-editor-settings")
                            .h(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .bg(rgb(theme::CONTROL_BACKGROUND))
                            .border_1()
                            .border_color(rgb(theme::BORDER_SUBTLE))
                            .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|editor, _, _, cx| {
                                editor.settings_dialog_open = true;
                                editor.close_aspect_ratio_dropdown(cx);
                                cx.notify();
                            }))
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .id("reveal-bundle")
                            .h(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .bg(rgb(theme::CONTROL_BACKGROUND))
                            .border_1()
                            .border_color(rgb(theme::BORDER_SUBTLE))
                            .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.reveal_path(&bundle_path);
                            }))
                            .child("Reveal in Finder"),
                    )
                    .child(
                        div()
                            .id("open-export")
                            .h(px(24.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .bg(rgb(ACCENT))
                            .hover(|button| button.bg(rgba(0xff4f_58dd)))
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|editor, _, _, cx| {
                                editor.export_dialog_open = true;
                                editor.export_error = None;
                                cx.notify();
                            }))
                            .child("Export"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .relative()
                            .bg(rgb(theme::CANVAS_BACKGROUND))
                            .child(
                                div()
                                    .size_full()
                                    .p_8()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(self.preview(selected)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(10.0))
                                    .left_0()
                                    .right_0()
                                    .flex()
                                    .justify_center()
                                    .child(
                                        div()
                                            .flex()
                                            .items_start()
                                            .gap_2()
                                            .child(self.aspect_ratio_dropdown(cx))
                                            .child(
                                                div()
                                                    .id("open-crop")
                                                    .h(px(26.0))
                                                    .px_3()
                                                    .flex()
                                                    .items_center()
                                                    .rounded_md()
                                                    .bg(rgb(theme::PANEL_BACKGROUND))
                                                    .border_1()
                                                    .border_color(rgb(theme::BORDER))
                                                    .shadow_sm()
                                                    .hover(|button| {
                                                        button.bg(rgb(theme::CONTROL_HOVER))
                                                    })
                                                    .cursor_pointer()
                                                    .text_xs()
                                                    .on_click(cx.listener(|editor, _, _, cx| {
                                                        editor.open_crop_dialog(cx);
                                                    }))
                                                    .child("Crop"),
                                            ),
                                    ),
                            ),
                    )
                    .child(self.sidebar(cx)),
            )
            .child(self.timeline(cx))
            .when(self.crop_dialog_open, |editor| {
                editor.child(self.crop_dialog(cx))
            })
            .when(self.settings_dialog_open, |editor| {
                editor.child(self.settings_dialog(cx))
            })
            .when(self.export_dialog_open, |editor| {
                editor.child(self.export_dialog(cx))
            })
    }
}

fn crop_mask(left: f32, top: f32, width: f32, height: f32) -> Div {
    div()
        .absolute()
        .left(relative(left.clamp(0.0, 1.0)))
        .top(relative(top.clamp(0.0, 1.0)))
        .w(relative(width.clamp(0.0, 1.0)))
        .h(relative(height.clamp(0.0, 1.0)))
        .bg(rgba(0x0000_0099))
}

fn crop_contains(crop: ScreenCrop, point: [f32; 2]) -> bool {
    point[0] >= crop.position[0]
        && point[0] <= crop.position[0] + crop.size[0]
        && point[1] >= crop.position[1]
        && point[1] <= crop.position[1] + crop.size[1]
}

fn crop_handle_points() -> [(CropResizeHandle, [f32; 2]); 8] {
    [
        (CropResizeHandle::TopLeft, [0.0, 0.0]),
        (CropResizeHandle::Top, [0.5, 0.0]),
        (CropResizeHandle::TopRight, [1.0, 0.0]),
        (CropResizeHandle::Right, [1.0, 0.5]),
        (CropResizeHandle::BottomRight, [1.0, 1.0]),
        (CropResizeHandle::Bottom, [0.5, 1.0]),
        (CropResizeHandle::BottomLeft, [0.0, 1.0]),
        (CropResizeHandle::Left, [0.0, 0.5]),
    ]
}

fn crop_handle_at(
    position: gpui::Point<Pixels>,
    viewport: Bounds<Pixels>,
    crop: ScreenCrop,
) -> Option<CropResizeHandle> {
    if viewport.size.width <= Pixels::ZERO || viewport.size.height <= Pixels::ZERO {
        return None;
    }
    crop_handle_points()
        .into_iter()
        .find(|(_, point)| {
            let x = viewport.origin.x
                + viewport.size.width * (crop.position[0] + crop.size[0] * point[0]);
            let y = viewport.origin.y
                + viewport.size.height * (crop.position[1] + crop.size[1] * point[1]);
            (position.x - x).abs() <= px(10.0) && (position.y - y).abs() <= px(10.0)
        })
        .map(|(handle, _)| handle)
}

fn crop_resize_handles(
    crop: ScreenCrop,
    preview_width: f32,
    preview_height: f32,
) -> Vec<gpui::Stateful<Div>> {
    crop_handle_points()
        .into_iter()
        .enumerate()
        .map(|(index, (handle, point))| {
            div()
                .id(format!("crop-resize-handle-{index}"))
                .absolute()
                .left(px(10.0
                    + preview_width
                        * (crop.position[0] + crop.size[0] * point[0])))
                .top(px(10.0
                    + preview_height
                        * (crop.position[1] + crop.size[1] * point[1])))
                .ml(px(-5.0))
                .mt(px(-5.0))
                .size(px(10.0))
                .rounded_sm()
                .bg(rgb(CROP_SELECTION))
                .cursor(crop_resize_cursor(handle))
        })
        .collect()
}

const fn crop_resize_cursor(handle: CropResizeHandle) -> CursorStyle {
    match handle {
        CropResizeHandle::TopLeft | CropResizeHandle::BottomRight => {
            CursorStyle::ResizeUpLeftDownRight
        }
        CropResizeHandle::TopRight | CropResizeHandle::BottomLeft => {
            CursorStyle::ResizeUpRightDownLeft
        }
        CropResizeHandle::Top | CropResizeHandle::Bottom => CursorStyle::ResizeUpDown,
        CropResizeHandle::Left | CropResizeHandle::Right => CursorStyle::ResizeLeftRight,
    }
}

fn resized_screen_crop(
    original: ScreenCrop,
    handle: CropResizeHandle,
    pointer: [f32; 2],
) -> ScreenCrop {
    let mut left = original.position[0];
    let mut top = original.position[1];
    let mut right = left + original.size[0];
    let mut bottom = top + original.size[1];
    if matches!(
        handle,
        CropResizeHandle::TopLeft | CropResizeHandle::Left | CropResizeHandle::BottomLeft
    ) {
        left = pointer[0].clamp(0.0, right - 0.01);
    }
    if matches!(
        handle,
        CropResizeHandle::TopRight | CropResizeHandle::Right | CropResizeHandle::BottomRight
    ) {
        right = pointer[0].clamp(left + 0.01, 1.0);
    }
    if matches!(
        handle,
        CropResizeHandle::TopLeft | CropResizeHandle::Top | CropResizeHandle::TopRight
    ) {
        top = pointer[1].clamp(0.0, bottom - 0.01);
    }
    if matches!(
        handle,
        CropResizeHandle::BottomLeft | CropResizeHandle::Bottom | CropResizeHandle::BottomRight
    ) {
        bottom = pointer[1].clamp(top + 0.01, 1.0);
    }
    ScreenCrop {
        position: [left, top],
        size: [right - left, bottom - top],
    }
}

fn crop_dialog_button(id: &'static str, label: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(34.0))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(rgb(theme::CONTROL_BACKGROUND))
        .border_1()
        .border_color(rgb(theme::BORDER_SUBTLE))
        .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
        .cursor_pointer()
        .text_xs()
        .child(label)
}

fn export_field(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme::TEXT_MUTED))
                .child(label),
        )
        .child(control)
}

fn segment_detail(label: &'static str, value: f64) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .text_xs()
        .child(div().text_color(rgb(theme::TEXT_MUTED)).child(label))
        .child(
            div()
                .text_color(rgb(theme::TEXT))
                .child(format!("{value:.2} s")),
        )
}

fn video_timeline_duration(bundle: &BlipBundle) -> f64 {
    bundle
        .video_segments
        .iter()
        .flatten()
        .map(VideoSegment::duration_secs)
        .sum()
}

fn audio_mixer(sources: &[blip_audio::AudioSource], bundle: &BlipBundle) -> blip_audio::AudioMixer {
    let segments = bundle
        .video_segments
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|segment| blip_audio::AudioTimelineSegment {
            source_start_secs: segment.source_start_secs,
            source_end_secs: segment.source_end_secs,
        })
        .collect();
    blip_audio::AudioMixer::new(sources.to_vec(), segments)
}

fn can_split_video_segment(duration_secs: f64, offset_secs: f64) -> bool {
    offset_secs >= MIN_VIDEO_SEGMENT_DURATION_SECS
        && duration_secs - offset_secs >= MIN_VIDEO_SEGMENT_DURATION_SECS
}

#[derive(Debug, PartialEq)]
struct VideoCutEdit {
    timeline_start_secs: f64,
    inserted_duration_secs: f64,
    left_id: u64,
    removed_id: Option<u64>,
}

fn edit_video_cut(segments: &mut Vec<VideoSegment>, right_id: u64) -> Option<VideoCutEdit> {
    let right_index = segments.iter().position(|segment| segment.id == right_id)?;
    let left_index = right_index.checked_sub(1)?;
    let timeline_start_secs = segments
        .iter()
        .take(right_index)
        .map(|segment| segment.duration_secs())
        .sum();
    let gap_secs =
        (segments[right_index].source_start_secs - segments[left_index].source_end_secs).max(0.0);
    let left_id = segments[left_index].id;

    if gap_secs > f64::EPSILON {
        segments[left_index].source_end_secs = segments[right_index].source_start_secs;
        Some(VideoCutEdit {
            timeline_start_secs,
            inserted_duration_secs: gap_secs,
            left_id,
            removed_id: None,
        })
    } else {
        let right_end_secs = segments[right_index].source_end_secs;
        segments[left_index].source_end_secs =
            segments[left_index].source_end_secs.max(right_end_secs);
        let removed_id = Some(segments[right_index].id);
        segments.remove(right_index);
        Some(VideoCutEdit {
            timeline_start_secs,
            inserted_duration_secs: 0.0,
            left_id,
            removed_id,
        })
    }
}

fn duration_label(duration_secs: f64) -> String {
    let rounded = (duration_secs.max(0.0) * 10.0).round() / 10.0;
    if rounded == 0.0 {
        "0s".to_owned()
    } else {
        format!("{rounded:.1}s")
    }
}

fn video_cut_gap_secs(left_source_end_secs: f64, right_source_start_secs: f64) -> f64 {
    right_source_start_secs - left_source_end_secs
}

fn video_segment_drag_layout(
    resize_mode: VideoSegmentResizeMode,
    edge: Option<VideoSegmentEdge>,
    timeline_start_secs: f64,
    original_duration_secs: f64,
    draft_duration_secs: f64,
) -> (f64, f64, f64) {
    if edge.is_none() {
        return (
            timeline_start_secs,
            timeline_start_secs + original_duration_secs,
            timeline_start_secs + original_duration_secs,
        );
    }

    if resize_mode == VideoSegmentResizeMode::Live {
        return (
            timeline_start_secs,
            timeline_start_secs + draft_duration_secs,
            timeline_start_secs + draft_duration_secs,
        );
    }

    let (display_start_secs, display_end_secs) = match edge {
        Some(VideoSegmentEdge::Start) if draft_duration_secs < original_duration_secs => (
            timeline_start_secs + original_duration_secs - draft_duration_secs,
            timeline_start_secs + original_duration_secs,
        ),
        _ => (
            timeline_start_secs,
            timeline_start_secs + draft_duration_secs,
        ),
    };
    let next_timeline_start_secs = timeline_start_secs
        + if draft_duration_secs > original_duration_secs {
            draft_duration_secs
        } else {
            original_duration_secs
        };
    (
        display_start_secs,
        display_end_secs,
        next_timeline_start_secs,
    )
}

fn resize_video_segment_range(
    edge: VideoSegmentEdge,
    source_start_secs: f64,
    source_end_secs: f64,
    minimum_source_start_secs: f64,
    maximum_source_end_secs: f64,
    delta_secs: f64,
) -> (f64, f64) {
    match edge {
        VideoSegmentEdge::Start => (
            (source_start_secs + delta_secs)
                .max(minimum_source_start_secs)
                .min(source_end_secs - 0.01),
            source_end_secs,
        ),
        VideoSegmentEdge::End => (
            source_start_secs,
            (source_end_secs + delta_secs)
                .min(maximum_source_end_secs)
                .max(source_start_secs + 0.01),
        ),
    }
}

fn resize_zoom_segment_range(
    edge: ZoomSegmentEdge,
    start_secs: f64,
    end_secs: f64,
    minimum_start_secs: f64,
    maximum_end_secs: f64,
    delta_secs: f64,
) -> (f64, f64) {
    match edge {
        ZoomSegmentEdge::Start => (
            (start_secs + delta_secs)
                .max(minimum_start_secs)
                .min(end_secs - 0.01),
            end_secs,
        ),
        ZoomSegmentEdge::End => (
            start_secs,
            (end_secs + delta_secs)
                .min(maximum_end_secs)
                .max(start_secs + 0.01),
        ),
    }
}

fn source_time_at(bundle: &BlipBundle, timeline_time_secs: f64) -> Option<f64> {
    let segments = bundle.video_segments.as_ref()?;
    let mut remaining = timeline_time_secs.max(0.0);
    for (index, segment) in segments.iter().enumerate() {
        let duration = segment.duration_secs();
        if remaining < duration || index + 1 == segments.len() {
            return Some(segment.source_start_secs + remaining.min(duration));
        }
        remaining -= duration;
    }
    None
}

fn shared_source_range(ranges: impl IntoIterator<Item = (f64, f64)>) -> Option<(f64, f64)> {
    let mut ranges = ranges.into_iter();
    let (mut shared_start, mut shared_end) = ranges.next()?;
    for (start, end) in ranges {
        shared_start = shared_start.max(start);
        shared_end = shared_end.min(end);
    }
    (shared_end > shared_start).then_some((shared_start, shared_end))
}

fn clamp_video_timeline_to_source_range(
    bundle: &mut BlipBundle,
    source_start_secs: f64,
    source_end_secs: f64,
) {
    let Some(segments) = bundle.video_segments.take() else {
        return;
    };
    let mut timeline_start_secs = 0.0;
    let mut deletions = Vec::new();
    let mut clipped_segments = Vec::with_capacity(segments.len());

    for mut segment in segments {
        let original_start_secs = segment.source_start_secs;
        let original_end_secs = segment.source_end_secs;
        let original_duration_secs = segment.duration_secs();
        let clipped_start_secs = original_start_secs.max(source_start_secs);
        let clipped_end_secs = original_end_secs.min(source_end_secs);

        if clipped_end_secs > clipped_start_secs {
            if clipped_start_secs > original_start_secs {
                deletions.push((
                    timeline_start_secs,
                    clipped_start_secs - original_start_secs,
                ));
            }
            if clipped_end_secs < original_end_secs {
                deletions.push((
                    timeline_start_secs + clipped_end_secs - original_start_secs,
                    original_end_secs - clipped_end_secs,
                ));
            }
            segment.source_start_secs = clipped_start_secs;
            segment.source_end_secs = clipped_end_secs;
            clipped_segments.push(segment);
        } else if original_duration_secs > 0.0 {
            deletions.push((timeline_start_secs, original_duration_secs));
        }
        timeline_start_secs += original_duration_secs;
    }

    deletions.sort_by(|left, right| right.0.total_cmp(&left.0));
    for (start_secs, duration_secs) in deletions {
        ripple_delete_ranges(&mut bundle.zoom_segments, start_secs, duration_secs);
    }
    bundle.video_segments = Some(clipped_segments);
}

fn ripple_delete_ranges(segments: &mut Vec<ZoomSegment>, start_secs: f64, duration_secs: f64) {
    let end_secs = start_secs + duration_secs;
    let map_time = |time_secs: f64| {
        if time_secs <= start_secs {
            time_secs
        } else if time_secs >= end_secs {
            time_secs - duration_secs
        } else {
            start_secs
        }
    };
    for segment in segments.iter_mut() {
        segment.start_secs = map_time(segment.start_secs);
        segment.end_secs = map_time(segment.end_secs);
    }
    segments.retain(|segment| segment.end_secs - segment.start_secs > 0.01);
}

fn ripple_insert_ranges(segments: &mut [ZoomSegment], start_secs: f64, duration_secs: f64) {
    for segment in segments {
        if segment.start_secs >= start_secs {
            segment.start_secs += duration_secs;
        }
        if segment.end_secs > start_secs {
            segment.end_secs += duration_secs;
        }
    }
}

fn preview_worker(
    decoder_inputs: Vec<(PathBuf, bool, f64)>,
    background_paths: Vec<PathBuf>,
    requests: async_channel::Receiver<PreviewRequest>,
    results: async_channel::Sender<PreviewResult>,
) {
    let mut decoders = decoder_inputs
        .into_iter()
        .filter_map(|(path, is_camera, start_offset_secs)| {
            match blip_avfoundation::VideoDecoder::open(&path) {
                Ok(decoder) => Some((decoder, is_camera, start_offset_secs)),
                Err(error) => {
                    eprintln!(
                        "blip-capture: failed to open video decoder for {}: {error}",
                        path.display()
                    );
                    None
                }
            }
        })
        .collect::<Vec<_>>();
    let background_count = background_paths.len();
    let (wallpaper_requests, wallpaper_request_receiver) = async_channel::unbounded();
    let (wallpaper_results, wallpaper_result_receiver) = async_channel::unbounded();
    if let Err(error) = std::thread::Builder::new()
        .name("wallpaper-decoder".into())
        .spawn(move || {
            wallpaper_worker(
                background_paths,
                wallpaper_request_receiver,
                wallpaper_results,
            );
        })
    {
        eprintln!("blip-capture: failed to start wallpaper decoder: {error}");
    }
    let mut background_frames = (0..background_count).map(|_| None).collect::<Vec<_>>();
    let mut failed_backgrounds = vec![false; background_count];
    let mut content_frame_cache = None;
    let mut compositor = match blip_compositor::FrameCompositor::new() {
        Ok(compositor) => compositor,
        Err(error) => {
            eprintln!("blip-capture: failed to create preview compositor: {error}");
            return;
        }
    };
    let mut output_compositor = match blip_compositor::FrameCompositor::new() {
        Ok(compositor) => compositor,
        Err(error) => {
            eprintln!("blip-capture: failed to create output compositor: {error}");
            return;
        }
    };

    while let Ok(mut request) = requests.recv_blocking() {
        while let Ok(latest_request) = requests.try_recv() {
            request = latest_request;
        }
        while let Ok((index, frame)) = wallpaper_result_receiver.try_recv() {
            if let Some(slot) = background_frames.get_mut(index) {
                *slot = frame.map(|frame| frame.0);
                failed_backgrounds[index] = slot.is_none();
            }
        }
        let wallpaper_pending = request.background_type == BackgroundType::Image
            && background_frames
                .get(request.background_preset)
                .is_some_and(Option::is_none)
            && !failed_backgrounds[request.background_preset];
        if wallpaper_pending {
            let _ = wallpaper_requests.send_blocking(request.background_preset);
        }

        if content_frame_cache
            .as_ref()
            .is_none_or(|(time_secs, _)| *time_secs != request.time_secs)
        {
            match render_content_frame(&mut decoders, &mut compositor, request.time_secs) {
                Ok(content_frame) => {
                    content_frame_cache = Some((request.time_secs, content_frame));
                }
                Err(error) => {
                    eprintln!("blip-capture: {error}");
                    continue;
                }
            }
        }
        let Some((_, content_frame)) = &content_frame_cache else {
            continue;
        };
        let (_, cropped_dimensions) =
            screen_crop_rect(content_frame.dimensions, request.screen_crop);

        let preview_short_edge = cropped_dimensions.0.min(cropped_dimensions.1).min(1080);
        let (output_width, output_height) = output_dimensions(
            preview_short_edge,
            cropped_dimensions,
            request.aspect_ratio,
            request.padding,
        );
        if wallpaper_pending {
            while background_frames[request.background_preset].is_none()
                && !failed_backgrounds[request.background_preset]
            {
                let Ok((index, frame)) = wallpaper_result_receiver.recv_blocking() else {
                    break;
                };
                if let Some(slot) = background_frames.get_mut(index) {
                    *slot = frame.map(|frame| frame.0);
                    failed_backgrounds[index] = slot.is_none();
                }
            }
        }
        let wallpaper = (request.background_type == BackgroundType::Image)
            .then(|| background_frames.get(request.background_preset))
            .flatten()
            .and_then(Option::as_ref);
        match render_output_frame(
            &mut output_compositor,
            &mut compositor,
            &content_frame.screen,
            content_frame.camera.as_ref(),
            content_frame.dimensions,
            (output_width, output_height),
            wallpaper,
            request.background_type,
            request.padding,
            request.border_radius,
            request.shadow,
            request.zoom,
            request.camera_layout,
            request.screen_crop,
        ) {
            Ok(frame) => {
                if results
                    .send_blocking(PreviewResult {
                        playback_generation: request.playback_generation,
                        timeline_time_secs: request.timeline_time_secs,
                        frame: PreviewFrame(frame),
                        screen: PreviewFrame(content_frame.screen.clone()),
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => eprintln!("blip-capture: error composing preview frame: {error}"),
        }
    }
}

fn render_content_frame(
    decoders: &mut [(blip_avfoundation::VideoDecoder, bool, f64)],
    compositor: &mut blip_compositor::FrameCompositor,
    time_secs: f64,
) -> Result<ContentFrame, String> {
    let mut active_frames = Vec::new();
    for (decoder, is_camera, start_offset_secs) in decoders {
        if let Some(input_time_secs) = aligned_input_time(time_secs, *start_offset_secs)
            && let Ok(pixel_buffer) = decoder.frame_at(input_time_secs)
        {
            active_frames.push((pixel_buffer, decoder.width(), decoder.height(), *is_camera));
        }
    }
    if active_frames.is_empty() {
        return Err(format!("no frames decoded at {time_secs}s"));
    }
    let (canvas_width, canvas_height) = active_frames
        .iter()
        .find(|(_, _, _, is_camera)| !*is_camera)
        .or_else(|| active_frames.first())
        .map(|(_, width, height, _)| (*width, *height))
        .unwrap_or((1920, 1080));
    let has_screen = active_frames.iter().any(|(_, _, _, is_camera)| !*is_camera);
    let camera = has_screen
        .then(|| {
            active_frames
                .iter()
                .rev()
                .find(|(_, _, _, is_camera)| *is_camera)
                .map(|(pixel_buffer, _, _, _)| pixel_buffer.clone())
        })
        .flatten();
    let screen_frames = active_frames
        .iter()
        .filter(|(_, _, _, is_camera)| !has_screen || !*is_camera);
    let mut sources = Vec::new();
    let mut items = Vec::new();
    for (pixel_buffer, _, _, _) in screen_frames {
        sources.push(blip_compositor::CompositorSource {
            pixel_buffer,
            content_rect: None,
        });
        items.push(blip_compositor::CompositorItem {
            content: blip_compositor::CompositorItemContent::Source(sources.len() - 1),
            transform: blip_compositor::ItemTransform::new([0.5, 0.5], [1.0, 1.0]),
        });
    }
    compositor
        .render(&sources, &items, (canvas_width, canvas_height))
        .map(|screen| ContentFrame {
            screen,
            camera,
            dimensions: (canvas_width, canvas_height),
        })
        .map_err(|error| format!("error composing content frame: {error}"))
}

fn aligned_input_time(timeline_time_secs: f64, start_offset_secs: f64) -> Option<f64> {
    let input_time_secs = timeline_time_secs - start_offset_secs;
    (input_time_secs >= 0.0).then_some(input_time_secs)
}

fn render_output_frame(
    compositor: &mut blip_compositor::FrameCompositor,
    overlay_compositor: &mut blip_compositor::FrameCompositor,
    content_frame: &core_video::pixel_buffer::CVPixelBuffer,
    camera_frame: Option<&core_video::pixel_buffer::CVPixelBuffer>,
    canvas_dimensions: (usize, usize),
    output_dimensions: (usize, usize),
    wallpaper: Option<&core_video::pixel_buffer::CVPixelBuffer>,
    background_type: BackgroundType,
    padding: f32,
    border_radius: f32,
    shadow: f32,
    zoom: blip_compositor::OutputTransform,
    camera_layout: CameraLayout,
    screen_crop: Option<ScreenCrop>,
) -> Result<core_video::pixel_buffer::CVPixelBuffer, String> {
    let (screen_content_rect, cropped_dimensions) =
        screen_crop_rect(canvas_dimensions, screen_crop);
    let (canvas_width, canvas_height) = cropped_dimensions;
    let (output_width, output_height) = output_dimensions;
    let shortest_dimension = output_width.min(output_height) as f32;
    let inset = shortest_dimension * padding / 200.0;
    let maximum_size = [
        (output_width as f32 - inset * 2.0).max(0.0) / output_width as f32,
        (output_height as f32 - inset * 2.0).max(0.0) / output_height as f32,
    ];
    let content_size = aspect_fit_size(
        maximum_size,
        (canvas_width as f64, canvas_height as f64),
        (output_width as f64, output_height as f64),
    );
    let content_shortest_dimension =
        (content_size[0] * output_width as f32).min(content_size[1] * output_height as f32);
    let corner_radius = content_shortest_dimension * border_radius / 100.0;
    let shadow_strength = shadow / 50.0;
    let box_shadow = blip_compositor::BoxShadow::new(
        [0.0, content_shortest_dimension * 0.02 * shadow_strength],
        [0.0, 0.0, 0.0, 0.9 * shadow_strength],
    )
    .with_blur_radius(content_shortest_dimension * 0.02 * shadow_strength)
    .with_spread_radius(content_shortest_dimension * -0.001 * shadow_strength);
    let mut output_sources = Vec::with_capacity(2);
    let background_content = if let Some(wallpaper) = wallpaper {
        output_sources.push(blip_compositor::CompositorSource {
            pixel_buffer: wallpaper,
            content_rect: Some(cover_content_rect(
                wallpaper.get_width(),
                wallpaper.get_height(),
                output_width,
                output_height,
            )),
        });
        blip_compositor::CompositorItemContent::Source(0)
    } else {
        preview_background(background_type)
    };
    let content_source = output_sources.len();
    output_sources.push(blip_compositor::CompositorSource {
        pixel_buffer: content_frame,
        content_rect: Some(screen_content_rect),
    });
    let content_transform = blip_compositor::ItemTransform::new([0.5, 0.5], content_size)
        .with_corner_radius(corner_radius)
        .with_squircle(true)
        .with_box_shadow(box_shadow);
    let output_items = [
        blip_compositor::CompositorItem {
            content: background_content,
            transform: blip_compositor::ItemTransform::new([0.5, 0.5], [1.0, 1.0]),
        },
        blip_compositor::CompositorItem {
            content: blip_compositor::CompositorItemContent::Source(content_source),
            transform: content_transform,
        },
    ];
    let output = compositor
        .render_with_output_transform(
            &output_sources,
            &output_items,
            output_dimensions,
            map_output_transform_to_item(
                map_output_transform_to_crop(zoom, screen_crop),
                content_transform,
            ),
        )
        .map_err(|error| format!("error composing output frame: {error}"))?;
    let Some(camera_frame) = camera_frame else {
        return Ok(output);
    };
    let camera_is_wide = camera_frame.get_width() >= camera_frame.get_height();
    let (camera_content_rect, camera_dimensions) = camera_crop_rect(
        (camera_frame.get_width(), camera_frame.get_height()),
        camera_layout.crop,
    );
    let sources = [
        blip_compositor::CompositorSource {
            pixel_buffer: &output,
            content_rect: None,
        },
        blip_compositor::CompositorSource {
            pixel_buffer: camera_frame,
            content_rect: Some(camera_content_rect),
        },
    ];
    let items = [
        blip_compositor::CompositorItem {
            content: blip_compositor::CompositorItemContent::Source(0),
            transform: blip_compositor::ItemTransform::new([0.5, 0.5], [1.0, 1.0]),
        },
        blip_compositor::CompositorItem {
            content: blip_compositor::CompositorItemContent::Source(1),
            transform: camera_transform(
                camera_dimensions,
                camera_is_wide,
                (output_width as f64, output_height as f64),
                camera_layout,
                zoom.scale,
            ),
        },
    ];
    overlay_compositor
        .render(&sources, &items, output_dimensions)
        .map_err(|error| format!("error composing camera overlay: {error}"))
}

fn export_worker(job: ExportJob, events: async_channel::Sender<ExportEvent>) {
    let output = job.output.clone();
    let result = render_export(job, &events);
    if result.is_err() {
        std::fs::remove_file(&output).ok();
    }
    let _ = events.send_blocking(ExportEvent::Finished(result.map(|()| output)));
}

fn render_export(
    job: ExportJob,
    events: &async_channel::Sender<ExportEvent>,
) -> Result<(), String> {
    let mut decoders = job
        .decoder_inputs
        .into_iter()
        .map(|(path, is_camera, start_offset_secs)| {
            blip_avfoundation::VideoDecoder::open(&path)
                .map(|decoder| (decoder, is_camera, start_offset_secs))
                .map_err(|error| format!("Could not open {}: {error}", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let wallpaper = job.wallpaper.as_deref().map(load_wallpaper).transpose()?;
    let mut content_compositor = blip_compositor::FrameCompositor::new()
        .map_err(|error| format!("Could not create content renderer: {error}"))?;
    let mut output_compositor = blip_compositor::FrameCompositor::new()
        .map_err(|error| format!("Could not create output renderer: {error}"))?;
    let source_dimensions = decoders
        .iter()
        .find(|(_, is_camera, _)| !*is_camera)
        .or_else(|| decoders.first())
        .map(|(decoder, _, _)| (decoder.width(), decoder.height()))
        .unwrap_or((1920, 1080));
    let (_, cropped_dimensions) = screen_crop_rect(source_dimensions, job.bundle.screen_crop);
    let dimensions = output_dimensions(
        job.short_edge,
        cropped_dimensions,
        job.aspect_ratio,
        job.padding,
    );
    let has_audio = job.audio_mixer.has_sources();
    let mut writer = blip_avfoundation::Mp4Writer::new_with_file_type_and_audio(
        &job.output,
        dimensions.0,
        dimensions.1,
        job.fps,
        job.file_type,
        has_audio,
    )
    .map_err(|error| format!("Could not create export: {error}"))?;
    let frame_count = (job.duration_secs * f64::from(job.fps)).ceil().max(1.0) as usize;

    for frame_index in 0..frame_count {
        let timeline_time_secs = frame_index as f64 / f64::from(job.fps);
        let source_time_secs = source_time_at(&job.bundle, timeline_time_secs)
            .ok_or_else(|| "Could not map the export timeline to source video.".to_owned())?;
        let content_frame =
            render_content_frame(&mut decoders, &mut content_compositor, source_time_secs)?;
        let output_frame = render_output_frame(
            &mut output_compositor,
            &mut content_compositor,
            &content_frame.screen,
            content_frame.camera.as_ref(),
            content_frame.dimensions,
            dimensions,
            wallpaper.as_ref(),
            job.background_type,
            job.padding,
            job.border_radius,
            job.shadow,
            zoom_transform_at(&job.bundle.zoom_segments, timeline_time_secs),
            job.bundle.camera_layout,
            job.bundle.screen_crop,
        )?;
        let timestamp = Duration::from_secs_f64(timeline_time_secs);
        while !writer
            .append_core_video(&output_frame, timestamp)
            .map_err(|error| format!("Could not encode frame: {error}"))?
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = events.send_blocking(ExportEvent::Progress(
            (frame_index + 1) as f32 / frame_count as f32,
        ));
    }
    if has_audio {
        const AUDIO_PACKET_FRAMES: usize = 1_024;
        let total_audio_frames = (job.duration_secs * f64::from(blip_audio::MIX_SAMPLE_RATE))
            .ceil()
            .max(1.0) as usize;
        let mut audio_frame = 0;
        while audio_frame < total_audio_frames {
            let frame_count = AUDIO_PACKET_FRAMES.min(total_audio_frames - audio_frame);
            let timestamp_secs = audio_frame as f64 / f64::from(blip_audio::MIX_SAMPLE_RATE);
            let packet = job
                .audio_mixer
                .render(timestamp_secs, frame_count)
                .map_err(|error| format!("Could not mix audio: {error}"))?;
            let timestamp = Duration::from_secs_f64(timestamp_secs);
            while !writer
                .append_audio(&packet, timestamp)
                .map_err(|error| format!("Could not encode audio: {error}"))?
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            audio_frame += frame_count;
        }
    }
    writer
        .finish()
        .map_err(|error| format!("Could not finish export: {error}"))
}

fn zoom_target_worker(
    screen_input: Option<PathBuf>,
    requests: async_channel::Receiver<f64>,
    results: async_channel::Sender<(f64, PreviewFrame)>,
) {
    let Some(screen_input) = screen_input else {
        return;
    };
    let mut decoder = match blip_avfoundation::VideoDecoder::open(&screen_input) {
        Ok(decoder) => decoder,
        Err(error) => {
            eprintln!(
                "blip-capture: failed to open zoom target decoder for {}: {error}",
                screen_input.display()
            );
            return;
        }
    };

    while let Ok(mut time_secs) = requests.recv_blocking() {
        while let Ok(latest_time_secs) = requests.try_recv() {
            time_secs = latest_time_secs;
        }
        match decoder.frame_at(time_secs) {
            Ok(frame) => {
                if results
                    .send_blocking((time_secs, PreviewFrame(frame)))
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                eprintln!("blip-capture: failed to decode zoom target at {time_secs}s: {error}")
            }
        }
    }
}

fn wallpaper_worker(
    background_paths: Vec<PathBuf>,
    priority_requests: async_channel::Receiver<usize>,
    results: async_channel::Sender<(usize, Option<PreviewFrame>)>,
) {
    let mut decoded = vec![false; background_paths.len()];
    let mut next_preload = 0;

    loop {
        let index = if let Ok(index) = priority_requests.try_recv() {
            index
        } else {
            while decoded.get(next_preload).copied() == Some(true) {
                next_preload += 1;
            }
            if next_preload >= background_paths.len() {
                break;
            }
            let index = next_preload;
            next_preload += 1;
            index
        };
        if decoded.get(index).copied() != Some(false) {
            continue;
        }

        let frame = match load_wallpaper(&background_paths[index]) {
            Ok(frame) => Some(PreviewFrame(frame)),
            Err(error) => {
                eprintln!(
                    "blip-capture: failed to decode background {}: {error}",
                    background_paths[index].display()
                );
                None
            }
        };
        decoded[index] = true;
        if results.send_blocking((index, frame)).is_err() {
            break;
        }
    }
}

fn preview_background(background_type: BackgroundType) -> blip_compositor::CompositorItemContent {
    match background_type {
        BackgroundType::Color => {
            blip_compositor::CompositorItemContent::Color([0.173, 0.184, 0.227, 1.0])
        }
        BackgroundType::Image => {
            blip_compositor::CompositorItemContent::Color([0.173, 0.184, 0.227, 1.0])
        }
        BackgroundType::Gradient => blip_compositor::CompositorItemContent::Gradient {
            start: [0.435, 0.239, 0.518, 1.0],
            end: [0.118, 0.212, 0.373, 1.0],
        },
    }
}

fn zoom_transform_at(segments: &[ZoomSegment], time_secs: f64) -> blip_compositor::OutputTransform {
    let Some(segment) = segments
        .iter()
        .filter(|segment| {
            time_secs >= segment.start_secs
                && time_secs <= segment.end_secs + segment.transition.duration_secs()
        })
        .max_by(|a, b| a.start_secs.total_cmp(&b.start_secs))
    else {
        return blip_compositor::OutputTransform::IDENTITY;
    };
    let transition_secs = segment.transition.duration_secs();
    let zoom_in = if segment.start_secs <= 0.0 {
        1.0
    } else {
        ((time_secs - segment.start_secs) / transition_secs).clamp(0.0, 1.0)
    };
    let zoom_out =
        ((segment.end_secs + transition_secs - time_secs) / transition_secs).clamp(0.0, 1.0);
    let attack = cubic_bezier_ease(zoom_in, 0.2, 0.8, 0.2, 1.0);
    let release = cubic_bezier_ease(zoom_out, 0.4, 0.0, 0.6, 1.0);
    let progress = attack.min(release) as f32;
    let mut transform = blip_compositor::OutputTransform {
        center: segment.target,
        scale: 1.0 + (segment.amount.clamp(1.0, 5.0) - 1.0) * progress,
    };

    if attack < 1.0
        && let Some(previous) = segments
            .iter()
            .filter(|previous| {
                previous.start_secs < segment.start_secs
                    && previous.end_secs <= segment.start_secs
                    && segment.start_secs < previous.end_secs + previous.transition.duration_secs()
            })
            .max_by(|a, b| a.start_secs.total_cmp(&b.start_secs))
    {
        let previous_transition_secs = previous.transition.duration_secs();
        let previous_zoom_out = ((previous.end_secs + previous_transition_secs
            - segment.start_secs)
            / previous_transition_secs)
            .clamp(0.0, 1.0);
        let previous_progress = cubic_bezier_ease(previous_zoom_out, 0.4, 0.0, 0.6, 1.0) as f32;
        let previous_scale = 1.0 + (previous.amount.clamp(1.0, 5.0) - 1.0) * previous_progress;
        let incoming_scale = 1.0 + (segment.amount.clamp(1.0, 5.0) - 1.0) * release as f32;
        let attack = attack as f32;
        transform.center = [
            previous.target[0] + (transform.center[0] - previous.target[0]) * attack,
            previous.target[1] + (transform.center[1] - previous.target[1]) * attack,
        ];
        transform.scale = previous_scale + (incoming_scale - previous_scale) * attack;
    }

    transform
}

fn cubic_bezier_ease(
    progress: f64,
    first_x: f64,
    first_y: f64,
    second_x: f64,
    second_y: f64,
) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    if progress == 0.0 || progress == 1.0 {
        return progress;
    }
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let parameter = (low + high) * 0.5;
        let x = cubic_bezier_coordinate(parameter, first_x, second_x);
        if x < progress {
            low = parameter;
        } else {
            high = parameter;
        }
    }
    cubic_bezier_coordinate((low + high) * 0.5, first_y, second_y)
}

fn cubic_bezier_coordinate(parameter: f64, first: f64, second: f64) -> f64 {
    let inverse = 1.0 - parameter;
    3.0 * inverse * inverse * parameter * first
        + 3.0 * inverse * parameter * parameter * second
        + parameter * parameter * parameter
}

fn bundled_backgrounds() -> Vec<BackgroundImage> {
    let cache_directory = std::env::temp_dir().join("blip-backgrounds");
    if std::fs::create_dir_all(&cache_directory).is_err() {
        return Vec::new();
    }

    BUNDLED_BACKGROUNDS
        .iter()
        .filter_map(|(name, image_bytes, thumbnail_bytes)| {
            let image = cache_directory.join(name);
            let thumbnail = cache_directory.join(format!("thumbnail-{name}"));
            if cache_background(&image, image_bytes)
                && cache_background(&thumbnail, thumbnail_bytes)
            {
                Some(BackgroundImage {
                    name: (*name).to_owned(),
                    image,
                    thumbnail,
                })
            } else {
                None
            }
        })
        .collect()
}

fn cache_background(path: &Path, bytes: &[u8]) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.len() == bytes.len() as u64)
        || std::fs::write(path, bytes).is_ok()
}

fn load_wallpaper(path: &Path) -> Result<core_video::pixel_buffer::CVPixelBuffer, String> {
    let image = image::open(path)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let width = image.width() as usize;
    let height = image.height() as usize;
    let surface_properties = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
    let attributes = CFDictionary::from_CFType_pairs(&[
        (
            CFString::from(core_video::pixel_buffer::CVPixelBufferKeys::IOSurfaceProperties),
            surface_properties.as_CFType(),
        ),
        (
            CFString::from(core_video::pixel_buffer::CVPixelBufferKeys::MetalCompatibility),
            CFBoolean::true_value().as_CFType(),
        ),
    ]);
    let pixel_buffer = core_video::pixel_buffer::CVPixelBuffer::new(
        core_video::pixel_buffer::kCVPixelFormatType_32BGRA,
        width,
        height,
        Some(&attributes),
    )
    .map_err(|status| format!("failed to create pixel buffer ({status})"))?;
    let status = pixel_buffer.lock_base_address(0);
    if status != 0 {
        return Err(format!("failed to lock pixel buffer ({status})"));
    }
    let result = (|| {
        let stride = pixel_buffer.get_bytes_per_row();
        let length = stride
            .checked_mul(height)
            .ok_or_else(|| "background image is too large".to_owned())?;
        let destination = unsafe {
            let address = pixel_buffer.get_base_address().cast::<u8>();
            if address.is_null() {
                return Err("background pixel buffer has no base address".to_owned());
            }
            std::slice::from_raw_parts_mut(address, length)
        };
        for (y, row) in image.rows().enumerate() {
            for (x, pixel) in row.enumerate() {
                let offset = y * stride + x * 4;
                destination[offset..offset + 4]
                    .copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        Ok(())
    })();
    let unlock_status = pixel_buffer.unlock_base_address(0);
    if unlock_status != 0 {
        return Err(format!("failed to unlock pixel buffer ({unlock_status})"));
    }
    result.map(|()| pixel_buffer)
}

fn cover_content_rect(
    source_width: usize,
    source_height: usize,
    output_width: usize,
    output_height: usize,
) -> blip_compositor::ContentRect {
    let source_aspect = source_width as f64 / source_height as f64;
    let output_aspect = output_width as f64 / output_height as f64;
    if source_aspect > output_aspect {
        let width = source_height as f64 * output_aspect;
        blip_compositor::ContentRect {
            x: (source_width as f64 - width) * 0.5,
            y: 0.0,
            width,
            height: source_height as f64,
        }
    } else {
        let height = source_width as f64 / output_aspect;
        blip_compositor::ContentRect {
            x: 0.0,
            y: (source_height as f64 - height) * 0.5,
            width: source_width as f64,
            height,
        }
    }
}

fn playback_position(start_secs: f64, elapsed_secs: f64, duration_secs: f64) -> (f64, bool) {
    let time_secs = (start_secs + elapsed_secs).clamp(0.0, duration_secs);
    (time_secs, time_secs >= duration_secs)
}

fn include_timeline_fps(current: Option<u32>, nominal_fps: f64) -> Option<u32> {
    let fps = nominal_fps.round().max(1.0) as u32;
    Some(current.unwrap_or(fps).max(fps))
}

fn timeline_time_fraction(
    time_secs: f64,
    view_start_secs: f64,
    view_duration_secs: f64,
) -> Option<f32> {
    if view_duration_secs <= 0.0
        || time_secs < view_start_secs
        || time_secs > view_start_secs + view_duration_secs
    {
        return None;
    }
    Some(((time_secs - view_start_secs) / view_duration_secs) as f32)
}

fn timeline_range_fraction(
    start_secs: f64,
    end_secs: f64,
    view_start_secs: f64,
    view_duration_secs: f64,
) -> Option<(f32, f32)> {
    if view_duration_secs <= 0.0 {
        return None;
    }
    let visible_start_secs = start_secs.max(view_start_secs);
    let visible_end_secs = end_secs.min(view_start_secs + view_duration_secs);
    if visible_end_secs <= visible_start_secs {
        return None;
    }
    Some((
        ((visible_start_secs - view_start_secs) / view_duration_secs) as f32,
        ((visible_end_secs - visible_start_secs) / view_duration_secs) as f32,
    ))
}

fn timeline_segment_range_fraction(
    start_secs: f64,
    end_secs: f64,
    view_start_secs: f64,
    view_duration_secs: f64,
) -> Option<(f32, f32)> {
    if view_duration_secs <= 0.0
        || end_secs <= view_start_secs
        || start_secs >= view_start_secs + view_duration_secs
    {
        return None;
    }
    Some((
        ((start_secs - view_start_secs) / view_duration_secs) as f32,
        ((end_secs - start_secs) / view_duration_secs) as f32,
    ))
}

fn zoom_segment_range_at(
    cursor_secs: f64,
    timeline_duration_secs: f64,
    segments: &[ZoomSegment],
) -> Option<(f64, f64)> {
    let duration_secs = timeline_duration_secs.min(3.0).max(0.0);
    let cursor_secs = cursor_secs.clamp(0.0, timeline_duration_secs.max(0.0));
    let available_start_secs = segments
        .iter()
        .filter(|segment| segment.end_secs <= cursor_secs)
        .map(|segment| segment.end_secs)
        .max_by(f64::total_cmp)
        .unwrap_or(0.0);
    let available_end_secs = segments
        .iter()
        .filter(|segment| segment.start_secs >= cursor_secs)
        .map(|segment| segment.start_secs)
        .min_by(f64::total_cmp)
        .unwrap_or(timeline_duration_secs);
    if available_end_secs - available_start_secs < duration_secs {
        return None;
    }
    let start_secs = cursor_secs.clamp(
        available_start_secs,
        (available_end_secs - duration_secs).max(available_start_secs),
    );
    Some((start_secs, start_secs + duration_secs))
}

fn zoomed_timeline_view(
    view_start_secs: f64,
    view_duration_secs: f64,
    timeline_duration_secs: f64,
    focal_fraction: f64,
    delta: f32,
) -> (f64, f64) {
    if timeline_duration_secs <= 0.0 {
        return (0.0, 0.0);
    }
    let focal_fraction = focal_fraction.clamp(0.0, 1.0);
    let view_duration_secs = view_duration_secs.clamp(0.0, timeline_duration_secs);
    let focal_time_secs = view_start_secs + focal_fraction * view_duration_secs;
    let minimum_duration_secs = (timeline_duration_secs / 10.0)
        .max(0.05)
        .min(timeline_duration_secs);
    let duration_secs = (view_duration_secs * (-f64::from(delta)).exp())
        .clamp(minimum_duration_secs, timeline_duration_secs);
    let start_secs = clamp_timeline_view_start(
        focal_time_secs - focal_fraction * duration_secs,
        duration_secs,
        timeline_duration_secs,
    );
    (start_secs, duration_secs)
}

fn timeline_extent_secs(duration_secs: f64) -> f64 {
    duration_secs.max(0.0) + TIMELINE_TRAILING_SPACE_SECS
}

fn clamp_timeline_view_start(
    view_start_secs: f64,
    view_duration_secs: f64,
    timeline_extent_secs: f64,
) -> f64 {
    let maximum_start_secs = (timeline_extent_secs - view_duration_secs).max(0.0);
    view_start_secs.clamp(0.0, maximum_start_secs)
}

fn panned_timeline_view(
    view_start_secs: f64,
    view_duration_secs: f64,
    timeline_duration_secs: f64,
    delta_fraction: f64,
) -> f64 {
    clamp_timeline_view_start(
        view_start_secs - delta_fraction * view_duration_secs,
        view_duration_secs,
        timeline_duration_secs,
    )
}

fn timeline_ruler(view_start_secs: f64, view_duration_secs: f64) -> Div {
    div().h(px(24.0)).pl_4().flex().items_end().child(
        canvas(
            move |bounds, window, _| {
                let width = f64::from(bounds.size.width);
                if view_duration_secs <= 0.0 || width <= 0.0 {
                    return Vec::new();
                }

                let interval = timeline_ruler_interval(view_duration_secs / width);
                let subdivisions = if interval <= 0.1 { 1 } else { 2 };
                let minor_interval = interval / subdivisions as f64;
                let first_index = (view_start_secs / minor_interval).ceil() as usize;
                let last_index =
                    ((view_start_secs + view_duration_secs) / minor_interval).floor() as usize;
                if first_index > last_index {
                    return Vec::new();
                }
                let mut ticks = Vec::with_capacity(last_index - first_index + 1);
                for index in first_index..=last_index {
                    let time_secs = index as f64 * minor_interval;
                    let x = bounds.left()
                        + bounds.size.width
                            * ((time_secs - view_start_secs) / view_duration_secs) as f32;
                    let major = index % subdivisions == 0;
                    let label = major.then(|| {
                        let text: SharedString = timeline_ruler_label(time_secs, interval).into();
                        let run = TextRun {
                            len: text.len(),
                            font: window.text_style().font(),
                            color: rgb(theme::TEXT_DIM).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        window
                            .text_system()
                            .shape_line(text, px(10.0), &[run], None)
                    });
                    ticks.push((x, major, label));
                }
                ticks
            },
            move |bounds, ticks, window, cx| {
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.left(), bounds.bottom() - px(1.0)),
                        size(bounds.size.width, px(1.0)),
                    ),
                    rgb(theme::BORDER_SUBTLE),
                ));
                for (x, major, label) in ticks {
                    let height = if major { px(8.0) } else { px(4.0) };
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.bottom() - height), size(px(1.0), height)),
                        rgb(theme::TEXT_DIM),
                    ));
                    if let Some(label) = label {
                        let max_x = (bounds.right() - label.width()).max(bounds.left());
                        let label_x = (x - label.width() / 2.0).clamp(bounds.left(), max_x);
                        let _ = label.paint(
                            point(label_x, bounds.top()),
                            px(12.0),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                }
            },
        )
        .h_full()
        .flex_1(),
    )
}

fn timeline_timecode(time_secs: f64, fps: u32) -> String {
    let fps = u64::from(fps.max(1));
    let total_frames = (time_secs.max(0.0) * fps as f64).floor() as u64;
    let total_seconds = total_frames / fps;
    format!(
        "{:02}:{:02}:{:02}",
        total_seconds / 60,
        total_seconds % 60,
        total_frames % fps,
    )
}

fn timeline_ruler_interval(seconds_per_pixel: f64) -> f64 {
    let target_interval = (seconds_per_pixel * 72.0).max(0.001);
    let magnitude = 10.0_f64.powf(target_interval.log10().floor());
    let normalized = target_interval / magnitude;
    let step = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    (step * magnitude).max(0.1)
}

fn timeline_ruler_label(time_secs: f64, interval: f64) -> String {
    let decimals = if interval < 1.0 {
        (-interval.log10()).ceil() as usize
    } else {
        0
    };
    if time_secs >= 60.0 {
        let minutes = (time_secs / 60.0).floor() as u64;
        let seconds = time_secs - minutes as f64 * 60.0;
        if decimals == 0 {
            format!("{minutes}:{seconds:02.0}")
        } else {
            format!("{minutes}:{seconds:02.*}", decimals)
        }
    } else if decimals == 0 {
        format!("{time_secs:.0}s")
    } else {
        format!("{time_secs:.decimals$}s")
    }
}

fn input_is_camera(input: &crate::bundle::BundleInput, index: usize) -> bool {
    input.kind == BundleInputKind::Video
        && (input.id.eq_ignore_ascii_case("camera")
            || input.name.to_lowercase().contains("camera")
            || (index > 0 && !input.id.eq_ignore_ascii_case("screen")))
}

fn aspect_fit_size(
    maximum_size: [f32; 2],
    (source_width, source_height): (f64, f64),
    (canvas_width, canvas_height): (f64, f64),
) -> [f32; 2] {
    if source_height == 0.0 || canvas_height == 0.0 || maximum_size[1] == 0.0 {
        return maximum_size;
    }
    let source_aspect = source_width / source_height;
    let canvas_aspect = canvas_width / canvas_height;
    let box_aspect = f64::from(maximum_size[0]) * canvas_aspect / f64::from(maximum_size[1]);
    if source_aspect > box_aspect {
        if source_aspect == 0.0 {
            maximum_size
        } else {
            [
                maximum_size[0],
                (f64::from(maximum_size[0]) * canvas_aspect / source_aspect) as f32,
            ]
        }
    } else if canvas_aspect == 0.0 {
        maximum_size
    } else {
        [
            (f64::from(maximum_size[1]) * source_aspect / canvas_aspect) as f32,
            maximum_size[1],
        ]
    }
}

fn fit_dimensions(aspect_ratio: f32, maximum_width: f32, maximum_height: f32) -> (f32, f32) {
    let aspect_ratio = aspect_ratio.max(f32::EPSILON);
    if maximum_width / maximum_height > aspect_ratio {
        (maximum_height * aspect_ratio, maximum_height)
    } else {
        (maximum_width, maximum_width / aspect_ratio)
    }
}

fn normalize_screen_crop(crop: Option<ScreenCrop>) -> Option<ScreenCrop> {
    let crop = crop?;
    let mut position = crop.position;
    let mut size = crop.size;
    for axis in 0..2 {
        if !position[axis].is_finite() {
            position[axis] = 0.0;
        }
        position[axis] = position[axis].clamp(0.0, 0.99);
        if !size[axis].is_finite() || size[axis] <= 0.0 {
            size[axis] = 1.0 - position[axis];
        }
        let remaining = 1.0 - position[axis];
        size[axis] = size[axis].max(0.01_f32.min(remaining)).min(remaining);
    }
    let crop = ScreenCrop { position, size };
    let full = crop.position.iter().all(|value| value.abs() < 0.0001)
        && crop.size.iter().all(|value| (value - 1.0).abs() < 0.0001);
    (!full).then_some(crop)
}

fn screen_crop_rect(
    dimensions: (usize, usize),
    crop: Option<ScreenCrop>,
) -> (blip_compositor::ContentRect, (usize, usize)) {
    let crop = normalize_screen_crop(crop).unwrap_or(ScreenCrop::FULL);
    let width = dimensions.0.max(1) as f64;
    let height = dimensions.1.max(1) as f64;
    let crop_width = (width * f64::from(crop.size[0])).max(1.0);
    let crop_height = (height * f64::from(crop.size[1])).max(1.0);
    (
        blip_compositor::ContentRect {
            x: width * f64::from(crop.position[0]),
            y: height * f64::from(crop.position[1]),
            width: crop_width,
            height: crop_height,
        },
        (crop_width.round() as usize, crop_height.round() as usize),
    )
}

fn output_dimensions(
    short_edge: usize,
    (source_width, source_height): (usize, usize),
    aspect_ratio: OutputAspectRatio,
    padding: f32,
) -> (usize, usize) {
    let ratio = match aspect_ratio {
        OutputAspectRatio::Auto => {
            let source_ratio = source_width.max(1) as f64 / source_height.max(1) as f64;
            let padding_fraction = f64::from(padding.clamp(0.0, 50.0)) / 100.0;
            if source_ratio >= 1.0 {
                source_ratio * (1.0 - padding_fraction) + padding_fraction
            } else {
                1.0 / ((1.0 - padding_fraction) / source_ratio + padding_fraction)
            }
        }
        OutputAspectRatio::Wide => 16.0 / 9.0,
        OutputAspectRatio::Vertical => 9.0 / 16.0,
        OutputAspectRatio::Square => 1.0,
        OutputAspectRatio::Classic => 4.0 / 3.0,
        OutputAspectRatio::Tall => 3.0 / 4.0,
    };
    let short_edge = even_dimension(short_edge as f64);
    if ratio >= 1.0 {
        (even_dimension(short_edge as f64 * ratio), short_edge)
    } else {
        (short_edge, even_dimension(short_edge as f64 / ratio))
    }
}

fn even_dimension(value: f64) -> usize {
    ((value / 2.0).round() as usize * 2).max(2)
}

fn map_output_transform_to_item(
    mut output: blip_compositor::OutputTransform,
    item: blip_compositor::ItemTransform,
) -> blip_compositor::OutputTransform {
    output.center = [
        item.center[0] + (output.center[0] - 0.5) * item.size[0],
        item.center[1] + (output.center[1] - 0.5) * item.size[1],
    ];
    output
}

fn map_output_transform_to_crop(
    mut output: blip_compositor::OutputTransform,
    crop: Option<ScreenCrop>,
) -> blip_compositor::OutputTransform {
    let Some(crop) = normalize_screen_crop(crop) else {
        return output;
    };
    output.center = [
        ((output.center[0] - crop.position[0]) / crop.size[0]).clamp(0.0, 1.0),
        ((output.center[1] - crop.position[1]) / crop.size[1]).clamp(0.0, 1.0),
    ];
    output
}

fn camera_transform(
    source_dimensions: (f64, f64),
    camera_is_wide: bool,
    canvas_dimensions: (f64, f64),
    layout: CameraLayout,
    zoom_scale: f32,
) -> blip_compositor::ItemTransform {
    let zoom_progress = (zoom_scale - 1.0).clamp(0.0, 1.0);
    let reduction = layout.zoom_size_reduction.clamp(0.0, 50.0) / 100.0;
    let maximum_size = layout.size.clamp(0.0, 50.0) / 100.0 * (1.0 - reduction * zoom_progress);
    let source_aspect = source_dimensions.0 / source_dimensions.1.max(f64::EPSILON);
    let fixed_extent = canvas_dimensions.0.min(canvas_dimensions.1) * f64::from(maximum_size);
    let pixel_size = if camera_is_wide {
        [fixed_extent * source_aspect, fixed_extent]
    } else {
        [fixed_extent, fixed_extent / source_aspect.max(f64::EPSILON)]
    };
    let size = [
        (pixel_size[0] / canvas_dimensions.0) as f32,
        (pixel_size[1] / canvas_dimensions.1) as f32,
    ];
    let edge_padding = layout.edge_padding.clamp(0.0, 50.0) / 100.0
        * canvas_dimensions.0.min(canvas_dimensions.1) as f32;
    let edge_padding_x = edge_padding / canvas_dimensions.0 as f32;
    let edge_padding_y = edge_padding / canvas_dimensions.1 as f32;
    let left = edge_padding_x + size[0] * 0.5;
    let right = 1.0 - edge_padding_x - size[0] * 0.5;
    let top = edge_padding_y + size[1] * 0.5;
    let bottom = 1.0 - edge_padding_y - size[1] * 0.5;
    let center = match layout.position {
        CameraPosition::TopLeft => [left, top],
        CameraPosition::TopCenter => [0.5, top],
        CameraPosition::TopRight => [right, top],
        CameraPosition::MiddleLeft => [left, 0.5],
        CameraPosition::Center => [0.5, 0.5],
        CameraPosition::MiddleRight => [right, 0.5],
        CameraPosition::BottomLeft => [left, bottom],
        CameraPosition::BottomCenter => [0.5, bottom],
        CameraPosition::BottomRight => [right, bottom],
    };
    let corner_radius = (f64::from(size[0]) * canvas_dimensions.0)
        .min(f64::from(size[1]) * canvas_dimensions.1)
        * match layout.crop {
            CameraCrop::Circle => 0.5,
            CameraCrop::Squircle | CameraCrop::Squirectangle => 32.0 / 180.0,
        };
    let shortest_dimension = (f64::from(size[0]) * canvas_dimensions.0)
        .min(f64::from(size[1]) * canvas_dimensions.1) as f32;
    let shadow_strength = layout.shadow.clamp(0.0, 50.0) / 50.0;
    let box_shadow = blip_compositor::BoxShadow::new(
        [0.0, shortest_dimension * 0.02 * shadow_strength],
        [0.0, 0.0, 0.0, 0.9 * shadow_strength],
    )
    .with_blur_radius(shortest_dimension * 0.02 * shadow_strength)
    .with_spread_radius(shortest_dimension * -0.001 * shadow_strength);
    blip_compositor::ItemTransform::new(center, size)
        .with_corner_radius(corner_radius as f32)
        .with_squircle(layout.crop != CameraCrop::Circle)
        .with_box_shadow(box_shadow)
}

fn camera_crop_rect(
    (source_width, source_height): (usize, usize),
    crop: CameraCrop,
) -> (blip_compositor::ContentRect, (f64, f64)) {
    let (crop_width, crop_height) = match crop {
        CameraCrop::Circle | CameraCrop::Squircle => {
            let side = source_width.min(source_height);
            (side, side)
        }
        CameraCrop::Squirectangle => (source_width, source_height),
    };
    let crop_width = crop_width as f64;
    let crop_height = crop_height as f64;
    (
        blip_compositor::ContentRect {
            x: (source_width as f64 - crop_width) / 2.0,
            y: (source_height as f64 - crop_height) / 2.0,
            width: crop_width,
            height: crop_height,
        },
        (crop_width, crop_height),
    )
}

#[cfg(test)]
mod tests {
    use crate::bundle::{
        BlipBundle, CameraCrop, CameraLayout, CameraPosition, OutputAspectRatio, ScreenCrop,
        VideoSegment, VideoSegmentResizeMode, ZoomSegment, ZoomTransitionSpeed,
    };

    use super::{
        CropResizeHandle, VideoSegmentEdge, ZoomSegmentEdge, aligned_input_time, camera_crop_rect,
        camera_transform, can_split_video_segment, clamp_video_timeline_to_source_range,
        crop_contains, cubic_bezier_ease, duration_label, edit_video_cut, fit_dimensions,
        include_timeline_fps, map_output_transform_to_crop, map_output_transform_to_item,
        normalize_screen_crop, output_dimensions, panned_timeline_view, playback_position,
        resize_video_segment_range, resize_zoom_segment_range, resized_screen_crop,
        ripple_delete_ranges, ripple_insert_ranges, screen_crop_rect, shared_source_range,
        source_time_at, timeline_extent_secs, timeline_range_fraction, timeline_ruler_interval,
        timeline_ruler_label, timeline_segment_range_fraction, timeline_time_fraction,
        timeline_timecode, video_cut_gap_secs, video_segment_drag_layout, zoom_segment_range_at,
        zoom_transform_at, zoomed_timeline_view,
    };

    #[test]
    fn fixed_output_aspect_ratios_use_the_resolution_short_edge() {
        assert_eq!(
            output_dimensions(1080, (1920, 1080), OutputAspectRatio::Wide, 8.0),
            (1920, 1080)
        );
        assert_eq!(
            output_dimensions(1080, (1920, 1080), OutputAspectRatio::Vertical, 8.0),
            (1080, 1920)
        );
        assert_eq!(
            output_dimensions(1080, (1920, 1080), OutputAspectRatio::Square, 8.0),
            (1080, 1080)
        );
        assert_eq!(
            output_dimensions(1080, (1920, 1080), OutputAspectRatio::Classic, 8.0),
            (1440, 1080)
        );
        assert_eq!(
            output_dimensions(1080, (1920, 1080), OutputAspectRatio::Tall, 8.0),
            (1080, 1440)
        );
    }

    #[test]
    fn crop_preview_fits_without_changing_the_content_aspect_ratio() {
        let wide = fit_dimensions(16.0 / 9.0, 760.0, 440.0);
        let classic = fit_dimensions(4.0 / 3.0, 760.0, 440.0);
        assert_eq!(wide, (760.0, 427.5));
        assert!((classic.0 - 586.6667).abs() < 0.001);
        assert_eq!(classic.1, 440.0);
    }

    #[test]
    fn auto_output_has_equal_padding_on_every_side() {
        for source in [(1920, 1080), (1080, 1920)] {
            let output = output_dimensions(1080, source, OutputAspectRatio::Auto, 8.0);
            let inset = output.0.min(output.1) as f32 * 8.0 / 200.0;
            let maximum_size = [
                (output.0 as f32 - inset * 2.0) / output.0 as f32,
                (output.1 as f32 - inset * 2.0) / output.1 as f32,
            ];
            let content_size = super::aspect_fit_size(
                maximum_size,
                (source.0 as f64, source.1 as f64),
                (output.0 as f64, output.1 as f64),
            );
            let horizontal_padding = (output.0 as f32 - content_size[0] * output.0 as f32) / 2.0;
            let vertical_padding = (output.1 as f32 - content_size[1] * output.1 as f32) / 2.0;

            assert!((horizontal_padding - vertical_padding).abs() < 1.0);
        }
    }

    #[test]
    fn screen_crop_maps_normalized_geometry_to_source_pixels() {
        let crop = ScreenCrop {
            position: [0.25, 0.1],
            size: [0.5, 0.75],
        };
        let (rect, dimensions) = screen_crop_rect((1920, 1080), Some(crop));

        assert_eq!(rect.x, 480.0);
        assert!((rect.y - 108.0).abs() < 0.001);
        assert_eq!(rect.width, 960.0);
        assert!((rect.height - 810.0).abs() < 0.001);
        assert_eq!(dimensions, (960, 810));
    }

    #[test]
    fn screen_crop_clamps_invalid_values_and_canonicalizes_full_frame() {
        assert_eq!(normalize_screen_crop(Some(ScreenCrop::FULL)), None);
        assert_eq!(normalize_screen_crop(None), None);

        let crop = normalize_screen_crop(Some(ScreenCrop {
            position: [f32::NAN, 1.5],
            size: [f32::INFINITY, -1.0],
        }))
        .expect("partial crop should remain");
        assert_eq!(crop.position, [0.0, 0.99]);
        assert_eq!(crop.size[0], 1.0);
        assert!((crop.size[1] - 0.01).abs() < 0.000_001);
    }

    #[test]
    fn screen_crop_remaps_zoom_targets_to_visible_content() {
        let crop = Some(ScreenCrop {
            position: [0.25, 0.2],
            size: [0.5, 0.5],
        });
        let centered = map_output_transform_to_crop(
            blip_compositor::OutputTransform {
                center: [0.5, 0.45],
                scale: 2.0,
            },
            crop,
        );
        let outside = map_output_transform_to_crop(
            blip_compositor::OutputTransform {
                center: [0.0, 1.0],
                scale: 2.0,
            },
            crop,
        );

        assert!((centered.center[0] - 0.5).abs() < 0.000_001);
        assert!((centered.center[1] - 0.5).abs() < 0.000_001);
        assert_eq!(outside.center, [0.0, 1.0]);
    }

    #[test]
    fn screen_crop_detects_points_inside_the_selection() {
        let crop = ScreenCrop {
            position: [0.2, 0.25],
            size: [0.5, 0.5],
        };

        assert!(crop_contains(crop, [0.2, 0.25]));
        assert!(crop_contains(crop, [0.7, 0.75]));
        assert!(!crop_contains(crop, [0.1, 0.5]));
    }

    #[test]
    fn screen_crop_corner_resize_keeps_the_opposite_corner_fixed() {
        let crop = ScreenCrop {
            position: [0.2, 0.2],
            size: [0.5, 0.5],
        };
        let resized = resized_screen_crop(crop, CropResizeHandle::TopLeft, [0.1, 0.05]);

        assert_eq!(resized.position, [0.1, 0.05]);
        assert!((resized.position[0] + resized.size[0] - 0.7).abs() < 0.000_001);
        assert!((resized.position[1] + resized.size[1] - 0.7).abs() < 0.000_001);
    }

    #[test]
    fn camera_stays_edge_anchored_and_shrinks_with_zoom() {
        let layout = CameraLayout {
            size: 28.0,
            position: CameraPosition::BottomRight,
            edge_padding: 3.0,
            zoom_size_reduction: 50.0,
            shadow: 20.0,
            crop: CameraCrop::Squirectangle,
        };
        let unzoomed = camera_transform((1920.0, 1080.0), true, (1920.0, 1080.0), layout, 1.0);
        let zoomed = camera_transform((1920.0, 1080.0), true, (1920.0, 1080.0), layout, 2.0);

        assert!((unzoomed.size[0] - 0.28).abs() < 0.0001);
        assert!((zoomed.size[0] - 0.14).abs() < 0.0001);
        let unzoomed_right = (1.0 - unzoomed.center[0] - unzoomed.size[0] * 0.5) * 1920.0;
        let zoomed_right = (1.0 - zoomed.center[0] - zoomed.size[0] * 0.5) * 1920.0;
        let unzoomed_bottom = (1.0 - unzoomed.center[1] - unzoomed.size[1] * 0.5) * 1080.0;
        let zoomed_bottom = (1.0 - zoomed.center[1] - zoomed.size[1] * 0.5) * 1080.0;
        assert!((unzoomed_right - 32.4).abs() < 0.001);
        assert!((zoomed_right - 32.4).abs() < 0.001);
        assert!((unzoomed_bottom - 32.4).abs() < 0.001);
        assert!((zoomed_bottom - 32.4).abs() < 0.001);
    }

    #[test]
    fn camera_crop_matches_recording_shapes() {
        let (square, square_dimensions) = camera_crop_rect((1920, 1080), CameraCrop::Circle);
        let (full, full_dimensions) = camera_crop_rect((1920, 1200), CameraCrop::Squirectangle);

        assert_eq!(square_dimensions, (1080.0, 1080.0));
        assert_eq!(square.x, 420.0);
        assert_eq!(square.y, 0.0);
        assert_eq!(full_dimensions, (1920.0, 1200.0));
        assert_eq!(full.x, 0.0);
        assert_eq!(full.y, 0.0);
    }

    #[test]
    fn camera_shape_changes_preserve_the_orientation_extent() {
        let layout = CameraLayout::default();
        let canvas = (1920.0, 1080.0);
        let wide_square = camera_transform((1.0, 1.0), true, canvas, layout, 1.0);
        let wide_full = camera_transform((16.0, 9.0), true, canvas, layout, 1.0);
        let tall_square = camera_transform((1.0, 1.0), false, canvas, layout, 1.0);
        let tall_full = camera_transform((9.0, 16.0), false, canvas, layout, 1.0);

        assert!((wide_square.size[1] - wide_full.size[1]).abs() < 0.0001);
        assert!((tall_square.size[0] - tall_full.size[0]).abs() < 0.0001);
    }

    #[test]
    fn playback_position_uses_elapsed_wall_clock_time() {
        assert_eq!(playback_position(2.0, 1.5, 10.0), (3.5, false));
    }

    #[test]
    fn playback_position_stops_at_the_timeline_end() {
        assert_eq!(playback_position(9.0, 2.0, 10.0), (10.0, true));
    }

    #[test]
    fn timeline_uses_the_highest_input_frame_rate() {
        let fps = include_timeline_fps(None, 30.0);
        let fps = include_timeline_fps(fps, 60.0);
        let fps = include_timeline_fps(fps, 24.0);

        assert_eq!(fps, Some(60));
    }

    #[test]
    fn aligns_inputs_to_the_screen_timeline() {
        assert_eq!(aligned_input_time(1.0, 0.25), Some(0.75));
        assert_eq!(aligned_input_time(0.1, 0.25), None);
        assert_eq!(aligned_input_time(0.0, -0.125), Some(0.125));
    }

    #[test]
    fn source_range_is_shared_by_all_inputs() {
        assert_eq!(
            shared_source_range([(0.0, 10.0), (0.25, 9.75)]),
            Some((0.25, 9.75))
        );
        assert_eq!(shared_source_range([(0.0, 1.0), (2.0, 3.0)]), None);
    }

    #[test]
    fn existing_timeline_is_clipped_to_the_shared_source_range() {
        let mut bundle: BlipBundle = serde_json::from_str(
            r#"{
                "version": 1,
                "created_at": "2024-01-01T00:00:00Z",
                "inputs": [],
                "zoom_segments": [
                    {"id": 1, "start_secs": 1.0, "end_secs": 7.0,
                     "target": [0.5, 0.5], "amount": 2.0, "transition": "medium"}
                ],
                "video_segments": [
                    {"id": 1, "source_start_secs": 0.0, "source_end_secs": 4.0},
                    {"id": 2, "source_start_secs": 6.0, "source_end_secs": 10.0}
                ]
            }"#,
        )
        .expect("decode bundle");

        clamp_video_timeline_to_source_range(&mut bundle, 1.0, 9.0);

        let segments = bundle.video_segments.expect("video segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].source_start_secs, 1.0);
        assert_eq!(segments[1].source_end_secs, 9.0);
        assert_eq!(bundle.zoom_segments[0].start_secs, 0.0);
        assert_eq!(bundle.zoom_segments[0].end_secs, 6.0);
    }

    #[test]
    fn timeline_zoom_keeps_the_focal_time_in_place() {
        let focal_fraction = 0.25;
        let focal_time = 20.0 + 40.0 * focal_fraction;
        let (start, duration) = zoomed_timeline_view(20.0, 40.0, 100.0, focal_fraction, 0.5);

        assert!(duration < 40.0);
        assert!((start + duration * focal_fraction - focal_time).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_is_capped_at_ten_times() {
        let (_, duration) = zoomed_timeline_view(0.0, 100.0, 100.0, 0.5, 100.0);

        assert_eq!(duration, 10.0);
    }

    #[test]
    fn timeline_pan_moves_and_clamps_the_visible_range() {
        assert_eq!(panned_timeline_view(20.0, 40.0, 100.0, -0.25), 30.0);
        assert_eq!(panned_timeline_view(20.0, 40.0, 100.0, 1.0), 0.0);
        assert_eq!(panned_timeline_view(50.0, 40.0, 100.0, -1.0), 60.0);
    }

    #[test]
    fn zoomed_timeline_preserves_trailing_space() {
        let view_duration_secs = 4.0;
        let view_start_secs =
            panned_timeline_view(0.0, view_duration_secs, timeline_extent_secs(10.0), -10.0);

        assert_eq!(view_start_secs + view_duration_secs, 13.0);
    }

    #[test]
    fn timeline_extent_includes_three_seconds_of_trailing_space() {
        assert_eq!(timeline_extent_secs(10.0), 13.0);
    }

    #[test]
    fn live_video_resize_reflows_the_timeline_during_the_drag() {
        assert_eq!(
            video_segment_drag_layout(
                VideoSegmentResizeMode::Live,
                Some(VideoSegmentEdge::Start),
                2.0,
                4.0,
                3.0,
            ),
            (2.0, 5.0, 5.0)
        );
    }

    #[test]
    fn ghost_video_resize_preserves_the_original_space_during_the_drag() {
        assert_eq!(
            video_segment_drag_layout(
                VideoSegmentResizeMode::Ghost,
                Some(VideoSegmentEdge::Start),
                2.0,
                4.0,
                3.0,
            ),
            (3.0, 6.0, 6.0)
        );
    }

    #[test]
    fn timeline_positions_are_relative_to_the_visible_range() {
        assert_eq!(timeline_time_fraction(30.0, 20.0, 40.0), Some(0.25));
        assert_eq!(timeline_time_fraction(10.0, 20.0, 40.0), None);
        assert_eq!(
            timeline_range_fraction(10.0, 30.0, 20.0, 40.0),
            Some((0.0, 0.25))
        );
    }

    #[test]
    fn timeline_segment_edges_remain_outside_the_visible_range() {
        assert_eq!(
            timeline_segment_range_fraction(10.0, 30.0, 20.0, 40.0),
            Some((-0.25, 0.5))
        );
    }

    #[test]
    fn zoom_segment_preview_starts_at_the_cursor() {
        assert_eq!(zoom_segment_range_at(4.0, 10.0, &[]), Some((4.0, 7.0)));
    }

    #[test]
    fn zoom_segment_preview_sits_before_a_nearby_segment() {
        let segments = vec![ZoomSegment {
            id: 1,
            start_secs: 6.0,
            end_secs: 8.0,
            target: [0.5, 0.5],
            amount: 2.0,
            transition: ZoomTransitionSpeed::Medium,
        }];

        assert_eq!(
            zoom_segment_range_at(4.0, 10.0, &segments),
            Some((3.0, 6.0))
        );
    }

    #[test]
    fn zoom_segment_preview_is_hidden_in_an_undersized_gap() {
        let segments = vec![
            ZoomSegment {
                id: 1,
                start_secs: 1.0,
                end_secs: 4.0,
                target: [0.5, 0.5],
                amount: 2.0,
                transition: ZoomTransitionSpeed::Medium,
            },
            ZoomSegment {
                id: 2,
                start_secs: 6.0,
                end_secs: 9.0,
                target: [0.5, 0.5],
                amount: 2.0,
                transition: ZoomTransitionSpeed::Medium,
            },
        ];

        assert_eq!(zoom_segment_range_at(5.0, 10.0, &segments), None);
    }

    #[test]
    fn zoom_segment_preview_stays_inside_the_timeline() {
        assert_eq!(zoom_segment_range_at(9.0, 10.0, &[]), Some((7.0, 10.0)));
        assert_eq!(zoom_segment_range_at(0.0, 2.0, &[]), Some((0.0, 2.0)));
    }

    #[test]
    fn timeline_ruler_adapts_to_seconds_per_pixel() {
        assert_eq!(timeline_ruler_interval(0.1 / 800.0), 0.1);
        assert_eq!(timeline_ruler_interval(1.0 / 800.0), 0.1);
        assert_eq!(timeline_ruler_interval(5.0 / 800.0), 0.5);
        assert_eq!(timeline_ruler_interval(10.0 / 800.0), 1.0);
        assert_eq!(timeline_ruler_interval(60.0 / 800.0), 10.0);
    }

    #[test]
    fn timeline_ruler_formats_fractional_and_minute_labels() {
        assert_eq!(timeline_ruler_label(1.5, 0.5), "1.5s");
        assert_eq!(timeline_ruler_label(65.0, 5.0), "1:05");
    }

    #[test]
    fn timeline_timecode_formats_fractional_seconds_as_frames() {
        assert_eq!(timeline_timecode(1.5, 60), "00:01:30");
        assert_eq!(timeline_timecode(65.25, 30), "01:05:07");
    }

    #[test]
    fn maps_edited_timeline_time_to_source_time() {
        let bundle: BlipBundle = serde_json::from_str(
            r#"{
                "version": 1,
                "created_at": "2026-07-28T12:00:00-07:00",
                "inputs": [],
                "video_segments": [
                    { "id": 1, "source_start_secs": 0.0, "source_end_secs": 2.0 },
                    { "id": 2, "source_start_secs": 5.0, "source_end_secs": 8.0 }
                ]
            }"#,
        )
        .expect("decode bundle");

        assert_eq!(source_time_at(&bundle, 1.0), Some(1.0));
        assert_eq!(source_time_at(&bundle, 2.0), Some(5.0));
        assert_eq!(source_time_at(&bundle, 4.5), Some(7.5));
    }

    #[test]
    fn video_cut_first_fills_the_gap_then_merges_the_right_segment() {
        let mut segments = vec![
            VideoSegment {
                id: 1,
                source_start_secs: 0.0,
                source_end_secs: 2.0,
            },
            VideoSegment {
                id: 2,
                source_start_secs: 4.5,
                source_end_secs: 7.0,
            },
        ];

        let fill = edit_video_cut(&mut segments, 2).expect("fill cut gap");
        assert_eq!(fill.timeline_start_secs, 2.0);
        assert_eq!(fill.inserted_duration_secs, 2.5);
        assert_eq!(fill.removed_id, None);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].source_end_secs, 4.5);

        let merge = edit_video_cut(&mut segments, 2).expect("merge cut segments");
        assert_eq!(merge.inserted_duration_secs, 0.0);
        assert_eq!(merge.removed_id, Some(2));
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].source_end_secs, 7.0);
    }

    #[test]
    fn video_cuts_require_at_least_one_second_on_each_side() {
        assert!(!can_split_video_segment(1.999, 1.0));
        assert!(can_split_video_segment(2.0, 1.0));
        assert!(!can_split_video_segment(2.0, 0.999));
        assert!(!can_split_video_segment(2.0, 1.001));
        assert!(can_split_video_segment(3.0, 1.0));
        assert!(can_split_video_segment(3.0, 2.0));
        assert!(!can_split_video_segment(3.0, 0.999));
        assert!(!can_split_video_segment(3.0, 2.001));
    }

    #[test]
    fn duration_labels_are_rounded_to_tenths() {
        assert_eq!(duration_label(0.0), "0s");
        assert_eq!(duration_label(1.24), "1.2s");
        assert_eq!(duration_label(1.25), "1.3s");
    }

    #[test]
    fn trimmed_split_edges_increase_the_cut_gap() {
        assert_eq!(video_cut_gap_secs(4.0, 4.0), 0.0);
        assert_eq!(video_cut_gap_secs(3.25, 4.0), 0.75);
        assert_eq!(video_cut_gap_secs(4.0, 4.5), 0.5);
    }

    #[test]
    fn video_segment_edges_can_extend_across_a_cut_gap() {
        assert_eq!(
            resize_video_segment_range(VideoSegmentEdge::End, 0.0, 2.0, 0.0, 4.5, 10.0),
            (0.0, 4.5)
        );
        assert_eq!(
            resize_video_segment_range(VideoSegmentEdge::Start, 4.5, 7.0, 2.0, 7.0, -10.0),
            (2.0, 7.0)
        );
    }

    #[test]
    fn first_video_segment_start_cannot_extend_before_zero() {
        assert_eq!(
            resize_video_segment_range(VideoSegmentEdge::Start, 1.5, 4.0, 0.0, 4.0, -10.0),
            (0.0, 4.0)
        );
    }

    #[test]
    fn zoom_segment_edges_resize_within_neighbor_bounds() {
        assert_eq!(
            resize_zoom_segment_range(ZoomSegmentEdge::Start, 4.0, 7.0, 2.0, 9.0, -10.0),
            (2.0, 7.0)
        );
        assert_eq!(
            resize_zoom_segment_range(ZoomSegmentEdge::End, 4.0, 7.0, 2.0, 9.0, 10.0),
            (4.0, 9.0)
        );
    }

    #[test]
    fn zoom_segment_edges_preserve_a_minimum_duration() {
        assert_eq!(
            resize_zoom_segment_range(ZoomSegmentEdge::Start, 4.0, 7.0, 0.0, 10.0, 10.0),
            (6.99, 7.0)
        );
        assert_eq!(
            resize_zoom_segment_range(ZoomSegmentEdge::End, 4.0, 7.0, 0.0, 10.0, -10.0),
            (4.0, 4.01)
        );
    }

    #[test]
    fn ripple_delete_shifts_and_trims_zoom_segments() {
        let mut segments = vec![
            ZoomSegment {
                id: 1,
                start_secs: 1.0,
                end_secs: 3.0,
                target: [0.5, 0.5],
                amount: 2.0,
                transition: ZoomTransitionSpeed::Medium,
            },
            ZoomSegment {
                id: 2,
                start_secs: 5.0,
                end_secs: 6.0,
                target: [0.5, 0.5],
                amount: 2.0,
                transition: ZoomTransitionSpeed::Medium,
            },
        ];

        ripple_delete_ranges(&mut segments, 2.0, 2.0);

        assert_eq!((segments[0].start_secs, segments[0].end_secs), (1.0, 2.0));
        assert_eq!((segments[1].start_secs, segments[1].end_secs), (3.0, 4.0));
    }

    #[test]
    fn ripple_insert_expands_crossing_zooms_and_shifts_later_zooms() {
        let mut segments = vec![
            ZoomSegment {
                id: 1,
                start_secs: 1.0,
                end_secs: 3.0,
                target: [0.5, 0.5],
                amount: 2.0,
                transition: ZoomTransitionSpeed::Medium,
            },
            ZoomSegment {
                id: 2,
                start_secs: 3.0,
                end_secs: 4.0,
                target: [0.5, 0.5],
                amount: 2.0,
                transition: ZoomTransitionSpeed::Medium,
            },
        ];

        ripple_insert_ranges(&mut segments, 2.0, 1.5);

        assert_eq!((segments[0].start_secs, segments[0].end_secs), (1.0, 4.5));
        assert_eq!((segments[1].start_secs, segments[1].end_secs), (4.5, 5.5));
    }

    #[test]
    fn maps_screen_zoom_target_into_fitted_content() {
        let content = blip_compositor::ItemTransform::new([0.5, 0.5], [0.8, 0.6]);

        let top_left = map_output_transform_to_item(
            blip_compositor::OutputTransform {
                center: [0.0, 0.0],
                scale: 2.0,
            },
            content,
        );
        let bottom_right = map_output_transform_to_item(
            blip_compositor::OutputTransform {
                center: [1.0, 1.0],
                scale: 2.0,
            },
            content,
        );

        assert!((top_left.center[0] - 0.1).abs() < f32::EPSILON);
        assert!((top_left.center[1] - 0.2).abs() < f32::EPSILON);
        assert!((bottom_right.center[0] - 0.9).abs() < f32::EPSILON);
        assert!((bottom_right.center[1] - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn medium_zoom_uses_half_second_transitions() {
        let segment = ZoomSegment {
            id: 1,
            start_secs: 2.0,
            end_secs: 5.0,
            target: [0.25, 0.75],
            amount: 3.0,
            transition: ZoomTransitionSpeed::Medium,
        };
        assert_eq!(zoom_transform_at(&[segment.clone()], 2.0).scale, 1.0);
        assert!(zoom_transform_at(&[segment.clone()], 2.125).scale > 2.0);
        assert_eq!(zoom_transform_at(&[segment.clone()], 2.5).scale, 3.0);
        assert_eq!(zoom_transform_at(&[segment.clone()], 5.0).scale, 3.0);
        assert!(zoom_transform_at(&[segment.clone()], 5.125).scale > 2.5);
        assert_eq!(zoom_transform_at(&[segment], 5.5).scale, 1.0);
    }

    #[test]
    fn zoom_starting_at_zero_is_applied_immediately() {
        let segment = ZoomSegment {
            id: 1,
            start_secs: 0.0,
            end_secs: 3.0,
            target: [0.25, 0.75],
            amount: 3.0,
            transition: ZoomTransitionSpeed::Medium,
        };

        assert_eq!(
            zoom_transform_at(&[segment], 0.0),
            blip_compositor::OutputTransform {
                center: [0.25, 0.75],
                scale: 3.0,
            }
        );
    }

    #[test]
    fn overlapping_zoom_transitions_continue_from_the_outgoing_zoom() {
        let first = ZoomSegment {
            id: 1,
            start_secs: 1.0,
            end_secs: 3.0,
            target: [0.2, 0.3],
            amount: 3.0,
            transition: ZoomTransitionSpeed::Medium,
        };
        let second = ZoomSegment {
            id: 2,
            start_secs: 3.25,
            end_secs: 5.0,
            target: [0.8, 0.7],
            amount: 2.0,
            transition: ZoomTransitionSpeed::Medium,
        };
        let outgoing_at_boundary = zoom_transform_at(&[first.clone()], second.start_secs);
        let combined_at_boundary =
            zoom_transform_at(&[first.clone(), second.clone()], second.start_secs);

        assert_eq!(combined_at_boundary, outgoing_at_boundary);
        assert_eq!(
            zoom_transform_at(&[first, second.clone()], second.start_secs + 0.5),
            zoom_transform_at(&[second.clone()], second.start_secs + 0.5)
        );
        assert_eq!(
            zoom_transform_at(&[second.clone()], second.start_secs).scale,
            1.0
        );
        assert!(combined_at_boundary.scale > 1.0);
    }

    #[test]
    fn cubic_bezier_ease_has_expected_endpoints() {
        assert_eq!(cubic_bezier_ease(0.0, 0.2, 0.8, 0.2, 1.0), 0.0);
        assert_eq!(cubic_bezier_ease(1.0, 0.2, 0.8, 0.2, 1.0), 1.0);
    }

    #[test]
    fn test_decoder_and_compositor() {
        let path = std::path::PathBuf::from("/tmp/test-bundle.blip/inputs/screen.mp4");
        if !path.exists() {
            return;
        }
        let mut decoder = blip_avfoundation::VideoDecoder::open(&path).expect("open decoder");
        println!(
            "decoder: {}x{}, fps={}",
            decoder.width(),
            decoder.height(),
            decoder.nominal_fps()
        );
        let frame = decoder.frame_at(0.0).expect("frame_at 0.0");
        println!(
            "frame: {}x{}, format={:#010x}",
            frame.get_width(),
            frame.get_height(),
            frame.get_pixel_format()
        );
        let mut compositor = blip_compositor::FrameCompositor::new().expect("compositor");
        let sources = [blip_compositor::CompositorSource {
            pixel_buffer: &frame,
            content_rect: None,
        }];
        let items = [blip_compositor::CompositorItem {
            content: blip_compositor::CompositorItemContent::Source(0),
            transform: blip_compositor::ItemTransform::new([0.5, 0.5], [1.0, 1.0]),
        }];
        let res = compositor.render(&sources, &items, (decoder.width(), decoder.height()));
        match &res {
            Ok(out) => println!("composed: {}x{}", out.get_width(), out.get_height()),
            Err(e) => println!("compositor render error: {e:?}"),
        }
        assert!(res.is_ok(), "render failed: {:?}", res.err());
    }
}

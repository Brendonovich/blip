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

use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::CFString,
};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Context, Div, FocusHandle, FontWeight,
    IntoElement, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, ObjectFit,
    PinchEvent, Pixels, Render, ScrollWheelEvent, SharedString, TextAlign, TextRun,
    TitlebarOptions, Window, WindowOptions, canvas, div, fill, img, point, prelude::*, px,
    relative, rgb, rgba, size, surface,
};

use crate::{
    bundle::{BlipBundle, VideoSegment, ZoomSegment, ZoomTransitionSpeed},
    theme,
};

const ACCENT: u32 = 0x00ff_4f58;
const SIDEBAR_WIDTH: Pixels = px(280.0);
const PREVIEW_LOADING_DURATION: Duration = Duration::from_millis(1_400);
const PREVIEW_FADE_DURATION: Duration = Duration::from_millis(240);
const PLAYBACK_TICK_INTERVAL: Duration = Duration::from_millis(16);
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
    image: PathBuf,
    thumbnail: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackgroundType {
    Color,
    Image,
    Gradient,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CornerStyle {
    Circular,
    Squircle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Mp4,
    Mov,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportResolution {
    P1080,
    P720,
}

impl ExportResolution {
    fn dimensions(self) -> (usize, usize) {
        match self {
            Self::P1080 => (1920, 1080),
            Self::P720 => (1280, 720),
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
    dimensions: (usize, usize),
    fps: u32,
    duration_secs: f64,
    bundle: BlipBundle,
    decoder_inputs: Vec<(PathBuf, bool)>,
    wallpaper: Option<PathBuf>,
    background_type: BackgroundType,
    padding: f32,
    border_radius: f32,
    shadow: f32,
    corner_style: CornerStyle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SliderKind {
    Padding,
    Radius,
    Shadow,
    ZoomAmount,
}

#[derive(Clone, Copy)]
struct ZoomSegmentDrag {
    id: u64,
    pointer_start_x: Pixels,
    original_start_secs: f64,
    duration_secs: f64,
}

#[derive(Clone, Copy)]
enum VideoSegmentEdge {
    Start,
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
    minimum_source_start_secs: f64,
    maximum_source_end_secs: f64,
    original_zoom_segments: Vec<ZoomSegment>,
}

#[derive(Clone, Copy)]
struct PreviewRequest {
    time_secs: f64,
    background_type: BackgroundType,
    background_preset: usize,
    padding: f32,
    border_radius: f32,
    shadow: f32,
    corner_style: CornerStyle,
    zoom: blip_compositor::OutputTransform,
}

struct PreviewFrame(core_video::pixel_buffer::CVPixelBuffer);

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
    timeline_view_start_secs: f64,
    timeline_view_duration_secs: f64,
    current_time_secs: f64,
    is_playing: bool,
    playback_started_at: Option<(Instant, f64)>,
    playback_generation: u64,
    cursor_time_secs: Option<f64>,
    zoom_hover_range: Option<(f64, f64)>,
    current_frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    current_screen_frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    timeline_bounds: Rc<Cell<Bounds<Pixels>>>,
    timeline_rows_bounds: Rc<Cell<Bounds<Pixels>>>,
    zoom_target_bounds: Rc<Cell<Bounds<Pixels>>>,
    background_type: BackgroundType,
    background_preset: usize,
    background_images: Vec<BackgroundImage>,
    background_padding: f32,
    border_radius: f32,
    shadow: f32,
    corner_style: CornerStyle,
    padding_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    radius_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    shadow_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    zoom_amount_slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    active_slider: Option<SliderKind>,
    dragging_zoom_target: bool,
    zoom_segment_drag: Option<ZoomSegmentDrag>,
    video_segment_drag: Option<VideoSegmentDrag>,
    export_dialog_open: bool,
    export_format: ExportFormat,
    export_resolution: ExportResolution,
    export_fps: u32,
    export_destination: Option<PathBuf>,
    export_progress: f32,
    exporting: bool,
    export_error: Option<String>,
    exported_path: Option<PathBuf>,
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
        let mut max_duration = 0.0;
        for (index, input) in bundle.inputs.iter().enumerate() {
            let media_path = path.join(&input.media);
            match blip_avfoundation::VideoDecoder::open(&media_path) {
                Ok(decoder) => {
                    let dur = decoder.duration().as_secs_f64();
                    if dur > max_duration {
                        max_duration = dur;
                    }
                    decoder_inputs.push((media_path, input_is_camera(input, index)));
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
        if bundle.video_segments.is_none() {
            bundle.video_segments = Some(if max_duration > 0.0 {
                vec![VideoSegment {
                    id: 1,
                    source_start_secs: 0.0,
                    source_end_secs: max_duration,
                }]
            } else {
                Vec::new()
            });
        }
        let timeline_duration = video_timeline_duration(&bundle);
        let background_images = bundled_backgrounds();
        let preview_backgrounds = background_images
            .iter()
            .map(|background| background.image.clone())
            .collect();
        let screen_input = decoder_inputs
            .iter()
            .find(|(_, is_camera)| !*is_camera)
            .map(|(path, _)| path.clone());
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
                        while let Ok(frame) = preview_result_receiver.recv().await {
                            if editor_entity
                                .update(cx, |editor, cx| {
                                    editor.current_frame = Some(frame.0);
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
                        timeline_view_start_secs: 0.0,
                        timeline_view_duration_secs: timeline_duration,
                        current_time_secs: 0.0,
                        is_playing: false,
                        playback_started_at: None,
                        playback_generation: 0,
                        cursor_time_secs: None,
                        zoom_hover_range: None,
                        current_frame: None,
                        current_screen_frame: None,
                        timeline_bounds: Rc::new(Cell::new(Bounds::default())),
                        timeline_rows_bounds: Rc::new(Cell::new(Bounds::default())),
                        zoom_target_bounds: Rc::new(Cell::new(Bounds::default())),
                        background_type: BackgroundType::Gradient,
                        background_preset: 0,
                        background_images,
                        background_padding: 8.0,
                        border_radius: 8.0,
                        shadow: 20.0,
                        corner_style: CornerStyle::Squircle,
                        padding_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        radius_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        shadow_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        zoom_amount_slider_bounds: Rc::new(Cell::new(Bounds::default())),
                        active_slider: None,
                        dragging_zoom_target: false,
                        zoom_segment_drag: None,
                        video_segment_drag: None,
                        export_dialog_open: false,
                        export_format: ExportFormat::Mp4,
                        export_resolution: ExportResolution::P1080,
                        export_fps: 30,
                        export_destination: None,
                        export_progress: 0.0,
                        exporting: false,
                        export_error: None,
                        exported_path: None,
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

    fn select_input(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.bundle.inputs.get(index).is_some() {
            self.selected_input = index;
            self.selected_zoom = None;
            self.selected_video_segment = None;
            cx.notify();
        }
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
            .export_destination
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
        let suggested_name = format!("{stem}.{}", self.export_format.extension());
        let receiver = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn(async move |editor, cx| match receiver.await {
            Ok(Ok(Some(path))) => {
                let _ = editor.update(cx, |editor, cx| {
                    editor.export_destination = Some(path);
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
        let Some(destination) = self.export_destination.clone() else {
            self.export_error = Some("Choose a destination before exporting.".into());
            cx.notify();
            return;
        };
        if self.duration_secs <= 0.0 {
            self.export_error = Some("There is no video on the timeline to export.".into());
            cx.notify();
            return;
        }
        let output = destination.with_extension(self.export_format.extension());
        let decoder_inputs = self
            .bundle
            .inputs
            .iter()
            .enumerate()
            .map(|(index, input)| (self.path.join(&input.media), input_is_camera(input, index)))
            .collect();
        let job = ExportJob {
            output: output.clone(),
            file_type: self.export_format.file_type(),
            dimensions: self.export_resolution.dimensions(),
            fps: self.export_fps,
            duration_secs: self.duration_secs,
            bundle: self.bundle.clone(),
            decoder_inputs,
            wallpaper: (self.background_type == BackgroundType::Image)
                .then(|| self.background_images.get(self.background_preset))
                .flatten()
                .map(|background| background.image.clone()),
            background_type: self.background_type,
            padding: self.background_padding,
            border_radius: self.border_radius,
            shadow: self.shadow,
            corner_style: self.corner_style,
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
        self.export_destination = Some(output);
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
        if offset <= 0.01 || offset >= duration - 0.01 {
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
            minimum_source_start_secs: index
                .checked_sub(1)
                .and_then(|index| segments.get(index))
                .map(|segment| segment.source_end_secs)
                .unwrap_or(0.0),
            maximum_source_end_secs: segments
                .get(index + 1)
                .map(|segment| segment.source_start_secs)
                .unwrap_or(segment.source_end_secs),
            original_zoom_segments: self.bundle.zoom_segments.clone(),
        });
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
        let original_duration = drag.original_source_end_secs - drag.original_source_start_secs;
        let (source_start_secs, source_end_secs) = resize_video_segment_range(
            drag.edge,
            drag.original_source_start_secs,
            drag.original_source_end_secs,
            drag.minimum_source_start_secs,
            drag.maximum_source_end_secs,
            delta_secs,
        );
        let new_duration = source_end_secs - source_start_secs;
        let Some(segment) = self
            .bundle
            .video_segments
            .as_mut()
            .and_then(|segments| segments.iter_mut().find(|segment| segment.id == drag.id))
        else {
            self.video_segment_drag = None;
            return true;
        };
        if segment.source_start_secs == source_start_secs
            && segment.source_end_secs == source_end_secs
        {
            return true;
        }
        segment.source_start_secs = source_start_secs;
        segment.source_end_secs = source_end_secs;

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
        self.timeline_view_duration_secs = drag
            .timeline_view_duration_secs
            .min(self.duration_secs)
            .max(0.0);
        self.timeline_view_start_secs = self
            .timeline_view_start_secs
            .min((self.duration_secs - self.timeline_view_duration_secs).max(0.0));
        self.current_time_secs = self.current_time_secs.min(self.duration_secs);
        self.request_preview();
        self.request_zoom_target();
        cx.notify();
        true
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
        self.timeline_view_duration_secs = self
            .timeline_view_duration_secs
            .min(self.duration_secs)
            .max(0.0);
        self.timeline_view_start_secs = self
            .timeline_view_start_secs
            .min((self.duration_secs - self.timeline_view_duration_secs).max(0.0));
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
        let Some(edit) = self
            .bundle
            .video_segments
            .as_mut()
            .and_then(|segments| edit_video_cut(segments, right_id))
        else {
            return;
        };

        if edit.inserted_duration_secs > 0.0 {
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
        if (self.timeline_view_start_secs <= f64::EPSILON
            && self.timeline_view_duration_secs >= old_duration - f64::EPSILON)
            || self.timeline_view_duration_secs > self.duration_secs
        {
            self.timeline_view_start_secs = 0.0;
            self.timeline_view_duration_secs = self.duration_secs;
        } else {
            self.timeline_view_start_secs = self
                .timeline_view_start_secs
                .min((self.duration_secs - self.timeline_view_duration_secs).max(0.0));
        }
        self.cursor_time_secs = None;
        self.save_bundle();
        self.request_preview();
        self.request_zoom_target();
        cx.notify();
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

    fn begin_zoom_segment_drag(&mut self, id: u64, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(segment) = self
            .bundle
            .zoom_segments
            .iter()
            .find(|segment| segment.id == id)
        else {
            return;
        };
        let start_secs = segment.start_secs;
        self.zoom_segment_drag = Some(ZoomSegmentDrag {
            id,
            pointer_start_x: event.position.x,
            original_start_secs: start_secs,
            duration_secs: (segment.end_secs - start_secs).max(0.0),
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
        let max_start_secs = (self.duration_secs - drag.duration_secs).max(0.0);
        let start_secs = (drag.original_start_secs
            + f64::from(delta_fraction) * self.timeline_view_duration_secs)
            .clamp(0.0, max_start_secs);
        let playback_time_secs = self.timeline_time_at(event.position.x);
        let Some(segment) = self
            .bundle
            .zoom_segments
            .iter_mut()
            .find(|segment| segment.id == drag.id)
        else {
            self.zoom_segment_drag = None;
            return true;
        };
        let segment_moved = segment.start_secs != start_secs;
        if segment_moved {
            segment.start_secs = start_secs;
            segment.end_secs = start_secs + drag.duration_secs;
        }
        if let Some(time_secs) = playback_time_secs {
            self.set_playback_time(time_secs);
        }
        if segment_moved {
            self.request_zoom_target();
        }
        cx.notify();
        true
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

    fn set_playback_time(&mut self, time_secs: f64) {
        self.stop_playback();
        self.current_time_secs = time_secs.clamp(0.0, self.duration_secs);
        self.request_preview();
    }

    fn stop_playback(&mut self) {
        self.is_playing = false;
        self.playback_started_at = None;
        self.playback_generation = self.playback_generation.wrapping_add(1);
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
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
        let generation = self.playback_generation;
        self.playback_started_at = Some((Instant::now(), self.current_time_secs));
        self.request_preview();
        cx.notify();

        cx.spawn(async move |editor, cx| {
            loop {
                cx.background_executor().timer(PLAYBACK_TICK_INTERVAL).await;
                let keep_playing = editor
                    .update(cx, |editor, cx| {
                        if !editor.is_playing || editor.playback_generation != generation {
                            return false;
                        }
                        let Some((started_at, start_time_secs)) = editor.playback_started_at else {
                            return false;
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
                        }
                        cx.notify();
                        !finished
                    })
                    .unwrap_or(false);
                if !keep_playing {
                    break;
                }
            }
        })
        .detach();
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
            self.duration_secs,
            focal_fraction,
            event.delta,
        );
        if start_secs != self.timeline_view_start_secs
            || duration_secs != self.timeline_view_duration_secs
        {
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
            self.duration_secs,
            f64::from(delta.x / bounds.size.width),
        );
        if start_secs != self.timeline_view_start_secs {
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
            time_secs: source_time_secs,
            background_type: self.background_type,
            background_preset: self.background_preset,
            padding: self.background_padding,
            border_radius: self.border_radius,
            shadow: self.shadow,
            corner_style: self.corner_style,
            zoom: zoom_transform_at(&self.bundle.zoom_segments, time_secs),
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
            SliderKind::ZoomAmount => self.zoom_amount_slider_bounds.get(),
        };
        if bounds.size.width <= Pixels::ZERO {
            return;
        }
        let fraction = ((position - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0);
        match kind {
            SliderKind::Padding => self.background_padding = (fraction * 50.0).round(),
            SliderKind::Radius => self.border_radius = (fraction * 50.0).round(),
            SliderKind::Shadow => self.shadow = (fraction * 50.0).round(),
            SliderKind::ZoomAmount => {
                self.set_zoom_amount(1.0 + (fraction * 40.0).round() / 10.0, cx);
                return;
            }
        }
        self.request_preview();
        cx.notify();
    }

    fn drag_slider(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
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
        let selected = self.background_type == value;
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
                editor.background_type = value;
                editor.request_preview();
                cx.notify();
            }))
            .child(label)
    }

    fn background_preset(&self, index: usize, path: PathBuf, cx: &mut Context<Self>) -> AnyElement {
        let selected =
            self.background_type == BackgroundType::Image && self.background_preset == index;
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
                editor.background_type = BackgroundType::Image;
                editor.background_preset = index;
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

    fn corner_option(
        &self,
        label: &'static str,
        value: CornerStyle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.corner_style == value;
        let sample = div().w(px(24.0)).h(px(16.0)).bg(rgb(if selected {
            theme::TEXT
        } else {
            theme::TEXT_MUTED
        }));
        let sample = match value {
            CornerStyle::Circular => sample.rounded_full(),
            CornerStyle::Squircle => sample.rounded(px(5.0)),
        };
        div()
            .id(format!("corner-style-{label}"))
            .flex_1()
            .h(px(52.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
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
                editor.corner_style = value;
                editor.request_preview();
                cx.notify();
            }))
            .child(sample)
            .child(label)
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

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        if self.selected_zoom().is_some() {
            return self.zoom_sidebar(cx);
        }
        if self.selected_video_segment().is_some() {
            return self.video_segment_sidebar(cx);
        }
        div()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .p_4()
            .flex()
            .flex_col()
            .gap_8()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_l_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .child(
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
                                    .child(self.background_option(
                                        "Color",
                                        BackgroundType::Color,
                                        cx,
                                    ))
                                    .child(self.background_option(
                                        "Image",
                                        BackgroundType::Image,
                                        cx,
                                    ))
                                    .child(self.background_option(
                                        "Gradient",
                                        BackgroundType::Gradient,
                                        cx,
                                    )),
                            )
                            .when(self.background_type == BackgroundType::Image, |section| {
                                section.child(self.background_presets(cx))
                            }),
                    )
                    .child(self.percentage_slider(
                        "Padding",
                        self.background_padding,
                        SliderKind::Padding,
                        Rc::clone(&self.padding_slider_bounds),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Screen"),
                    )
                    .child(self.percentage_slider(
                        "Border radius",
                        self.border_radius,
                        SliderKind::Radius,
                        Rc::clone(&self.radius_slider_bounds),
                        cx,
                    ))
                    .child(self.percentage_slider(
                        "Shadow",
                        self.shadow,
                        SliderKind::Shadow,
                        Rc::clone(&self.shadow_slider_bounds),
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
                                    .child("Corner style"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(self.corner_option(
                                        "Circular",
                                        CornerStyle::Circular,
                                        cx,
                                    ))
                                    .child(self.corner_option(
                                        "Squircle",
                                        CornerStyle::Squircle,
                                        cx,
                                    )),
                            ),
                    ),
            )
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

    fn export_dialog(&self, cx: &mut Context<Self>) -> Div {
        let destination = self
            .export_destination
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Choose a file...")
            .to_owned();
        let can_export = self.export_destination.is_some() && !self.exporting;
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
                                self.export_format == ExportFormat::Mp4,
                                cx,
                                |editor, cx| {
                                    editor.export_format = ExportFormat::Mp4;
                                    editor.exported_path = None;
                                    cx.notify();
                                },
                            ))
                            .child(self.export_option(
                                "export-format-mov",
                                "MOV (H.264)",
                                self.export_format == ExportFormat::Mov,
                                cx,
                                |editor, cx| {
                                    editor.export_format = ExportFormat::Mov;
                                    editor.exported_path = None;
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
                                self.export_resolution == ExportResolution::P1080,
                                cx,
                                |editor, cx| {
                                    editor.export_resolution = ExportResolution::P1080;
                                    editor.exported_path = None;
                                    cx.notify();
                                },
                            ))
                            .child(self.export_option(
                                "export-resolution-720",
                                "720p",
                                self.export_resolution == ExportResolution::P720,
                                cx,
                                |editor, cx| {
                                    editor.export_resolution = ExportResolution::P720;
                                    editor.exported_path = None;
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
                                self.export_fps == 30,
                                cx,
                                |editor, cx| {
                                    editor.export_fps = 30;
                                    editor.exported_path = None;
                                    cx.notify();
                                },
                            ))
                            .child(self.export_option(
                                "export-fps-60",
                                "60 fps",
                                self.export_fps == 60,
                                cx,
                                |editor, cx| {
                                    editor.export_fps = 60;
                                    editor.exported_path = None;
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
        if let Some(frame) = &self.current_frame {
            div()
                .w_full()
                .max_w(px(1280.0))
                .max_h(px(720.0))
                .aspect_ratio(1.777_777_8)
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
                .aspect_ratio(1.777_777_8)
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
        let playback_fraction =
            timeline_time_fraction(self.current_time_secs, view_start_secs, view_duration_secs);
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
        for (index, input) in self.bundle.inputs.iter().enumerate() {
            let bounds_cell = Rc::clone(&bounds_cell);
            let mut video_track = div().h(px(48.0)).flex_1().relative();
            let mut timeline_start = 0.0;
            let mut cuts = Vec::new();
            let mut previous_source_end = None;
            let mut video_segment_layer = div().absolute().size_full().overflow_hidden();
            let video_segments = self.bundle.video_segments.as_deref().unwrap_or_default();
            for (segment_index, segment) in video_segments.iter().enumerate() {
                let id = segment.id;
                let segment_duration = segment.duration_secs();
                if let Some(source_end_secs) = previous_source_end
                    && let Some(fraction) =
                        timeline_time_fraction(timeline_start, view_start_secs, view_duration_secs)
                {
                    cuts.push((
                        id,
                        fraction,
                        video_cut_gap_secs(source_end_secs, segment.source_start_secs),
                    ));
                }
                let visible_range = timeline_segment_range_fraction(
                    timeline_start,
                    timeline_start + segment_duration,
                    view_start_secs,
                    view_duration_secs,
                );
                let segment_selected = self.selected_video_segment == Some(id);
                let media = media_name(input.media.as_path());
                if let Some((start, width)) = visible_range {
                    let label_start = (-start).max(0.0) / width;
                    video_segment_layer = video_segment_layer.child(
                        div()
                            .id(format!("video-segment-{index}-{id}"))
                            .absolute()
                            .left(relative(start))
                            .w(relative(width))
                            .top_0()
                            .bottom_0()
                            .min_w(px(2.0))
                            .when(self.cut_mode, |segment| segment.cursor_crosshair())
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
                                            .left(relative(label_start))
                                            .top_0()
                                            .bottom_0()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .child(media),
                                    ),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                                    editor.selected_input = index;
                                    if let Some(time_secs) =
                                        editor.timeline_time_at(event.position.x)
                                    {
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
                                    .w(px(8.0))
                                    .cursor_e_resize()
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
                                    .w(px(8.0))
                                    .cursor_w_resize()
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
                            ),
                    );
                }
                timeline_start += segment_duration;
                previous_source_end = Some(segment.source_end_secs);
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
            for (right_id, fraction, gap_secs) in cuts {
                video_track = video_track.child(
                    div()
                        .absolute()
                        .left(relative(fraction))
                        .top(px(-18.0))
                        .bottom_0()
                        .w(px(1.0))
                        .child(
                            div()
                                .absolute()
                                .top(px(16.0))
                                .bottom_0()
                                .w(px(1.0))
                                .bg(rgb(ACCENT)),
                        )
                        .child(
                            div()
                                .id(format!("video-cut-{index}-{right_id}"))
                                .absolute()
                                .top_0()
                                .left(px(-16.0))
                                .min_w(px(33.0))
                                .h(px(16.0))
                                .px_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(rgb(ACCENT))
                                .hover(|bubble| bubble.bg(rgb(theme::SELECTION)))
                                .cursor_pointer()
                                .text_xs()
                                .text_color(rgb(theme::APP_BACKGROUND))
                                .on_click(cx.listener(move |editor, _, _, cx| {
                                    editor.activate_video_cut(right_id, cx);
                                    cx.stop_propagation();
                                }))
                                .child(cut_gap_label(gap_secs)),
                        ),
                );
            }
            tracks = tracks.child(
                div()
                    .h(px(48.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(("track-label", index))
                            .w(px(80.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .on_click(cx.listener(move |editor, _, _, cx| {
                                editor.select_input(index, cx);
                            }))
                            .child(input.name.clone()),
                    )
                    .child(video_track),
            );
        }
        let zoom_track_bounds = Rc::clone(&bounds_cell);
        let mut zoom_segments = div().h(px(48.0)).flex_1().relative().overflow_hidden();
        for segment in &self.bundle.zoom_segments {
            let id = segment.id;
            let visible_range = timeline_segment_range_fraction(
                segment.start_secs,
                segment.end_secs,
                view_start_secs,
                view_duration_secs,
            );
            let selected = self.selected_zoom == Some(id);
            if let Some((start, width)) = visible_range {
                let label_start = (-start).max(0.0) / width;
                zoom_segments = zoom_segments.child(
                    div()
                        .id(("zoom-segment", id))
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
                                        .left(relative(label_start))
                                        .top_0()
                                        .bottom_0()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .text_xs()
                                        .child(format!("{}x", segment.amount as u32)),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                                editor.begin_zoom_segment_drag(id, event, cx);
                                if let Some(time_secs) = editor.timeline_time_at(event.position.x) {
                                    editor.set_playback_time(time_secs);
                                }
                                cx.stop_propagation();
                            }),
                        ),
                );
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
            let label_start = (-start).max(0.0) / width;
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
                            .border_color(rgb(theme::SELECTION))
                            .child(
                                div()
                                    .absolute()
                                    .left(relative(label_start))
                                    .top_0()
                                    .bottom_0()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .text_xs()
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
                    editor.begin_zoom_segment_drag(id, event, cx);
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
        tracks = tracks.child(
            div()
                .h(px(48.0))
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(80.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child("Zoom"),
                )
                .child(zoom_segments),
        );
        tracks = tracks.when_some(playback_fraction, |tracks, playback_fraction| {
            tracks.child(
                div()
                    .absolute()
                    .left(px(92.0))
                    .right_0()
                    .top(px(-4.0))
                    .bottom(px(-8.0))
                    .child(
                        div()
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
                            ),
                    ),
            )
        });
        tracks = tracks.when_some(preview_fraction, |tracks, fraction| {
            tracks.child(
                div()
                    .absolute()
                    .left(px(92.0))
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
                            .bg(rgb(theme::TEXT))
                            .opacity(0.5)
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-5.0))
                                    .left(px(-4.0))
                                    .size(px(10.0))
                                    .rounded_full()
                                    .bg(rgb(theme::TEXT)),
                            ),
                    ),
            )
        });

        let time_label = format!(
            "{:02}:{:02}.{:02} / {:02}:{:02}.{:02}",
            (self.current_time_secs as u64) / 60,
            (self.current_time_secs as u64) % 60,
            ((self.current_time_secs % 1.0) * 100.0) as u64,
            (self.duration_secs as u64) / 60,
            (self.duration_secs as u64) % 60,
            ((self.duration_secs % 1.0) * 100.0) as u64,
        );

        div()
            .p_4()
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
                    let bounds = editor.timeline_bounds.get();
                    if event.position.x >= bounds.origin.x - px(6.0) {
                        let Some(time_secs) = editor.timeline_time_at(event.position.x) else {
                            return;
                        };
                        editor.set_playback_time(time_secs);
                        cx.notify();
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
                            .w(px(80.0))
                            .text_xs()
                            .text_color(rgb(theme::TEXT_DIM))
                            .child("TIMELINE"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .child(
                                div()
                                    .id("toggle-playback")
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(rgb(theme::CONTROL_BACKGROUND))
                                    .border_1()
                                    .border_color(rgb(theme::BORDER_SUBTLE))
                                    .cursor_pointer()
                                    .text_color(rgb(theme::TEXT))
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.toggle_playback(cx);
                                    }))
                                    .child(if self.is_playing {
                                        "Pause (Space)"
                                    } else {
                                        "Play (Space)"
                                    }),
                            )
                            .child(
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
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(time_label),
                    ),
            )
            .child(timeline_ruler(view_start_secs, view_duration_secs))
            .child(tracks)
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
            .on_action(cx.listener(|editor, _: &crate::TogglePlayback, _, cx| {
                editor.toggle_playback(cx);
            }))
            .on_action(cx.listener(|editor, _: &crate::DeleteSelected, _, cx| {
                editor.delete_selected(cx);
            }))
            .on_action(cx.listener(|editor, _: &crate::CloseExportDialog, _, cx| {
                if editor.export_dialog_open
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
                cx.listener(|editor, _, _, _| {
                    if editor.active_slider == Some(SliderKind::ZoomAmount) {
                        editor.save_bundle();
                    }
                    editor.active_slider = None;
                    if editor.zoom_segment_drag.take().is_some() {
                        editor.save_bundle();
                    }
                    if editor.video_segment_drag.take().is_some() {
                        editor.save_bundle();
                    }
                    if editor.dragging_zoom_target {
                        editor.save_bundle();
                    }
                    editor.dragging_zoom_target = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|editor, _, _, _| {
                    if editor.active_slider == Some(SliderKind::ZoomAmount) {
                        editor.save_bundle();
                    }
                    editor.active_slider = None;
                    if editor.zoom_segment_drag.take().is_some() {
                        editor.save_bundle();
                    }
                    if editor.video_segment_drag.take().is_some() {
                        editor.save_bundle();
                    }
                    if editor.dragging_zoom_target {
                        editor.save_bundle();
                    }
                    editor.dragging_zoom_target = false;
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
                            .p_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(theme::CANVAS_BACKGROUND))
                            .child(self.preview(selected)),
                    )
                    .child(self.sidebar(cx)),
            )
            .child(self.timeline(cx))
            .when(self.export_dialog_open, |editor| {
                editor.child(self.export_dialog(cx))
            })
    }
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

fn cut_gap_label(gap_secs: f64) -> String {
    let rounded = (gap_secs.max(0.0) * 10.0).round() / 10.0;
    if rounded == 0.0 {
        "0s".to_owned()
    } else {
        format!("{rounded:.1}s")
    }
}

fn video_cut_gap_secs(left_source_end_secs: f64, right_source_start_secs: f64) -> f64 {
    right_source_start_secs - left_source_end_secs
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
    decoder_inputs: Vec<(PathBuf, bool)>,
    background_paths: Vec<PathBuf>,
    requests: async_channel::Receiver<PreviewRequest>,
    results: async_channel::Sender<PreviewFrame>,
) {
    let mut decoders = decoder_inputs
        .into_iter()
        .filter_map(
            |(path, is_camera)| match blip_avfoundation::VideoDecoder::open(&path) {
                Ok(decoder) => Some((decoder, is_camera)),
                Err(error) => {
                    eprintln!(
                        "blip-capture: failed to open video decoder for {}: {error}",
                        path.display()
                    );
                    None
                }
            },
        )
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
            .is_none_or(|(time_secs, _, _, _)| *time_secs != request.time_secs)
        {
            match render_content_frame(&mut decoders, &mut compositor, request.time_secs) {
                Ok((content_frame, canvas_width, canvas_height)) => {
                    content_frame_cache = Some((
                        request.time_secs,
                        content_frame,
                        canvas_width,
                        canvas_height,
                    ));
                }
                Err(error) => {
                    eprintln!("blip-capture: {error}");
                    continue;
                }
            }
        }
        let Some((_, content_frame, canvas_width, canvas_height)) = &content_frame_cache else {
            continue;
        };

        let output_width = (*canvas_width).max(2) & !1;
        let output_height = (((output_width as f64 * 9.0 / 16.0).round() as usize).max(2) + 1) & !1;
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
            content_frame,
            (*canvas_width, *canvas_height),
            (output_width, output_height),
            wallpaper,
            request.background_type,
            request.padding,
            request.border_radius,
            request.shadow,
            request.corner_style,
            request.zoom,
        ) {
            Ok(frame) => {
                if results.send_blocking(PreviewFrame(frame)).is_err() {
                    break;
                }
            }
            Err(error) => eprintln!("blip-capture: error composing preview frame: {error}"),
        }
    }
}

fn render_content_frame(
    decoders: &mut [(blip_avfoundation::VideoDecoder, bool)],
    compositor: &mut blip_compositor::FrameCompositor,
    time_secs: f64,
) -> Result<(core_video::pixel_buffer::CVPixelBuffer, usize, usize), String> {
    let mut active_frames = Vec::new();
    for (decoder, is_camera) in decoders {
        if let Ok(pixel_buffer) = decoder.frame_at(time_secs) {
            active_frames.push((pixel_buffer, decoder.width(), decoder.height(), *is_camera));
        }
    }
    if active_frames.is_empty() {
        return Err(format!("no frames decoded at {time_secs}s"));
    }
    active_frames.sort_by_key(|(_, _, _, is_camera)| if *is_camera { 1 } else { 0 });
    let (canvas_width, canvas_height) = active_frames
        .iter()
        .find(|(_, _, _, is_camera)| !*is_camera)
        .or_else(|| active_frames.first())
        .map(|(_, width, height, _)| (*width, *height))
        .unwrap_or((1920, 1080));
    let mut sources = Vec::with_capacity(active_frames.len());
    let mut items = Vec::with_capacity(active_frames.len());
    for (index, (pixel_buffer, width, height, is_camera)) in active_frames.iter().enumerate() {
        sources.push(blip_compositor::CompositorSource {
            pixel_buffer,
            content_rect: None,
        });
        let transform = if *is_camera {
            camera_transform(
                (*width as f64, *height as f64),
                (canvas_width as f64, canvas_height as f64),
            )
        } else {
            blip_compositor::ItemTransform::new([0.5, 0.5], [1.0, 1.0])
        };
        items.push(blip_compositor::CompositorItem {
            content: blip_compositor::CompositorItemContent::Source(index),
            transform,
        });
    }
    compositor
        .render(&sources, &items, (canvas_width, canvas_height))
        .map(|frame| (frame, canvas_width, canvas_height))
        .map_err(|error| format!("error composing content frame: {error}"))
}

fn render_output_frame(
    compositor: &mut blip_compositor::FrameCompositor,
    content_frame: &core_video::pixel_buffer::CVPixelBuffer,
    canvas_dimensions: (usize, usize),
    output_dimensions: (usize, usize),
    wallpaper: Option<&core_video::pixel_buffer::CVPixelBuffer>,
    background_type: BackgroundType,
    padding: f32,
    border_radius: f32,
    shadow: f32,
    corner_style: CornerStyle,
    zoom: blip_compositor::OutputTransform,
) -> Result<core_video::pixel_buffer::CVPixelBuffer, String> {
    let (canvas_width, canvas_height) = canvas_dimensions;
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
        content_rect: None,
    });
    let content_transform = blip_compositor::ItemTransform::new([0.5, 0.5], content_size)
        .with_corner_radius(corner_radius)
        .with_squircle(corner_style == CornerStyle::Squircle)
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
    compositor
        .render_with_output_transform(
            &output_sources,
            &output_items,
            output_dimensions,
            map_output_transform_to_item(zoom, content_transform),
        )
        .map_err(|error| format!("error composing output frame: {error}"))
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
        .map(|(path, is_camera)| {
            blip_avfoundation::VideoDecoder::open(&path)
                .map(|decoder| (decoder, is_camera))
                .map_err(|error| format!("Could not open {}: {error}", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let wallpaper = job.wallpaper.as_deref().map(load_wallpaper).transpose()?;
    let mut content_compositor = blip_compositor::FrameCompositor::new()
        .map_err(|error| format!("Could not create content renderer: {error}"))?;
    let mut output_compositor = blip_compositor::FrameCompositor::new()
        .map_err(|error| format!("Could not create output renderer: {error}"))?;
    let mut writer = blip_avfoundation::Mp4Writer::new_with_file_type(
        &job.output,
        job.dimensions.0,
        job.dimensions.1,
        job.fps,
        job.file_type,
    )
    .map_err(|error| format!("Could not create export: {error}"))?;
    let frame_count = (job.duration_secs * f64::from(job.fps)).ceil().max(1.0) as usize;

    for frame_index in 0..frame_count {
        let timeline_time_secs = frame_index as f64 / f64::from(job.fps);
        let source_time_secs = source_time_at(&job.bundle, timeline_time_secs)
            .ok_or_else(|| "Could not map the export timeline to source video.".to_owned())?;
        let (content_frame, width, height) =
            render_content_frame(&mut decoders, &mut content_compositor, source_time_secs)?;
        let output_frame = render_output_frame(
            &mut output_compositor,
            &content_frame,
            (width, height),
            job.dimensions,
            wallpaper.as_ref(),
            job.background_type,
            job.padding,
            job.border_radius,
            job.shadow,
            job.corner_style,
            zoom_transform_at(&job.bundle.zoom_segments, timeline_time_secs),
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
    let zoom_in = ((time_secs - segment.start_secs) / transition_secs).clamp(0.0, 1.0);
    let zoom_out =
        ((segment.end_secs + transition_secs - time_secs) / transition_secs).clamp(0.0, 1.0);
    let attack = cubic_bezier_ease(zoom_in, 0.2, 0.8, 0.2, 1.0);
    let release = cubic_bezier_ease(zoom_out, 0.4, 0.0, 0.6, 1.0);
    let progress = attack.min(release) as f32;
    blip_compositor::OutputTransform {
        center: segment.target,
        scale: 1.0 + (segment.amount.clamp(1.0, 5.0) - 1.0) * progress,
    }
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
                Some(BackgroundImage { image, thumbnail })
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

fn media_name(path: &std::path::Path) -> SharedString {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Recording")
        .to_owned()
        .into()
}

fn playback_position(start_secs: f64, elapsed_secs: f64, duration_secs: f64) -> (f64, bool) {
    let time_secs = (start_secs + elapsed_secs).clamp(0.0, duration_secs);
    (time_secs, time_secs >= duration_secs)
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
    let start_secs = (focal_time_secs - focal_fraction * duration_secs)
        .clamp(0.0, timeline_duration_secs - duration_secs);
    (start_secs, duration_secs)
}

fn panned_timeline_view(
    view_start_secs: f64,
    view_duration_secs: f64,
    timeline_duration_secs: f64,
    delta_fraction: f64,
) -> f64 {
    (view_start_secs - delta_fraction * view_duration_secs)
        .clamp(0.0, (timeline_duration_secs - view_duration_secs).max(0.0))
}

fn timeline_ruler(view_start_secs: f64, view_duration_secs: f64) -> Div {
    div()
        .h(px(24.0))
        .flex()
        .items_end()
        .gap_3()
        .child(div().w(px(80.0)))
        .child(
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
                            let text: SharedString =
                                timeline_ruler_label(time_secs, interval).into();
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
    input.id.eq_ignore_ascii_case("camera")
        || input.name.to_lowercase().contains("camera")
        || (index > 0 && !input.id.eq_ignore_ascii_case("screen"))
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

fn camera_transform(
    source_dimensions: (f64, f64),
    canvas_dimensions: (f64, f64),
) -> blip_compositor::ItemTransform {
    let size = aspect_fit_size([0.28, 0.28], source_dimensions, canvas_dimensions);
    let margin_x = 0.03_f32;
    let margin_y = 0.03_f32;
    let center_x = 1.0 - margin_x - (size[0] * 0.5);
    let center_y = 1.0 - margin_y - (size[1] * 0.5);
    let corner_radius = (f64::from(size[0]) * canvas_dimensions.0)
        .min(f64::from(size[1]) * canvas_dimensions.1)
        * 0.08;
    blip_compositor::ItemTransform::new([center_x, center_y], size)
        .with_corner_radius(corner_radius as f32)
}

#[cfg(test)]
mod tests {
    use crate::bundle::{BlipBundle, VideoSegment, ZoomSegment, ZoomTransitionSpeed};

    use super::{
        VideoSegmentEdge, cubic_bezier_ease, cut_gap_label, edit_video_cut,
        map_output_transform_to_item, panned_timeline_view, playback_position,
        resize_video_segment_range, ripple_delete_ranges, ripple_insert_ranges, source_time_at,
        timeline_range_fraction, timeline_ruler_interval, timeline_ruler_label,
        timeline_segment_range_fraction, timeline_time_fraction, video_cut_gap_secs,
        zoom_segment_range_at, zoom_transform_at, zoomed_timeline_view,
    };

    #[test]
    fn playback_position_uses_elapsed_wall_clock_time() {
        assert_eq!(playback_position(2.0, 1.5, 10.0), (3.5, false));
    }

    #[test]
    fn playback_position_stops_at_the_timeline_end() {
        assert_eq!(playback_position(9.0, 2.0, 10.0), (10.0, true));
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
    fn cut_gap_labels_are_rounded_to_tenths() {
        assert_eq!(cut_gap_label(0.0), "0s");
        assert_eq!(cut_gap_label(1.24), "1.2s");
        assert_eq!(cut_gap_label(1.25), "1.3s");
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

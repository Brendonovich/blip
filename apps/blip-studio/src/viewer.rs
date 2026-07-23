use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::StreamArgs;
use crate::assets::{CHEVRON_DOWN, GRIP_VERTICAL, StudioAssets};
use crate::compositor::{
    CompositorItem, CompositorItemContent, CompositorSource, FrameCompositor, ItemTransform,
};
use crate::numeric_input::{NumericInput, NumericInputEvent};
use crate::rtmp::{RtmpConfig, RtmpStream};
use crate::theme;
use anyhow::Context as _;
use async_channel::Sender;
use blip_avfoundation::{CameraCapturer, CameraDevice, CameraFrame};
use blip_sck::{
    CaptureError, CaptureFilter, Capturer, Display as CaptureDisplay, PixelFormat,
    ShareableContent, StreamConfig, StreamConfigBuilder, VideoFrame, Window as CaptureWindow,
};
use core_foundation::base::TCFType;
use core_video::pixel_buffer::CVPixelBuffer;
#[cfg(target_os = "macos")]
use gpui::KeyBinding;
use gpui::{
    Animation, AnimationExt as _, App, BorderStyle, Bounds, Context, DevicePixels, Div,
    DragMoveEvent, Entity, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Pixels, Point, Render, TitlebarOptions, Transformation, Window,
    WindowOptions, canvas, deferred, div, ease_out_quint, outline, percentage, prelude::*, px,
    quad, rgb, size, surface, svg,
};
enum ViewerEvent {
    RenderReady,
    CompositionReady,
    CompositionFailed(String),
    Error(SourceId, u64, String),
}

const SIDEBAR_WIDTH: Pixels = px(280.0);
const INSPECTOR_WIDTH: Pixels = px(232.0);
const TITLEBAR_SAFE_AREA: Pixels = px(38.0);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const RENDER_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const SOURCE_MENU_ANIMATION_DURATION: Duration = Duration::from_millis(150);
const SCENE_ROW_ANIMATION_DURATION: Duration = Duration::from_millis(140);
const SCENE_ROW_STRIDE: f32 = 36.0;
const DEFAULT_CANVAS_DIMENSIONS: (usize, usize) = (1920, 1080);
const PREVIEW_CANVAS_DIMENSIONS: (usize, usize) = (1280, 720);
const PREFERRED_CAMERA_FPS: u32 = 60;
const MIN_ITEM_SCALE: f32 = 0.05;
const SNAP_THRESHOLD_PX: f32 = 8.0;
const CORNER_HANDLE_INSET: Pixels = px(16.0);

#[cfg(target_os = "macos")]
gpui::actions!(
    blip,
    [Quit, DeleteSelected, MoveLeft, MoveRight, MoveUp, MoveDown]
);

#[derive(Clone, Copy)]
struct CaptureOptions {
    fps: u32,
    cursor: bool,
}

impl From<&StreamArgs> for CaptureOptions {
    fn from(args: &StreamArgs) -> Self {
        Self {
            fps: args.fps,
            cursor: args.cursor,
        }
    }
}

#[derive(Clone)]
enum CaptureTarget {
    Display(CaptureDisplay),
    Window(CaptureWindow),
    Camera(CameraDevice),
}

impl CaptureTarget {
    fn id(&self) -> SourceId {
        match self {
            Self::Display(display) => SourceId::Display(display.id()),
            Self::Window(window) => SourceId::Window(window.id()),
            Self::Camera(camera) => SourceId::Camera(camera_id(camera.unique_id())),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Display(display) => format!(
                "Display {}  {}x{}",
                display.id(),
                display.physical_width(),
                display.physical_height()
            ),
            Self::Window(window) => {
                let application = window
                    .application()
                    .map_or_else(|| "Unknown app".into(), |application| application.name());
                let title = window.title().unwrap_or_else(|| "Untitled".into());
                format!("{application} - {title}")
            }
            Self::Camera(camera) => camera.localized_name().to_owned(),
        }
    }

    fn filter(&self) -> Option<CaptureFilter> {
        match self {
            Self::Display(display) => Some(CaptureFilter::from(display.clone())),
            Self::Window(window) => Some(CaptureFilter::from(window)),
            Self::Camera(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
enum SourceGroup {
    Displays,
    Windows,
    Cameras,
}

impl SourceGroup {
    fn contains(self, target: &CaptureTarget) -> bool {
        matches!(
            (self, target),
            (Self::Displays, CaptureTarget::Display(_))
                | (Self::Windows, CaptureTarget::Window(_))
                | (Self::Cameras, CaptureTarget::Camera(_))
        )
    }
}

const fn full_canvas_layout() -> ItemLayout {
    ItemLayout {
        center: [0.5, 0.5],
        base_size: [1.0, 1.0],
        scale: 1.0,
        corner_radius: 0.0,
    }
}

const fn inset_layout() -> ItemLayout {
    ItemLayout {
        center: [0.5, 0.5],
        base_size: [0.5, 0.5],
        scale: 1.0,
        corner_radius: 48.0,
    }
}

struct CaptureResource {
    capturer: CaptureBackend,
    generation: u64,
}

enum CaptureBackend {
    Screen(Capturer),
    Camera(CameraCapturer),
}

impl CaptureBackend {
    fn start(&self) -> anyhow::Result<()> {
        match self {
            Self::Screen(capturer) => capturer.start().map_err(Into::into),
            Self::Camera(capturer) => capturer.start().map_err(Into::into),
        }
    }

    fn stop(&self) -> anyhow::Result<()> {
        match self {
            Self::Screen(capturer) => capturer.stop().map_err(Into::into),
            Self::Camera(capturer) => {
                capturer.stop();
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SourceId {
    Display(u32),
    Window(u32),
    Camera(u64),
    Color(u64),
}

#[derive(Clone, Copy)]
struct ColorSource {
    color: [u8; 3],
}

#[derive(Clone, Copy)]
struct ItemLayout {
    center: [f32; 2],
    base_size: [f32; 2],
    scale: f32,
    corner_radius: f32,
}

type ElementId = u64;

#[derive(Clone, Copy)]
struct SceneElement {
    id: ElementId,
    source: SourceId,
    layout: ItemLayout,
}

struct Scene {
    elements: Vec<SceneElement>,
    next_id: ElementId,
}

impl Scene {
    fn new() -> Self {
        Self {
            elements: Vec::new(),
            next_id: 1,
        }
    }

    fn add(&mut self, source: SourceId, layout: ItemLayout) -> ElementId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.elements.push(SceneElement { id, source, layout });
        id
    }

    fn element(&self, id: ElementId) -> Option<&SceneElement> {
        self.elements.iter().find(|element| element.id == id)
    }

    fn element_mut(&mut self, id: ElementId) -> Option<&mut SceneElement> {
        self.elements.iter_mut().find(|element| element.id == id)
    }

    fn remove(&mut self, id: ElementId) -> Option<SceneElement> {
        let index = self.elements.iter().position(|element| element.id == id)?;
        Some(self.elements.remove(index))
    }

    fn uses_source(&self, source: SourceId) -> bool {
        self.elements.iter().any(|element| element.source == source)
    }

    fn move_to_index(&mut self, item: ElementId, index: usize) -> bool {
        let Some(current_index) = self.elements.iter().position(|element| element.id == item)
        else {
            return false;
        };
        let destination = index.min(self.elements.len().saturating_sub(1));
        if current_index == destination {
            return false;
        }
        let element = self.elements.remove(current_index);
        self.elements.insert(destination, element);
        true
    }
}

#[derive(Clone, Copy)]
struct RenderedElement {
    id: ElementId,
    transform: ItemTransform,
}

#[derive(Clone)]
struct SceneDrag {
    item: ElementId,
    label: String,
    grab_offset: Point<Pixels>,
    list_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Render for SceneDrag {
    #[allow(clippy::arithmetic_side_effects)]
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mouse = window.mouse_position();
        let root = Point::new(mouse.x - self.grab_offset.x, mouse.y - self.grab_offset.y);
        let desired = self
            .list_bounds
            .get()
            .map_or(root, |bounds| clamped_scene_drag_origin(root, bounds));
        div().pl(desired.x - root.x).pt(desired.y - root.y).child(
            div()
                .w(px(256.0))
                .h(px(32.0))
                .px_2()
                .flex()
                .items_center()
                .rounded_sm()
                .bg(rgb(theme::CONTROL_ACTIVE))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .text_xs()
                .text_color(rgb(theme::TEXT))
                .child(
                    svg()
                        .size(px(14.0))
                        .flex_none()
                        .path(GRIP_VERTICAL)
                        .text_color(rgb(theme::TEXT_DIM))
                        .mr_1(),
                )
                .child(self.label.clone()),
        )
    }
}

#[derive(Clone)]
struct SourceFrame {
    pixel_buffer: CVPixelBuffer,
    content_rect: Option<blip_sck::FrameRect>,
    dimensions: (f64, f64),
}

// SAFETY: CoreVideo pixel buffers are immutable while retained and may be shared between the
// capture, compositor, and presentation threads.
unsafe impl Send for SourceFrame {}

struct RenderRequest {
    elements: Vec<SceneElement>,
    frames: HashMap<SourceId, SourceFrame>,
    colors: HashMap<SourceId, ColorSource>,
    locked_dimensions: Option<(usize, usize)>,
}

struct ComposedFrame {
    image: CVPixelBuffer,
    dimensions: (usize, usize),
    elements: Vec<RenderedElement>,
}

// SAFETY: The compositor finishes writing the retained CoreVideo buffer before sending it to the
// presentation thread. The remaining fields contain only owned numeric data.
unsafe impl Send for ComposedFrame {}

struct CompositionHub {
    latest: Mutex<Option<ComposedFrame>>,
    event_pending: AtomicBool,
    sender: Sender<ViewerEvent>,
}

impl CompositionHub {
    fn submit(&self, frame: ComposedFrame) {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *latest = Some(frame);
        if !self.event_pending.swap(true, Ordering::AcqRel) {
            let _ = self.sender.try_send(ViewerEvent::CompositionReady);
        }
    }

    fn take(&self) -> Option<ComposedFrame> {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = latest.take();
        self.event_pending.store(false, Ordering::Release);
        frame
    }
}

#[derive(Default)]
struct RenderQueue {
    pending: Mutex<Option<RenderRequest>>,
    ready: Condvar,
}

impl RenderQueue {
    fn submit(&self, request: RenderRequest) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pending = Some(request);
        self.ready.notify_one();
    }

    fn take(&self) -> RenderRequest {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(request) = pending.take() {
                return request;
            }
            pending = self
                .ready
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn take_pending(&self) -> Option<RenderRequest> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

struct FrameHub {
    frames: Mutex<HashMap<SourceId, CapturedFrame>>,
    camera_statuses: Mutex<HashMap<SourceId, CameraStatus>>,
    removed_sources: Mutex<Vec<SourceId>>,
    render_pending: AtomicBool,
    sender: Sender<ViewerEvent>,
}

#[derive(Clone, Copy)]
struct CameraStatus {
    window_started: Instant,
    frames: u32,
    measured_fps: Option<f32>,
    dimensions: (usize, usize),
    dropped_frames: u64,
}

impl CameraStatus {
    fn record(&mut self, now: Instant, dimensions: (usize, usize)) {
        self.frames = self.frames.saturating_add(1);
        self.dimensions = dimensions;
        let elapsed = now.saturating_duration_since(self.window_started);
        if elapsed >= Duration::from_secs(1) {
            let frames = u16::try_from(self.frames).unwrap_or(u16::MAX);
            self.measured_fps = Some(f32::from(frames) / elapsed.as_secs_f32());
            self.window_started = now;
            self.frames = 0;
        }
    }
}

enum CapturedFrame {
    Screen(VideoFrame),
    Camera(CameraFrame),
}

impl FrameHub {
    fn submit(&self, source: SourceId, frame: CapturedFrame) {
        if let CapturedFrame::Camera(camera) = &frame
            && let Ok(mut statuses) = self.camera_statuses.lock()
        {
            let now = Instant::now();
            statuses
                .entry(source)
                .or_insert(CameraStatus {
                    window_started: now,
                    frames: 0,
                    measured_fps: None,
                    dimensions: (camera.width(), camera.height()),
                    dropped_frames: 0,
                })
                .record(now, (camera.width(), camera.height()));
        }
        if let Ok(mut frames) = self.frames.lock() {
            frames.insert(source, frame);
        }
        self.request_render();
    }

    fn drain(&self) -> HashMap<SourceId, CapturedFrame> {
        let mut frames = self
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let drained = std::mem::take(&mut *frames);
        self.render_pending.store(false, Ordering::Release);
        drained
    }

    fn drain_removed_sources(&self) -> Vec<SourceId> {
        let mut sources = self
            .removed_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *sources)
    }

    fn report_error(&self, source: SourceId, generation: u64, message: String) {
        let _ = self
            .sender
            .try_send(ViewerEvent::Error(source, generation, message));
    }

    fn scene_changed(&self, removed_source: Option<SourceId>) {
        if let Some(source) = removed_source
            && let Ok(mut frames) = self.frames.lock()
        {
            frames.remove(&source);
            if let Ok(mut removed_sources) = self.removed_sources.lock() {
                removed_sources.push(source);
            }
            if let Ok(mut statuses) = self.camera_statuses.lock() {
                statuses.remove(&source);
            }
        }
        self.request_render();
    }

    fn request_render(&self) {
        if !self.render_pending.swap(true, Ordering::AcqRel) {
            let _ = self.sender.try_send(ViewerEvent::RenderReady);
        }
    }

    fn camera_status(&self, source: SourceId) -> Option<CameraStatus> {
        self.camera_statuses
            .lock()
            .ok()
            .and_then(|statuses| statuses.get(&source).copied())
    }

    fn report_camera_drop(&self, source: SourceId) {
        if let Ok(mut statuses) = self.camera_statuses.lock()
            && let Some(status) = statuses.get_mut(&source)
        {
            status.dropped_frames = status.dropped_frames.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TransformAdjustment {
    X,
    Y,
    Width,
    Height,
    CornerRadius,
    ColorRed,
    ColorGreen,
    ColorBlue,
}

impl ResizeHandle {
    const fn axes(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [-1.0, -1.0],
            Self::Top => [0.0, -1.0],
            Self::TopRight => [1.0, -1.0],
            Self::Right => [1.0, 0.0],
            Self::BottomRight => [1.0, 1.0],
            Self::Bottom => [0.0, 1.0],
            Self::BottomLeft => [-1.0, 1.0],
            Self::Left => [-1.0, 0.0],
        }
    }
}

#[derive(Clone, Copy)]
enum DragOperation {
    Move {
        item: ElementId,
        offset: [f32; 2],
    },
    Resize {
        item: ElementId,
        handle: ResizeHandle,
        initial_transform: ItemTransform,
        initial_scale: f32,
        anchor: [f32; 2],
    },
    CornerRadius {
        item: ElementId,
        bounds: Bounds<Pixels>,
        preview_scale: f32,
    },
}

#[derive(Clone, Copy, Default, Debug)]
struct SnapGuides {
    x: Option<f32>,
    y: Option<f32>,
}

struct FrameViewer {
    image: Option<CVPixelBuffer>,
    content_dimensions: Option<(usize, usize)>,
    scene: Rc<RefCell<Scene>>,
    rendered_elements: Vec<RenderedElement>,
    selected_item: Option<ElementId>,
    drag_operation: Option<DragOperation>,
    snap_guides: SnapGuides,
    hovered_handle: Option<ResizeHandle>,
    corner_handle_hovered: bool,
    transform_inputs: TransformInputs,
    focused_transform_inputs: HashSet<TransformAdjustment>,
    source_menu_open: bool,
    source_menu_visible: bool,
    source_menu_transition: u64,
    scene_dragging_item: Option<ElementId>,
    scene_drag_grab_offset: Option<Point<Pixels>>,
    scene_item_offsets: HashMap<ElementId, f32>,
    scene_reorder_transition: u64,
    scene_item_animation_started: Option<Instant>,
    scene_list_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    scene_row_bounds: Rc<RefCell<HashMap<ElementId, Bounds<Pixels>>>>,
    targets: Vec<CaptureTarget>,
    options: CaptureOptions,
    captures: Rc<RefCell<HashMap<SourceId, CaptureResource>>>,
    pending_captures: HashMap<SourceId, u64>,
    failed_captures: HashSet<SourceId>,
    next_capture_generation: u64,
    colors: Rc<RefCell<HashMap<SourceId, ColorSource>>>,
    next_color_source_id: u64,
    frame_hub: Arc<FrameHub>,
    stream: Option<RtmpStream>,
    stream_config: Option<RtmpConfig>,
    stream_canvas: Rc<Cell<Option<(usize, usize)>>>,
    stream_generation: u64,
    stream_error_sender: Sender<(u64, String)>,
    control_error: Option<String>,
}

struct TransformInputs {
    x: Entity<NumericInput>,
    y: Entity<NumericInput>,
    width: Entity<NumericInput>,
    height: Entity<NumericInput>,
    corner_radius: Entity<NumericInput>,
    color_red: Entity<NumericInput>,
    color_green: Entity<NumericInput>,
    color_blue: Entity<NumericInput>,
}

impl TransformInputs {
    fn new(cx: &mut Context<FrameViewer>) -> Self {
        Self {
            x: transform_input("X", TransformAdjustment::X, cx),
            y: transform_input("Y", TransformAdjustment::Y, cx),
            width: transform_input("W", TransformAdjustment::Width, cx),
            height: transform_input("H", TransformAdjustment::Height, cx),
            corner_radius: transform_input("R", TransformAdjustment::CornerRadius, cx),
            color_red: transform_input("R", TransformAdjustment::ColorRed, cx),
            color_green: transform_input("G", TransformAdjustment::ColorGreen, cx),
            color_blue: transform_input("B", TransformAdjustment::ColorBlue, cx),
        }
    }
}

fn transform_input(
    label: &'static str,
    adjustment: TransformAdjustment,
    cx: &mut Context<FrameViewer>,
) -> Entity<NumericInput> {
    let input = cx.new(|cx| NumericInput::new(label, cx));
    cx.subscribe(
        &input,
        move |viewer, _, event: &NumericInputEvent, cx| match event {
            NumericInputEvent::Changed(value) => {
                viewer.set_transform_value(adjustment, *value, cx);
            }
            NumericInputEvent::FocusChanged(true) => {
                viewer.focused_transform_inputs.insert(adjustment);
                cx.notify();
            }
            NumericInputEvent::FocusChanged(false) => {
                viewer.focused_transform_inputs.remove(&adjustment);
                cx.notify();
            }
        },
    )
    .detach();
    input
}

impl FrameViewer {
    fn blur_inputs(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.blur();
        if self.source_menu_open {
            self.close_source_menu(cx);
        }
    }

    #[allow(clippy::unused_self)]
    fn stop_mouse_propagation(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
    }

    fn toggle_source_menu(&mut self, cx: &mut Context<Self>) {
        if self.source_menu_open {
            self.close_source_menu(cx);
            return;
        }
        self.source_menu_transition = self.source_menu_transition.saturating_add(1);
        self.source_menu_visible = true;
        self.source_menu_open = true;
        cx.notify();
    }

    fn close_source_menu(&mut self, cx: &mut Context<Self>) {
        if !self.source_menu_visible || !self.source_menu_open {
            return;
        }
        self.source_menu_transition = self.source_menu_transition.saturating_add(1);
        let transition = self.source_menu_transition;
        self.source_menu_open = false;
        let viewer = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(SOURCE_MENU_ANIMATION_DURATION)
                .await;
            let _ = viewer.update(cx, |viewer, cx| {
                if !viewer.source_menu_open && viewer.source_menu_transition == transition {
                    viewer.source_menu_visible = false;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn select_scene_item(&mut self, item: ElementId, cx: &mut Context<Self>) {
        self.selected_item = Some(item);
        cx.notify();
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn drag_scene_item_over(&mut self, event: &DragMoveEvent<SceneDrag>, cx: &mut Context<Self>) {
        let dragged = event.drag(cx).item;
        let Some(grab_offset) = self.scene_drag_grab_offset else {
            return;
        };
        let Some(bounds) = self.scene_list_bounds.get() else {
            return;
        };
        let mut scene = self.scene.borrow_mut();
        let Some(destination) = scene_drag_render_index(
            event.event.position.y,
            grab_offset.y,
            bounds,
            scene.elements.len(),
        ) else {
            return;
        };
        let previous_order = scene
            .elements
            .iter()
            .map(|element| element.id)
            .collect::<Vec<_>>();
        if scene.move_to_index(dragged, destination) {
            self.scene_item_offsets = scene_reorder_offsets(&previous_order, &scene, dragged);
            self.scene_reorder_transition = self.scene_reorder_transition.saturating_add(1);
            self.scene_item_animation_started = Some(Instant::now());
            drop(scene);
            self.frame_hub.scene_changed(None);
            cx.notify();
        }
    }

    fn drop_scene_item(
        &mut self,
        drag: &SceneDrag,
        _target: ElementId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.animate_scene_drag_release(drag.item, window);
        self.scene_dragging_item = None;
        self.selected_item = Some(drag.item);
        cx.notify();
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn animate_scene_drag_release(&mut self, item: ElementId, window: &Window) {
        let Some(grab_offset) = self.scene_drag_grab_offset.take() else {
            return;
        };
        let Some(bounds) = self.scene_list_bounds.get() else {
            return;
        };
        let mouse = window.mouse_position();
        let root = Point::new(mouse.x - grab_offset.x, mouse.y - grab_offset.y);
        let release_origin = clamped_scene_drag_origin(root, bounds);
        let final_origin = self
            .scene_row_bounds
            .borrow()
            .get(&item)
            .map(|bounds| bounds.origin)
            .or_else(|| scene_item_origin(&self.scene.borrow(), item, bounds));
        let Some(final_origin) = final_origin else {
            return;
        };
        let remaining = self
            .scene_item_animation_started
            .map_or(0.0, |started| scene_animation_remaining(started.elapsed()));
        for offset in self.scene_item_offsets.values_mut() {
            *offset *= remaining;
        }
        self.scene_item_offsets
            .retain(|_, offset| offset.abs() > 0.1);
        let release_offset = (release_origin.y - final_origin.y).as_f32();
        if release_offset.abs() > 0.1 {
            self.scene_item_offsets.insert(item, release_offset);
        }
        self.scene_reorder_transition = self.scene_reorder_transition.saturating_add(1);
        self.scene_item_animation_started = Some(Instant::now());
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if !self.focused_transform_inputs.is_empty() {
            return;
        }
        let Some(selected) = self.selected_item.take() else {
            return;
        };
        let removed = self.scene.borrow_mut().remove(selected);
        let Some(removed) = removed else {
            return;
        };
        let source_in_use = self.scene.borrow().uses_source(removed.source);
        let removed_source = if source_in_use {
            None
        } else {
            match removed.source {
                SourceId::Color(_) => {
                    self.colors.borrow_mut().remove(&removed.source);
                }
                SourceId::Display(_) | SourceId::Window(_) | SourceId::Camera(_) => {
                    self.pending_captures.remove(&removed.source);
                    self.failed_captures.remove(&removed.source);
                    if let Some(resource) = self.captures.borrow_mut().remove(&removed.source)
                        && let Err(error) = resource.capturer.stop()
                    {
                        self.control_error = Some(error.to_string());
                    }
                }
            }
            Some(removed.source)
        };
        self.rendered_elements
            .retain(|element| element.id != selected);
        self.drag_operation = None;
        self.hovered_handle = None;
        self.corner_handle_hovered = false;
        self.frame_hub.scene_changed(removed_source);
        cx.notify();
    }

    #[allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::cast_precision_loss
    )]
    fn move_selected(&mut self, delta: [f32; 2], cx: &mut Context<Self>) {
        if !self.focused_transform_inputs.is_empty() {
            return;
        }
        let Some(selected) = self.selected_item else {
            return;
        };
        let (width, height) = self.content_dimensions.unwrap_or(DEFAULT_CANVAS_DIMENSIONS);
        let Ok(width) = u32::try_from(width) else {
            return;
        };
        let Ok(height) = u32::try_from(height) else {
            return;
        };
        let mut scene = self.scene.borrow_mut();
        let Some(element) = scene.element_mut(selected) else {
            return;
        };
        element.layout.center =
            nudged_center(element.layout.center, delta, [width as f32, height as f32]);
        drop(scene);
        self.frame_hub.scene_changed(None);
        cx.notify();
    }

    fn toggle_stream(&mut self, cx: &mut Context<Self>) {
        self.stream_generation = self.stream_generation.saturating_add(1);
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
            self.stream_canvas.set(None);
            self.control_error = None;
            cx.notify();
            return;
        }

        let Some(config) = self.stream_config.clone() else {
            self.control_error = Some("start with --rtmp-url to configure a destination".into());
            cx.notify();
            return;
        };
        let Some(dimensions) = self.content_dimensions else {
            self.control_error = Some("waiting for the first captured frame".into());
            cx.notify();
            return;
        };
        let dimensions = even_dimensions(dimensions);
        match RtmpStream::start(
            config,
            dimensions,
            self.stream_generation,
            self.stream_error_sender.clone(),
        ) {
            Ok(stream) => {
                self.stream_canvas.set(Some(dimensions));
                self.stream = Some(stream);
                self.control_error = None;
            }
            Err(error) => self.control_error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn stream_failed(&mut self, generation: u64, message: String, cx: &mut Context<Self>) {
        if generation != self.stream_generation {
            return;
        }
        self.stream.take();
        self.stream_canvas.set(None);
        self.control_error = Some(message);
        cx.notify();
    }

    fn show_frame(
        &mut self,
        image: CVPixelBuffer,
        dimensions: (usize, usize),
        elements: Vec<RenderedElement>,
        cx: &mut Context<Self>,
    ) {
        if let Some(stream) = &self.stream
            && let Err(error) = stream.send(image.clone())
        {
            self.stream.take();
            self.stream_canvas.set(None);
            self.control_error = Some(error.to_string());
        }
        self.image = Some(image);
        self.content_dimensions = Some(dimensions);
        self.rendered_elements = elements;
        cx.notify();
    }

    fn add_capture_target(&mut self, target_index: usize, cx: &mut Context<Self>) {
        let Some(target) = self.targets.get(target_index).cloned() else {
            return;
        };
        if let CaptureTarget::Camera(camera) = target {
            self.add_camera_target(camera, cx);
            return;
        }
        let source = target.id();
        if !self.captures.borrow().contains_key(&source) {
            let generation = 1;
            let capturer = match build_capturer(&target, self.options, generation, &self.frame_hub)
            {
                Ok(capturer) => capturer,
                Err(error) => {
                    self.control_error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            };
            if let Err(error) = capturer.start() {
                self.control_error = Some(error.to_string());
                cx.notify();
                return;
            }
            self.captures.borrow_mut().insert(
                source,
                CaptureResource {
                    capturer,
                    generation,
                },
            );
        }
        let id = self.scene.borrow_mut().add(source, inset_layout());
        self.selected_item = Some(id);
        self.close_source_menu(cx);
        self.control_error = None;
        self.frame_hub.scene_changed(None);
        cx.notify();
    }

    fn add_camera_target(&mut self, camera: CameraDevice, cx: &mut Context<Self>) {
        let source = SourceId::Camera(camera_id(camera.unique_id()));
        let id = self.scene.borrow_mut().add(source, inset_layout());
        self.selected_item = Some(id);
        self.close_source_menu(cx);
        self.control_error = None;
        self.frame_hub.scene_changed(None);

        if self.captures.borrow().contains_key(&source)
            || self.pending_captures.contains_key(&source)
        {
            cx.notify();
            return;
        }

        let generation = self.next_capture_generation;
        self.next_capture_generation = self.next_capture_generation.saturating_add(1);
        self.pending_captures.insert(source, generation);
        self.failed_captures.remove(&source);

        let viewer = cx.weak_entity();
        let frame_hub = Arc::clone(&self.frame_hub);
        let background = cx.background_executor().spawn(async move {
            let callback_hub = Arc::clone(&frame_hub);
            let drop_hub = Arc::clone(&frame_hub);
            let capturer = CameraCapturer::new_with_drop_callback(
                &camera,
                PREFERRED_CAMERA_FPS,
                move |frame| callback_hub.submit(source, CapturedFrame::Camera(frame)),
                move || drop_hub.report_camera_drop(source),
            )?;
            capturer.start()?;
            Ok::<_, blip_avfoundation::CameraError>(capturer)
        });
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = viewer.update(cx, |viewer, cx| {
                viewer.camera_started(source, generation, result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn camera_started(
        &mut self,
        source: SourceId,
        generation: u64,
        result: Result<CameraCapturer, blip_avfoundation::CameraError>,
        cx: &mut Context<Self>,
    ) {
        if !should_finish_pending_capture(
            self.pending_captures.get(&source).copied(),
            generation,
            self.scene.borrow().uses_source(source),
        ) {
            if let Ok(capturer) = result {
                capturer.stop();
            }
            return;
        }
        self.pending_captures.remove(&source);
        match result {
            Ok(capturer) => {
                self.captures.borrow_mut().insert(
                    source,
                    CaptureResource {
                        capturer: CaptureBackend::Camera(capturer),
                        generation,
                    },
                );
                self.failed_captures.remove(&source);
                self.control_error = None;
            }
            Err(error) => {
                self.failed_captures.insert(source);
                self.control_error = Some(error.to_string());
            }
        }
        self.frame_hub.scene_changed(None);
        cx.notify();
    }

    fn add_color_source(&mut self, cx: &mut Context<Self>) {
        let source = SourceId::Color(self.next_color_source_id);
        self.next_color_source_id = self.next_color_source_id.saturating_add(1);
        self.colors.borrow_mut().insert(
            source,
            ColorSource {
                color: [88, 101, 242],
            },
        );
        let id = self.scene.borrow_mut().add(source, full_canvas_layout());
        self.selected_item = Some(id);
        self.close_source_menu(cx);
        self.frame_hub.scene_changed(None);
        cx.notify();
    }

    fn source_label(&self, source: SourceId) -> String {
        match source {
            SourceId::Color(_) => "Color".into(),
            SourceId::Camera(_) => {
                let label = self
                    .targets
                    .iter()
                    .find(|target| target.id() == source)
                    .map_or_else(|| "Unavailable camera".into(), CaptureTarget::label);
                if self.pending_captures.contains_key(&source) {
                    return format!("{label} - Loading...");
                }
                if self.failed_captures.contains(&source) {
                    return format!("{label} - Failed");
                }
                self.frame_hub
                    .camera_status(source)
                    .map_or(label.clone(), |status| {
                        let (width, height) = status.dimensions;
                        let label = status.measured_fps.map_or_else(
                            || format!("{label} - {width}x{height}"),
                            |fps| format!("{label} - {width}x{height} - {fps:.1} FPS"),
                        );
                        if status.dropped_frames == 0 {
                            label
                        } else {
                            format!("{label} - {} dropped", status.dropped_frames)
                        }
                    })
            }
            SourceId::Display(_) | SourceId::Window(_) => self
                .targets
                .iter()
                .find(|target| target.id() == source)
                .map_or_else(|| "Unavailable capture".into(), CaptureTarget::label),
        }
    }

    fn selected_color(&self) -> Option<[u8; 3]> {
        let item = self.selected_item?;
        let source = self.scene.borrow().element(item)?.source;
        self.colors.borrow().get(&source).map(|color| color.color)
    }

    fn set_selected_color(&mut self, color: [u8; 3], cx: &mut Context<Self>) {
        let Some(item) = self.selected_item else {
            return;
        };
        let Some(source) = self
            .scene
            .borrow()
            .element(item)
            .map(|element| element.source)
        else {
            return;
        };
        let mut colors = self.colors.borrow_mut();
        let Some(source) = colors.get_mut(&source) else {
            return;
        };
        source.color = color;
        self.frame_hub.scene_changed(None);
        cx.notify();
    }

    fn capture_failed(
        &mut self,
        source: SourceId,
        generation: u64,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        let source_in_use = self.scene.borrow().uses_source(source);
        let current_generation = self
            .captures
            .borrow()
            .get(&source)
            .map(|resource| resource.generation);
        if !should_restart_capture(source_in_use, current_generation, generation) {
            return;
        }
        self.captures.borrow_mut().remove(&source);
        let Some(target) = self
            .targets
            .iter()
            .find(|target| target.id() == source)
            .cloned()
        else {
            self.control_error = Some(format!("{source:?}: {message}; source is unavailable"));
            cx.notify();
            return;
        };
        let next_generation = generation.saturating_add(1);
        let restarted = build_capturer(&target, self.options, next_generation, &self.frame_hub)
            .and_then(|capturer| {
                capturer
                    .start()
                    .map_err(|error| CaptureError::Framework(error.to_string()))?;
                Ok(capturer)
            });
        match restarted {
            Ok(capturer) => {
                self.captures.borrow_mut().insert(
                    source,
                    CaptureResource {
                        capturer,
                        generation: next_generation,
                    },
                );
                self.control_error = None;
            }
            Err(error) => {
                self.control_error = Some(format!(
                    "{source:?}: {message}; failed to restart capture: {error}"
                ));
            }
        }
        cx.notify();
    }

    fn rendered_transform(&self, item: ElementId) -> Option<ItemTransform> {
        self.rendered_elements
            .iter()
            .find(|element| element.id == item)
            .map(|element| element.transform)
    }

    #[allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::cast_precision_loss
    )]
    fn begin_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.blur();
        self.snap_guides = SnapGuides::default();
        let Some(dimensions) = self.content_dimensions else {
            return;
        };
        let Some(frame_bounds) = preview_frame_bounds(window, dimensions) else {
            return;
        };
        if let Some(item) = self.selected_item {
            let Some(transform) = self.rendered_transform(item) else {
                return;
            };
            let bounds = transform_bounds(frame_bounds, transform);
            let radius = item_corner_radius(frame_bounds, dimensions, transform);
            if corner_radius_handle_bounds(bounds, radius).contains(&event.position) {
                let preview_scale = frame_bounds.size.width / px(dimensions.0 as f32);
                self.drag_operation = Some(DragOperation::CornerRadius {
                    item,
                    bounds,
                    preview_scale,
                });
                self.corner_handle_hovered = true;
                self.hovered_handle = None;
                cx.notify();
                return;
            }
            if let Some(handle) = resize_handle_at(event.position, bounds) {
                let Some(layout) = self
                    .scene
                    .borrow()
                    .element(item)
                    .map(|element| element.layout)
                else {
                    return;
                };
                self.drag_operation = Some(resize_operation(item, handle, transform, layout.scale));
                self.hovered_handle = Some(handle);
                self.corner_handle_hovered = false;
                cx.notify();
                return;
            }
        }
        let selected_item = self
            .rendered_elements
            .iter()
            .rev()
            .find(|element| {
                transform_bounds(frame_bounds, element.transform).contains(&event.position)
            })
            .map(|element| element.id);
        self.selected_item = selected_item;
        self.hovered_handle = None;
        self.corner_handle_hovered = false;
        self.drag_operation = selected_item.map(|item| {
            let [center_x, center_y] = self
                .rendered_transform(item)
                .map_or([0.5, 0.5], |transform| transform.center);
            DragOperation::Move {
                item,
                offset: [
                    normalized_position(
                        event.position.x,
                        frame_bounds.origin.x,
                        frame_bounds.size.width,
                    ) - center_x,
                    normalized_position(
                        event.position.y,
                        frame_bounds.origin.y,
                        frame_bounds.size.height,
                    ) - center_y,
                ],
            }
        });
        cx.notify();
    }

    fn drag_inset(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(operation) = self.drag_operation {
            match operation {
                DragOperation::Move { item, offset } => {
                    self.reposition_item(item, offset, event.position, window);
                }
                DragOperation::Resize { .. } => self.resize_item(operation, event.position, window),
                DragOperation::CornerRadius { .. } => {
                    self.adjust_corner_radius(operation, event.position);
                }
            }
            cx.notify();
            return;
        }
        let hovered = self.selected_item.and_then(|item| {
            let dimensions = self.content_dimensions?;
            let frame = preview_frame_bounds(window, dimensions)?;
            let transform = self.rendered_transform(item)?;
            let bounds = transform_bounds(frame, transform);
            let radius = item_corner_radius(frame, dimensions, transform);
            if corner_radius_handle_bounds(bounds, radius).contains(&event.position) {
                Some((None, true))
            } else {
                Some((resize_handle_at(event.position, bounds), false))
            }
        });
        let (hovered_handle, corner_handle_hovered) = hovered.unwrap_or((None, false));
        if hovered_handle != self.hovered_handle
            || corner_handle_hovered != self.corner_handle_hovered
        {
            self.hovered_handle = hovered_handle;
            self.corner_handle_hovered = corner_handle_hovered;
            cx.notify();
        }
    }

    fn end_drag(&mut self, _event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_operation = None;
        self.snap_guides = SnapGuides::default();
        if let Some(item) = self.scene_dragging_item {
            self.animate_scene_drag_release(item, window);
        }
        self.scene_dragging_item = None;
        cx.notify();
    }

    fn reposition_item(
        &mut self,
        item: ElementId,
        offset: [f32; 2],
        position: Point<Pixels>,
        window: &Window,
    ) {
        let Some(dimensions) = self.content_dimensions else {
            return;
        };
        let Some(frame_bounds) = preview_frame_bounds(window, dimensions) else {
            return;
        };
        let [offset_x, offset_y] = offset;
        let center = [
            normalized_position_unclamped(
                position.x,
                frame_bounds.origin.x,
                frame_bounds.size.width,
            ) - offset_x,
            normalized_position_unclamped(
                position.y,
                frame_bounds.origin.y,
                frame_bounds.size.height,
            ) - offset_y,
        ];
        let Some(transform) = self.rendered_transform(item) else {
            return;
        };
        let (center, guides) = snapped_move_center(
            center,
            transform.size,
            normalized_snap_threshold(frame_bounds),
        );
        self.snap_guides = guides;
        if let Some(element) = self.scene.borrow_mut().element_mut(item) {
            element.layout.center = center;
            self.frame_hub.scene_changed(None);
        }
    }

    fn resize_item(&mut self, operation: DragOperation, position: Point<Pixels>, window: &Window) {
        let DragOperation::Resize {
            item,
            handle,
            initial_transform,
            initial_scale,
            anchor,
        } = operation
        else {
            return;
        };
        let Some(dimensions) = self.content_dimensions else {
            return;
        };
        let Some(frame) = preview_frame_bounds(window, dimensions) else {
            return;
        };
        let pointer = [
            normalized_position_unclamped(position.x, frame.origin.x, frame.size.width),
            normalized_position_unclamped(position.y, frame.origin.y, frame.size.height),
        ];
        let (center, scale_ratio, guides) = resized_item_with_border_snap(
            handle,
            initial_transform,
            anchor,
            pointer,
            MIN_ITEM_SCALE / initial_scale,
            normalized_snap_threshold(frame),
        );
        self.snap_guides = guides;
        if let Some(element) = self.scene.borrow_mut().element_mut(item) {
            element.layout.center = center;
            element.layout.scale = initial_scale * scale_ratio;
            self.frame_hub.scene_changed(None);
        }
    }

    fn adjust_corner_radius(&self, operation: DragOperation, position: Point<Pixels>) {
        let DragOperation::CornerRadius {
            item,
            bounds,
            preview_scale,
        } = operation
        else {
            return;
        };
        let radius = corner_radius_from_position(bounds, preview_scale, position);
        if let Some(element) = self.scene.borrow_mut().element_mut(item) {
            element.layout.corner_radius = radius;
            self.frame_hub.scene_changed(None);
        }
    }

    #[allow(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn set_transform_value(
        &mut self,
        adjustment: TransformAdjustment,
        value: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.selected_item else {
            return;
        };
        let color_channel = match adjustment {
            TransformAdjustment::ColorRed => Some(0),
            TransformAdjustment::ColorGreen => Some(1),
            TransformAdjustment::ColorBlue => Some(2),
            TransformAdjustment::X
            | TransformAdjustment::Y
            | TransformAdjustment::Width
            | TransformAdjustment::Height
            | TransformAdjustment::CornerRadius => None,
        };
        if let Some(channel) = color_channel {
            let Some(source) = self
                .scene
                .borrow()
                .element(item)
                .map(|element| element.source)
            else {
                return;
            };
            let mut colors = self.colors.borrow_mut();
            let Some(color) = colors.get_mut(&source) else {
                return;
            };
            let Some(channel) = color.color.get_mut(channel) else {
                return;
            };
            *channel = value.round().clamp(0.0, 255.0) as u8;
            self.frame_hub.scene_changed(None);
            cx.notify();
            return;
        }
        let Some((canvas_width, canvas_height)) = self.content_dimensions else {
            return;
        };
        let canvas_width = canvas_width as f32;
        let canvas_height = canvas_height as f32;
        let Some(transform) = self.rendered_transform(item) else {
            return;
        };
        let mut scene = self.scene.borrow_mut();
        let Some(element) = scene.element_mut(item) else {
            return;
        };
        let layout = &mut element.layout;
        match adjustment {
            TransformAdjustment::X => {
                layout.center[0] = value / canvas_width + transform.size[0] * 0.5;
            }
            TransformAdjustment::Y => {
                layout.center[1] = value / canvas_height + transform.size[1] * 0.5;
            }
            TransformAdjustment::Width => {
                let width = transform.size[0] * canvas_width;
                layout.scale = (layout.scale * value.max(1.0) / width).max(MIN_ITEM_SCALE);
            }
            TransformAdjustment::Height => {
                let height = transform.size[1] * canvas_height;
                layout.scale = (layout.scale * value.max(1.0) / height).max(MIN_ITEM_SCALE);
            }
            TransformAdjustment::CornerRadius => {
                let maximum =
                    (transform.size[0] * canvas_width).min(transform.size[1] * canvas_height) * 0.5;
                layout.corner_radius = value.clamp(0.0, maximum);
            }
            TransformAdjustment::ColorRed
            | TransformAdjustment::ColorGreen
            | TransformAdjustment::ColorBlue => return,
        }
        drop(scene);
        self.frame_hub.scene_changed(None);
        cx.notify();
    }

    #[allow(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        clippy::too_many_lines
    )]
    fn inspector(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let panel = div()
            .w(INSPECTOR_WIDTH)
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .px_3()
            .py_4()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_l_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .text_color(rgb(theme::TEXT));
        let Some(item) = self.selected_item else {
            return panel
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Transform"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::TEXT_DIM))
                        .child("Select an item to edit"),
                );
        };
        let (canvas_width, canvas_height) =
            self.content_dimensions.unwrap_or(DEFAULT_CANVAS_DIMENSIONS);
        let canvas_size = [canvas_width as f32, canvas_height as f32];
        let Some(transform) = self.rendered_transform(item) else {
            return panel.child("Waiting for source frame");
        };
        let x = (transform.center[0] - transform.size[0] * 0.5) * canvas_size[0];
        let y = (transform.center[1] - transform.size[1] * 0.5) * canvas_size[1];
        let width = transform.size[0] * canvas_size[0];
        let height = transform.size[1] * canvas_size[1];
        let radius = transform.clamped_corner_radius(canvas_size);
        Self::sync_transform_input(&self.transform_inputs.x, x, window, cx);
        Self::sync_transform_input(&self.transform_inputs.y, y, window, cx);
        Self::sync_transform_input(&self.transform_inputs.width, width, window, cx);
        Self::sync_transform_input(&self.transform_inputs.height, height, window, cx);
        Self::sync_transform_input(&self.transform_inputs.corner_radius, radius, window, cx);
        let selected_color = self.selected_color();
        if let Some(color) = selected_color {
            let [red, green, blue] = color;
            Self::sync_transform_input(
                &self.transform_inputs.color_red,
                f32::from(red),
                window,
                cx,
            );
            Self::sync_transform_input(
                &self.transform_inputs.color_green,
                f32::from(green),
                window,
                cx,
            );
            Self::sync_transform_input(
                &self.transform_inputs.color_blue,
                f32::from(blue),
                window,
                cx,
            );
        }
        let panel = panel
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Element {item}")),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Position"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().child(self.transform_inputs.x.clone()))
                    .child(div().flex_1().child(self.transform_inputs.y.clone())),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().child(self.transform_inputs.width.clone()))
                    .child(div().flex_1().child(self.transform_inputs.height.clone())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .child("Aspect ratio locked"),
            )
            .child(
                div()
                    .mt_1()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(theme::BORDER_SUBTLE))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Appearance"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(self.transform_inputs.corner_radius.clone()),
                    )
                    .child(div().flex_1()),
            );
        if let Some(color) = selected_color {
            panel.child(self.color_controls(color, cx))
        } else {
            panel
        }
    }

    fn sync_transform_input(
        input: &Entity<NumericInput>,
        value: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let focused = input.read(cx).focus_handle().is_focused(window);
        input.update(cx, |input, cx| input.set_value(value, focused, cx));
    }

    fn color_controls(&self, color: [u8; 3], cx: &mut Context<Self>) -> Div {
        const PRESETS: [[u8; 3]; 8] = [
            [17, 17, 17],
            [255, 255, 255],
            [239, 68, 68],
            [249, 115, 22],
            [234, 179, 8],
            [34, 197, 94],
            [59, 130, 246],
            [168, 85, 247],
        ];
        let swatches = PRESETS.into_iter().enumerate().map(|(index, preset)| {
            div()
                .id(format!("color-preset-{index}"))
                .size(px(20.0))
                .rounded_sm()
                .bg(rgb(color_rgb(preset)))
                .border_1()
                .border_color(rgb(if preset == color {
                    theme::FOCUS
                } else {
                    theme::BORDER
                }))
                .on_click(cx.listener(move |viewer, _, _, cx| {
                    viewer.set_selected_color(preset, cx);
                }))
        });
        div()
            .mt_1()
            .pt_3()
            .border_t_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Color"),
            )
            .child(div().flex().gap_1().children(swatches))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(self.transform_inputs.color_red.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(self.transform_inputs.color_green.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(self.transform_inputs.color_blue.clone()),
                    ),
            )
    }

    fn source_picker(&self, cx: &mut Context<Self>) -> Div {
        let opening = self.source_menu_open;
        let transition = self.source_menu_transition;
        let chevron = svg()
            .size(px(14.0))
            .path(CHEVRON_DOWN)
            .text_color(rgb(theme::TEXT_MUTED))
            .flex_none();
        let chevron = if transition == 0 {
            chevron.into_any_element()
        } else {
            chevron
                .with_animation(
                    format!("source-chevron-transition-{transition}"),
                    Animation::new(SOURCE_MENU_ANIMATION_DURATION).with_easing(ease_out_quint()),
                    move |chevron, delta| {
                        let progress = if opening { delta } else { 1.0 - delta };
                        chevron
                            .with_transformation(Transformation::rotate(percentage(progress * 0.5)))
                    },
                )
                .into_any_element()
        };
        let mut picker = div().relative().h(px(30.0)).child(
            div()
                .id("add-source")
                .w_full()
                .h(px(30.0))
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .rounded_sm()
                .bg(rgb(theme::CONTROL_BACKGROUND))
                .border_1()
                .border_color(rgb(theme::BORDER_SUBTLE))
                .hover(|button| button.bg(rgb(theme::CONTROL_HOVER)))
                .child(div().text_sm().child("Add source"))
                .child(chevron)
                .on_mouse_down(MouseButton::Left, cx.listener(Self::stop_mouse_propagation))
                .on_click(cx.listener(|viewer, _, _, cx| viewer.toggle_source_menu(cx))),
        );
        if self.source_menu_visible {
            let menu = div()
                .id("source-menu")
                .absolute()
                .left_0()
                .right_0()
                .max_h(px(280.0))
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .overflow_y_scroll()
                .rounded_sm()
                .bg(rgb(theme::CONTROL_BACKGROUND))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::stop_mouse_propagation))
                .child(self.source_picker_group("Displays", SourceGroup::Displays, cx))
                .child(self.source_picker_group("Windows", SourceGroup::Windows, cx))
                .child(self.source_picker_group("Cameras", SourceGroup::Cameras, cx))
                .child(Self::color_source_picker(cx))
                .with_animation(
                    format!("source-menu-transition-{transition}"),
                    Animation::new(SOURCE_MENU_ANIMATION_DURATION).with_easing(ease_out_quint()),
                    move |menu, delta| {
                        let visibility = if opening { delta } else { 1.0 - delta };
                        menu.top(px(34.0 - 6.0 * (1.0 - visibility)))
                            .opacity(visibility)
                    },
                );
            picker = picker.child(deferred(menu).priority(1));
        }
        picker
    }

    fn source_picker_group(
        &self,
        title: &'static str,
        group: SourceGroup,
        cx: &mut Context<Self>,
    ) -> Div {
        let counts = self.scene.borrow().elements.iter().fold(
            HashMap::<SourceId, usize>::new(),
            |mut counts, element| {
                let count = counts.entry(element.source).or_default();
                *count = count.saturating_add(1);
                counts
            },
        );
        let targets = self
            .targets
            .iter()
            .enumerate()
            .filter(|(_, target)| group.contains(target))
            .map(|(index, target)| {
                let count = counts.get(&target.id()).copied().unwrap_or_default();
                let label = if count == 0 {
                    target.label()
                } else {
                    format!("{} ({count})", target.label())
                };
                div()
                    .id(format!("source-option-{index}"))
                    .w_full()
                    .min_h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .text_xs()
                    .hover(|option| option.bg(rgb(theme::CONTROL_HOVER)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .on_click(cx.listener(move |viewer, _, _, cx| {
                        viewer.add_capture_target(index, cx);
                    }))
            });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_DIM))
                    .child(title),
            )
            .children(targets)
    }

    fn color_source_picker(cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_DIM))
                    .child("Other"),
            )
            .child(
                div()
                    .id("source-option-color")
                    .w_full()
                    .min_h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .text_xs()
                    .hover(|option| option.bg(rgb(theme::CONTROL_HOVER)))
                    .child(
                        div()
                            .size(px(14.0))
                            .rounded_sm()
                            .bg(rgb(0x0058_65f2))
                            .border_1()
                            .border_color(rgb(theme::BORDER)),
                    )
                    .child("Color")
                    .on_click(cx.listener(|viewer, _, _, cx| viewer.add_color_source(cx))),
            )
    }

    #[allow(clippy::arithmetic_side_effects, clippy::too_many_lines)]
    fn scene_items(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let elements = self.scene.borrow().elements.clone();
        let viewer = cx.weak_entity();
        let scene_list_bounds = Rc::clone(&self.scene_list_bounds);
        let measured_bounds = Rc::clone(&self.scene_list_bounds);
        let scene_row_bounds = Rc::clone(&self.scene_row_bounds);
        let rows = elements.into_iter().rev().map(|element| {
            let selected = self.selected_item == Some(element.id);
            let label = self.source_label(element.source);
            let source_ready = matches!(element.source, SourceId::Color(_))
                || self.captures.borrow().contains_key(&element.source);
            let item_id = element.id;
            let drag_viewer = viewer.clone();
            let measured_row_bounds = Rc::clone(&scene_row_bounds);
            let animation_offset = self
                .scene_item_offsets
                .get(&item_id)
                .copied()
                .unwrap_or_default();
            let reorder_transition = self.scene_reorder_transition;
            let drag = SceneDrag {
                item: item_id,
                label: label.clone(),
                grab_offset: Point::default(),
                list_bounds: Rc::clone(&scene_list_bounds),
            };
            div()
                .id(format!("scene-item-{item_id}"))
                .w_full()
                .min_h(px(32.0))
                .px_2()
                .flex()
                .items_center()
                .relative()
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
                .hover(|item| item.bg(rgb(theme::CONTROL_HOVER)))
                .cursor_move()
                .opacity(if self.scene_dragging_item == Some(item_id) {
                    0.0
                } else if source_ready {
                    1.0
                } else {
                    0.45
                })
                .child(
                    canvas(
                        move |bounds, _, _| {
                            measured_row_bounds.borrow_mut().insert(item_id, bounds);
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    svg()
                        .size(px(14.0))
                        .flex_none()
                        .path(GRIP_VERTICAL)
                        .text_color(rgb(theme::TEXT_DIM))
                        .mr_1(),
                )
                .child(div().flex_1().overflow_hidden().text_xs().child(label))
                .on_click(cx.listener(move |viewer, _, _, cx| {
                    viewer.select_scene_item(item_id, cx);
                }))
                .on_drag(drag, move |drag, grab_offset, _, cx| {
                    let _ = drag_viewer.update(cx, |viewer, cx| {
                        viewer.scene_dragging_item = Some(drag.item);
                        viewer.scene_drag_grab_offset = Some(grab_offset);
                        viewer.selected_item = Some(drag.item);
                        cx.notify();
                    });
                    let drag = SceneDrag {
                        item: drag.item,
                        label: drag.label.clone(),
                        grab_offset,
                        list_bounds: Rc::clone(&drag.list_bounds),
                    };
                    cx.new(|_| drag)
                })
                .can_drop(move |value, _, _| {
                    value
                        .downcast_ref::<SceneDrag>()
                        .is_some_and(|drag| drag.item != item_id)
                })
                .on_drag_move::<SceneDrag>(cx.listener(move |viewer, event, _, cx| {
                    viewer.drag_scene_item_over(event, cx);
                }))
                .on_drop(cx.listener(move |viewer, drag: &SceneDrag, window, cx| {
                    viewer.drop_scene_item(drag, item_id, window, cx);
                }))
                .with_animation(
                    format!("scene-item-{item_id}-reorder-{reorder_transition}"),
                    Animation::new(SCENE_ROW_ANIMATION_DURATION).with_easing(ease_out_quint()),
                    move |item, delta| item.top(px(animation_offset * (1.0 - delta))),
                )
        });
        div()
            .id("scene-items")
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .relative()
            .overflow_y_scroll()
            .child(
                canvas(
                    move |bounds, _, _| measured_bounds.set(Some(bounds)),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(rows)
    }

    #[allow(clippy::too_many_lines)]
    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let streaming = self.stream.is_some();
        let canvas_dimensions = self.stream_canvas.get().or(self.content_dimensions);
        let canvas_label = canvas_dimensions.map_or_else(
            || "Canvas: waiting for video".into(),
            |(width, height)| format!("Canvas: {width} x {height}"),
        );
        let mut sidebar = div()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_4()
            .px_3()
            .pt(TITLEBAR_SAFE_AREA)
            .pb_4()
            .bg(rgb(theme::PANEL_BACKGROUND))
            .border_r_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .id("stream-toggle")
                    .w_full()
                    .min_h(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .bg(if streaming {
                        rgb(theme::DANGER_BACKGROUND)
                    } else {
                        rgb(theme::PRIMARY_BACKGROUND)
                    })
                    .border_1()
                    .border_color(if streaming {
                        rgb(theme::DANGER_BACKGROUND)
                    } else {
                        rgb(theme::PRIMARY_BACKGROUND)
                    })
                    .active(|button| button.opacity(0.72))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if streaming {
                                rgb(theme::DANGER_TEXT)
                            } else {
                                rgb(theme::PRIMARY_TEXT)
                            })
                            .child(if streaming {
                                "Stop Stream"
                            } else {
                                "Start Stream"
                            }),
                    )
                    .on_click(cx.listener(|viewer, _, _, cx| viewer.toggle_stream(cx))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(canvas_label),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .child(if streaming {
                        "Canvas size locked while streaming"
                    } else {
                        "Default canvas size"
                    }),
            )
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Sources"),
            )
            .child(self.source_picker(cx))
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Scene"),
            )
            .child(self.scene_items(cx));
        if let Some(error) = self.control_error.clone() {
            sidebar = sidebar.child(
                div()
                    .text_sm()
                    .text_color(rgb(theme::ERROR_TEXT))
                    .child(error),
            );
        }
        sidebar
    }
}

impl Render for FrameViewer {
    #[allow(clippy::arithmetic_side_effects, clippy::too_many_lines)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut preview = div()
            .flex_1()
            .h_full()
            .relative()
            .bg(rgb(theme::CANVAS_BACKGROUND))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_drag))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_drag))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_drag));
        preview = if self.corner_handle_hovered {
            preview.cursor_crosshair()
        } else {
            match self.hovered_handle {
                Some(ResizeHandle::Left | ResizeHandle::Right) => preview.cursor_ew_resize(),
                Some(ResizeHandle::Top | ResizeHandle::Bottom) => preview.cursor_ns_resize(),
                Some(ResizeHandle::TopLeft | ResizeHandle::BottomRight) => {
                    preview.cursor_nwse_resize()
                }
                Some(ResizeHandle::TopRight | ResizeHandle::BottomLeft) => {
                    preview.cursor_nesw_resize()
                }
                None => preview,
            }
        };
        if let Some(image) = self.image.clone() {
            preview = preview.child(
                surface(image)
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
        }
        if let Some(selected_item) = self.selected_item
            && let Some(dimensions) = self.content_dimensions
            && let Some(transform) = self.rendered_transform(selected_item)
        {
            let guides = self.snap_guides;
            preview = preview.child(
                canvas(
                    move |bounds, _, _| {
                        let frame = content_frame_bounds(bounds, dimensions)?;
                        Some((
                            transform_bounds(frame, transform),
                            item_corner_radius(frame, dimensions, transform),
                            frame,
                        ))
                    },
                    move |_, selection, window, _| {
                        if let Some((bounds, radius, frame)) = selection {
                            if let Some(x) = guides.x {
                                let guide_x = frame.origin.x + frame.size.width * x;
                                window.paint_quad(quad(
                                    Bounds::new(
                                        Point::new(guide_x - px(0.5), frame.origin.y),
                                        size(px(1.0), frame.size.height),
                                    ),
                                    Pixels::ZERO,
                                    rgb(theme::SELECTION),
                                    Pixels::ZERO,
                                    rgb(theme::SELECTION),
                                    BorderStyle::default(),
                                ));
                            }
                            if let Some(y) = guides.y {
                                let guide_y = frame.origin.y + frame.size.height * y;
                                window.paint_quad(quad(
                                    Bounds::new(
                                        Point::new(frame.origin.x, guide_y - px(0.5)),
                                        size(frame.size.width, px(1.0)),
                                    ),
                                    Pixels::ZERO,
                                    rgb(theme::SELECTION),
                                    Pixels::ZERO,
                                    rgb(theme::SELECTION),
                                    BorderStyle::default(),
                                ));
                            }
                            window.paint_quad(
                                outline(bounds, rgb(theme::SELECTION), BorderStyle::default())
                                    .corner_radii(radius)
                                    .border_widths(px(2.0)),
                            );
                            for (_, center) in resize_handles(bounds) {
                                window.paint_quad(quad(
                                    resize_handle_bounds(center),
                                    px(2.0),
                                    rgb(theme::TEXT),
                                    px(1.0),
                                    rgb(theme::SELECTION),
                                    BorderStyle::default(),
                                ));
                            }
                            window.paint_quad(quad(
                                corner_radius_handle_bounds(bounds, radius),
                                px(5.0),
                                rgb(theme::SELECTION),
                                px(1.0),
                                rgb(theme::TEXT),
                                BorderStyle::default(),
                            ));
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        }
        div()
            .size_full()
            .flex()
            .bg(rgb(theme::APP_BACKGROUND))
            .text_color(rgb(theme::TEXT))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::blur_inputs))
            .on_mouse_move(cx.listener(Self::drag_inset))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_drag))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_drag))
            .child(self.sidebar(cx))
            .child(preview)
            .child(self.inspector(window, cx))
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn view(args: &StreamArgs) -> Result<(), Box<dyn Error>> {
    let content = shareable_content()?;
    let targets = capture_targets(&content)?;
    let selected_target = selected_target_index(&targets, &content, args)?;
    let options = CaptureOptions::from(args);
    let (event_sender, event_receiver) = async_channel::unbounded();
    let frame_hub = Arc::new(FrameHub {
        frames: Mutex::new(HashMap::new()),
        camera_statuses: Mutex::new(HashMap::new()),
        removed_sources: Mutex::new(Vec::new()),
        render_pending: AtomicBool::new(false),
        sender: event_sender.clone(),
    });
    let composition_hub = Arc::new(CompositionHub {
        latest: Mutex::new(None),
        event_pending: AtomicBool::new(false),
        sender: event_sender.clone(),
    });
    let target = targets
        .get(selected_target)
        .cloned()
        .ok_or(CaptureError::NoDisplay)?;
    let initial_source = target.id();
    let initial_generation = 1;
    let initial_capturer = build_capturer(&target, options, initial_generation, &frame_hub)?;
    initial_capturer.start()?;
    let captures = Rc::new(RefCell::new(HashMap::from([(
        initial_source,
        CaptureResource {
            capturer: initial_capturer,
            generation: initial_generation,
        },
    )])));
    let mut initial_scene = Scene::new();
    initial_scene.add(initial_source, full_canvas_layout());
    let selected_item = initial_scene.add(initial_source, inset_layout());
    let scene = Rc::new(RefCell::new(initial_scene));
    let compositor_scene = Rc::clone(&scene);
    let colors = Rc::new(RefCell::new(HashMap::new()));
    let viewer_colors = Rc::clone(&colors);
    let compositor_colors = Rc::clone(&colors);
    let stream_canvas = Rc::new(Cell::new(None));
    let compositor_canvas = Rc::clone(&stream_canvas);
    let stream_config = args.rtmp_url.clone().map(|url| RtmpConfig {
        url,
        fps: args.fps,
        bitrate: args.bitrate,
    });
    let (stream_error_sender, stream_error_receiver) = async_channel::unbounded();

    let viewer_captures = Rc::clone(&captures);
    let final_captures = Rc::clone(&captures);
    let viewer_frame_hub = Arc::clone(&frame_hub);
    let compositor_frame_hub = Arc::clone(&frame_hub);
    let render_queue = Arc::new(RenderQueue::default());
    let worker_render_queue = Arc::clone(&render_queue);
    let worker_composition_hub = Arc::clone(&composition_hub);
    let compositor_sender = event_sender;
    std::thread::Builder::new()
        .name("blip-studio-compositor".into())
        .spawn(move || {
            run_compositor(
                &worker_render_queue,
                &worker_composition_hub,
                &compositor_sender,
            );
        })?;
    let viewer_error = Arc::new(Mutex::new(None));
    let app_error = Arc::clone(&viewer_error);
    gpui_platform::application()
        .with_assets(StudioAssets)
        .run(move |cx: &mut App| {
            NumericInput::bind_keys(cx);
            #[cfg(target_os = "macos")]
            {
                cx.on_action(|_: &Quit, cx| cx.quit());
                cx.bind_keys([
                    KeyBinding::new("cmd-q", Quit, None),
                    KeyBinding::new("delete", DeleteSelected, None),
                    KeyBinding::new("backspace", DeleteSelected, None),
                    KeyBinding::new("left", MoveLeft, None),
                    KeyBinding::new("right", MoveRight, None),
                    KeyBinding::new("up", MoveUp, None),
                    KeyBinding::new("down", MoveDown, None),
                ]);
            }

            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let viewer = cx.new(|cx| {
                let transform_inputs = TransformInputs::new(cx);
                FrameViewer {
                    image: None,
                    content_dimensions: None,
                    scene,
                    rendered_elements: Vec::new(),
                    selected_item: Some(selected_item),
                    drag_operation: None,
                    snap_guides: SnapGuides::default(),
                    hovered_handle: None,
                    corner_handle_hovered: false,
                    transform_inputs,
                    focused_transform_inputs: HashSet::new(),
                    source_menu_open: false,
                    source_menu_visible: false,
                    source_menu_transition: 0,
                    scene_dragging_item: None,
                    scene_drag_grab_offset: None,
                    scene_item_offsets: HashMap::new(),
                    scene_reorder_transition: 0,
                    scene_item_animation_started: None,
                    scene_list_bounds: Rc::new(Cell::new(None)),
                    scene_row_bounds: Rc::new(RefCell::new(HashMap::new())),
                    targets,
                    options,
                    captures: viewer_captures,
                    pending_captures: HashMap::new(),
                    failed_captures: HashSet::new(),
                    next_capture_generation: 2,
                    colors: viewer_colors,
                    next_color_source_id: 1,
                    frame_hub: viewer_frame_hub,
                    stream: None,
                    stream_config,
                    stream_canvas,
                    stream_generation: 0,
                    stream_error_sender,
                    control_error: None,
                }
            });
            let action_viewer = viewer.clone();
            cx.on_action(move |_: &DeleteSelected, cx| {
                action_viewer.update(cx, FrameViewer::delete_selected);
            });
            let action_viewer = viewer.clone();
            cx.on_action(move |_: &MoveLeft, cx| {
                action_viewer.update(cx, |viewer, cx| viewer.move_selected([-1.0, 0.0], cx));
            });
            let action_viewer = viewer.clone();
            cx.on_action(move |_: &MoveRight, cx| {
                action_viewer.update(cx, |viewer, cx| viewer.move_selected([1.0, 0.0], cx));
            });
            let action_viewer = viewer.clone();
            cx.on_action(move |_: &MoveUp, cx| {
                action_viewer.update(cx, |viewer, cx| viewer.move_selected([0.0, -1.0], cx));
            });
            let action_viewer = viewer.clone();
            cx.on_action(move |_: &MoveDown, cx| {
                action_viewer.update(cx, |viewer, cx| viewer.move_selected([0.0, 1.0], cx));
            });
            let event_viewer = viewer.clone();
            let event_error = Arc::clone(&app_error);
            let event_composition_hub = Arc::clone(&composition_hub);
            cx.spawn(async move |cx| {
                let mut source_frames = HashMap::new();
                while let Ok(event) = event_receiver.recv().await {
                    match event {
                        ViewerEvent::RenderReady => {
                            for (source, frame) in compositor_frame_hub.drain() {
                                match source_frame(&frame) {
                                    Ok(frame) => {
                                        source_frames.insert(source, frame);
                                    }
                                    Err(error) => {
                                        if let Ok(mut viewer_error) = event_error.lock() {
                                            *viewer_error = Some(error.to_string());
                                        }
                                        cx.update(|cx| cx.quit());
                                        return;
                                    }
                                }
                            }
                            for source in compositor_frame_hub.drain_removed_sources() {
                                source_frames.remove(&source);
                            }
                            render_queue.submit(RenderRequest {
                                elements: compositor_scene.borrow().elements.clone(),
                                frames: source_frames.clone(),
                                colors: compositor_colors.borrow().clone(),
                                locked_dimensions: compositor_canvas.get(),
                            });
                        }
                        ViewerEvent::CompositionReady => {
                            if let Some(frame) = event_composition_hub.take() {
                                event_viewer.update(cx, |viewer, cx| {
                                    viewer.show_frame(
                                        frame.image,
                                        frame.dimensions,
                                        frame.elements,
                                        cx,
                                    );
                                });
                            }
                        }
                        ViewerEvent::CompositionFailed(message) => {
                            if let Ok(mut viewer_error) = event_error.lock() {
                                *viewer_error = Some(message);
                            }
                            cx.update(|cx| cx.quit());
                            return;
                        }
                        ViewerEvent::Error(source, generation, message) => {
                            event_viewer.update(cx, |viewer, cx| {
                                viewer.capture_failed(source, generation, &message, cx);
                            });
                        }
                    }
                }
            })
            .detach();
            let stream_event_viewer = viewer.clone();
            cx.spawn(async move |cx| {
                while let Ok((generation, message)) = stream_error_receiver.recv().await {
                    stream_event_viewer.update(cx, |viewer, cx| {
                        viewer.stream_failed(generation, message, cx);
                    });
                }
            })
            .detach();

            let window_options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("blip-studio".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(window_error) = cx.open_window(window_options, |window, _| {
                window.set_window_title("blip-studio");
                viewer
            }) {
                if let Ok(mut error) = app_error.lock() {
                    *error = Some(format!("failed to open viewer window: {window_error}"));
                }
                cx.quit();
                return;
            }
            cx.activate(true);
        });

    let capture_error = viewer_error.lock().ok().and_then(|mut error| error.take());
    let stop_result = stop_captures(&final_captures);
    if let Some(message) = capture_error {
        return Err(CaptureError::Framework(message).into());
    }
    stop_result?;
    Ok(())
}

fn shareable_content() -> Result<ShareableContent, CaptureError> {
    if !blip_sck::has_permission() {
        let _ = blip_sck::request_permission();
        return Err(CaptureError::PermissionDenied);
    }
    ShareableContent::current(CAPTURE_TIMEOUT)
}

fn source_frame(frame: &CapturedFrame) -> anyhow::Result<SourceFrame> {
    match frame {
        CapturedFrame::Screen(frame) => {
            let geometry = frame.geometry();
            let content_rect = geometry.map(|geometry| geometry.content_rect);
            let dimensions = if let Some(rect) = content_rect {
                (rect.width, rect.height)
            } else {
                frame_dimensions(frame.width(), frame.height())?
            };
            Ok(SourceFrame {
                pixel_buffer: retain_screen_pixel_buffer(frame),
                content_rect,
                dimensions,
            })
        }
        CapturedFrame::Camera(frame) => Ok(SourceFrame {
            pixel_buffer: retain_camera_pixel_buffer(frame),
            content_rect: None,
            dimensions: frame_dimensions(frame.width(), frame.height())?,
        }),
    }
}

fn run_compositor(
    render_queue: &RenderQueue,
    composition_hub: &CompositionHub,
    sender: &Sender<ViewerEvent>,
) {
    set_interactive_thread_qos();
    let mut compositor = match FrameCompositor::new() {
        Ok(compositor) => compositor,
        Err(error) => {
            let _ = sender.try_send(ViewerEvent::CompositionFailed(error.to_string()));
            return;
        }
    };
    let mut last_render = None;

    loop {
        let mut request = render_queue.take();
        if let Some(last_render) = last_render {
            let elapsed = Instant::now().saturating_duration_since(last_render);
            if let Some(delay) = RENDER_INTERVAL.checked_sub(elapsed) {
                std::thread::sleep(delay);
                if let Some(latest) = render_queue.take_pending() {
                    request = latest;
                }
            }
        }
        let render_started = Instant::now();
        let composed = compose_scene(
            &mut compositor,
            &request.elements,
            &request.frames,
            &request.colors,
            request.locked_dimensions,
        );
        last_render = Some(render_started);
        match composed {
            Ok((image, dimensions, elements)) => composition_hub.submit(ComposedFrame {
                image,
                dimensions,
                elements,
            }),
            Err(error) => {
                if sender
                    .try_send(ViewerEvent::CompositionFailed(error.to_string()))
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

fn set_interactive_thread_qos() {
    // SAFETY: This updates only the calling compositor thread's scheduling class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

fn frame_dimensions(width: usize, height: usize) -> anyhow::Result<(f64, f64)> {
    Ok((
        f64::from(u32::try_from(width).context("frame width exceeds u32")?),
        f64::from(u32::try_from(height).context("frame height exceeds u32")?),
    ))
}

fn compose_scene(
    compositor: &mut FrameCompositor,
    elements: &[SceneElement],
    frames: &HashMap<SourceId, SourceFrame>,
    colors: &HashMap<SourceId, ColorSource>,
    locked_dimensions: Option<(usize, usize)>,
) -> anyhow::Result<(CVPixelBuffer, (usize, usize), Vec<RenderedElement>)> {
    let canvas_dimensions = locked_dimensions.unwrap_or(DEFAULT_CANVAS_DIMENSIONS);
    let output_dimensions = locked_dimensions.unwrap_or(PREVIEW_CANVAS_DIMENSIONS);
    let mut source_ids = Vec::new();
    let mut source_indices = HashMap::new();
    for element in elements {
        if frames.contains_key(&element.source) && !source_indices.contains_key(&element.source) {
            source_indices.insert(element.source, source_ids.len());
            source_ids.push(element.source);
        }
    }
    let sources = source_ids
        .iter()
        .filter_map(|source| frames.get(source))
        .map(|frame| CompositorSource {
            pixel_buffer: &frame.pixel_buffer,
            content_rect: frame.content_rect,
        })
        .collect::<Vec<_>>();
    let mut items = Vec::new();
    let mut rendered = Vec::new();
    for element in elements {
        let (content, source_dimensions) = match element.source {
            SourceId::Color(_) => {
                let Some(color) = colors.get(&element.source) else {
                    continue;
                };
                (
                    CompositorItemContent::Color(normalized_color(color.color)),
                    (
                        f64::from(
                            u32::try_from(canvas_dimensions.0)
                                .context("canvas width exceeds u32")?,
                        ),
                        f64::from(
                            u32::try_from(canvas_dimensions.1)
                                .context("canvas height exceeds u32")?,
                        ),
                    ),
                )
            }
            SourceId::Display(_) | SourceId::Window(_) | SourceId::Camera(_) => {
                let (Some(source_index), Some(frame)) = (
                    source_indices.get(&element.source),
                    frames.get(&element.source),
                ) else {
                    continue;
                };
                (
                    CompositorItemContent::Source(*source_index),
                    frame.dimensions,
                )
            }
        };
        let transform = element_transform(element.layout, source_dimensions, canvas_dimensions)?;
        let corner_scale =
            f64::from(u32::try_from(output_dimensions.0).context("preview width exceeds u32")?)
                / f64::from(
                    u32::try_from(canvas_dimensions.0).context("canvas width exceeds u32")?,
                );
        #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
        let compositor_transform = transform
            .with_corner_radius((f64::from(transform.corner_radius) * corner_scale) as f32);
        items.push(CompositorItem {
            content,
            transform: compositor_transform,
        });
        rendered.push(RenderedElement {
            id: element.id,
            transform,
        });
    }
    let image = compositor.render(&sources, &items, output_dimensions)?;
    Ok((image, canvas_dimensions, rendered))
}

fn normalized_color([red, green, blue]: [u8; 3]) -> [f32; 4] {
    [
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        1.0,
    ]
}

fn element_transform(
    layout: ItemLayout,
    source_dimensions: (f64, f64),
    canvas_dimensions: (usize, usize),
) -> anyhow::Result<ItemTransform> {
    let canvas_dimensions = (
        f64::from(u32::try_from(canvas_dimensions.0).context("canvas width exceeds u32")?),
        f64::from(u32::try_from(canvas_dimensions.1).context("canvas height exceeds u32")?),
    );
    Ok(ItemTransform::new(
        layout.center,
        scaled_size(
            aspect_fit_size(layout.base_size, source_dimensions, canvas_dimensions),
            layout.scale,
        ),
    )
    .with_corner_radius(layout.corner_radius))
}

fn scaled_size(size: [f32; 2], scale: f32) -> [f32; 2] {
    [size[0] * scale, size[1] * scale]
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
fn aspect_fit_size(
    maximum_size: [f32; 2],
    (source_width, source_height): (f64, f64),
    (canvas_width, canvas_height): (f64, f64),
) -> [f32; 2] {
    let source_aspect = source_width / source_height;
    let canvas_aspect = canvas_width / canvas_height;
    let box_aspect = f64::from(maximum_size[0]) * canvas_aspect / f64::from(maximum_size[1]);
    if source_aspect > box_aspect {
        [
            maximum_size[0],
            (f64::from(maximum_size[0]) * canvas_aspect / source_aspect) as f32,
        ]
    } else {
        [
            (f64::from(maximum_size[1]) * source_aspect / canvas_aspect) as f32,
            maximum_size[1],
        ]
    }
}

fn stop_captures(captures: &RefCell<HashMap<SourceId, CaptureResource>>) -> anyhow::Result<()> {
    let mut first_error = None;
    for resource in captures.borrow().values() {
        if let Err(error) = resource.capturer.stop()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn even_dimensions((width, height): (usize, usize)) -> (usize, usize) {
    (width & !1, height & !1)
}

fn should_restart_capture(
    source_in_use: bool,
    current_generation: Option<u64>,
    failed_generation: u64,
) -> bool {
    source_in_use && current_generation == Some(failed_generation)
}

fn should_finish_pending_capture(
    pending_generation: Option<u64>,
    completed_generation: u64,
    source_in_use: bool,
) -> bool {
    source_in_use && pending_generation == Some(completed_generation)
}

fn capture_targets(content: &ShareableContent) -> Result<Vec<CaptureTarget>, Box<dyn Error>> {
    let targets = content
        .displays()
        .into_iter()
        .map(CaptureTarget::Display)
        .chain(
            content
                .application_windows()
                .into_iter()
                .map(CaptureTarget::Window),
        )
        .chain(
            blip_avfoundation::list_video_devices()?
                .into_iter()
                .map(CaptureTarget::Camera),
        )
        .collect();
    Ok(targets)
}

fn selected_target_index(
    targets: &[CaptureTarget],
    content: &ShareableContent,
    args: &StreamArgs,
) -> Result<usize, Box<dyn Error>> {
    let target_index = if let Some(display_id) = args.display {
        targets.iter().position(
            |target| matches!(target, CaptureTarget::Display(display) if display.id() == display_id),
        )
    } else if let Some(window_id) = args.window {
        targets.iter().position(
            |target| matches!(target, CaptureTarget::Window(window) if window.id() == window_id),
        )
    } else {
        let main_display_id = content.main_display().ok_or(CaptureError::NoDisplay)?.id();
        targets.iter().position(
            |target| matches!(target, CaptureTarget::Display(display) if display.id() == main_display_id),
        )
    };
    target_index.ok_or_else(|| "requested capture target is not available".into())
}

fn build_capturer(
    target: &CaptureTarget,
    options: CaptureOptions,
    generation: u64,
    frame_hub: &Arc<FrameHub>,
) -> Result<CaptureBackend, CaptureError> {
    let source = target.id();
    let frame_hub = Arc::clone(frame_hub);
    let stop_hub = Arc::clone(&frame_hub);
    match target {
        CaptureTarget::Display(_) | CaptureTarget::Window(_) => {
            let filter = target.filter().ok_or_else(|| {
                CaptureError::InvalidConfiguration("missing capture filter".into())
            })?;
            Capturer::builder(filter, stream_config(options))?
                .with_timeout(CAPTURE_TIMEOUT)
                .with_video_frame_callback(move |frame| {
                    frame_hub.submit(source, CapturedFrame::Screen(frame));
                })
                .with_stop_callback(move |error| {
                    stop_hub.report_error(
                        source,
                        generation,
                        error.localizedDescription().to_string(),
                    );
                })
                .build()
                .map(CaptureBackend::Screen)
        }
        CaptureTarget::Camera(camera) => {
            let drop_hub = Arc::clone(&frame_hub);
            CameraCapturer::new_with_drop_callback(
                camera,
                PREFERRED_CAMERA_FPS,
                move |frame| frame_hub.submit(source, CapturedFrame::Camera(frame)),
                move || drop_hub.report_camera_drop(source),
            )
            .map(CaptureBackend::Camera)
            .map_err(|error| CaptureError::Framework(error.to_string()))
        }
    }
}

fn stream_config(options: CaptureOptions) -> StreamConfigBuilder {
    StreamConfig::builder()
        .with_fps(options.fps)
        .with_cursor(options.cursor)
        .with_queue_depth(2)
        .with_pixel_format(PixelFormat::Bgra)
}

#[allow(clippy::arithmetic_side_effects)]
fn preview_bounds(window: &Window) -> Bounds<Pixels> {
    let viewport = window.viewport_size();
    Bounds::new(
        Point::new(SIDEBAR_WIDTH, Pixels::ZERO),
        size(
            (viewport.width - SIDEBAR_WIDTH - INSPECTOR_WIDTH).max(Pixels::ZERO),
            viewport.height,
        ),
    )
}

fn preview_frame_bounds(
    window: &Window,
    (content_width, content_height): (usize, usize),
) -> Option<Bounds<Pixels>> {
    content_frame_bounds(preview_bounds(window), (content_width, content_height))
}

#[allow(clippy::arithmetic_side_effects)]
fn transform_bounds(frame: Bounds<Pixels>, transform: ItemTransform) -> Bounds<Pixels> {
    let [center_x, center_y] = transform.center;
    let [width, height] = transform.size;
    let inset_size = size(frame.size.width * width, frame.size.height * height);
    Bounds::new(
        Point::new(
            frame.origin.x + frame.size.width * center_x - inset_size.width * 0.5,
            frame.origin.y + frame.size.height * center_y - inset_size.height * 0.5,
        ),
        inset_size,
    )
}

fn resize_operation(
    item: ElementId,
    handle: ResizeHandle,
    transform: ItemTransform,
    initial_scale: f32,
) -> DragOperation {
    let [horizontal, vertical] = handle.axes();
    let anchor = [
        transform.center[0] - horizontal * transform.size[0] * 0.5,
        transform.center[1] - vertical * transform.size[1] * 0.5,
    ];
    DragOperation::Resize {
        item,
        handle,
        initial_transform: transform,
        initial_scale,
        anchor,
    }
}

fn resized_item(
    handle: ResizeHandle,
    initial: ItemTransform,
    anchor: [f32; 2],
    pointer: [f32; 2],
    minimum_ratio: f32,
) -> ([f32; 2], f32) {
    let [horizontal, vertical] = handle.axes();
    let resize_vector = [horizontal * initial.size[0], vertical * initial.size[1]];
    let pointer_vector = [pointer[0] - anchor[0], pointer[1] - anchor[1]];
    let denominator = resize_vector[0] * resize_vector[0] + resize_vector[1] * resize_vector[1];
    let ratio = ((pointer_vector[0] * resize_vector[0] + pointer_vector[1] * resize_vector[1])
        / denominator)
        .max(minimum_ratio);
    let center = [
        if horizontal == 0.0 {
            initial.center[0]
        } else {
            anchor[0] + resize_vector[0] * ratio * 0.5
        },
        if vertical == 0.0 {
            initial.center[1]
        } else {
            anchor[1] + resize_vector[1] * ratio * 0.5
        },
    ];
    (center, ratio)
}

fn resized_item_with_border_snap(
    handle: ResizeHandle,
    initial: ItemTransform,
    anchor: [f32; 2],
    pointer: [f32; 2],
    minimum_ratio: f32,
    threshold: [f32; 2],
) -> ([f32; 2], f32, SnapGuides) {
    let (_, ratio) = resized_item(handle, initial, anchor, pointer, minimum_ratio);
    let [horizontal, vertical] = handle.axes();
    let mut snapped_ratio = ratio;
    let mut smallest_adjustment = f32::INFINITY;
    let mut guides = SnapGuides::default();
    for (axis, anchor, direction, size, threshold) in [
        (true, anchor[0], horizontal, initial.size[0], threshold[0]),
        (false, anchor[1], vertical, initial.size[1], threshold[1]),
    ] {
        if direction == 0.0 {
            continue;
        }
        let border = if direction < 0.0 { 0.0 } else { 1.0 };
        let moving_edge = anchor + direction * size * ratio;
        if (moving_edge - border).abs() > threshold {
            continue;
        }
        let candidate = ((border - anchor) / (direction * size)).max(minimum_ratio);
        let adjustment = (candidate - ratio).abs();
        if adjustment < smallest_adjustment {
            smallest_adjustment = adjustment;
            snapped_ratio = candidate;
            guides = if axis {
                SnapGuides {
                    x: Some(border),
                    y: None,
                }
            } else {
                SnapGuides {
                    x: None,
                    y: Some(border),
                }
            };
        }
    }
    let (center, ratio) = resized_item(
        handle,
        initial,
        anchor,
        [
            anchor[0] + horizontal * initial.size[0] * snapped_ratio,
            anchor[1] + vertical * initial.size[1] * snapped_ratio,
        ],
        minimum_ratio,
    );
    (center, ratio, guides)
}

fn snapped_move_center(
    center: [f32; 2],
    size: [f32; 2],
    threshold: [f32; 2],
) -> ([f32; 2], SnapGuides) {
    let (x, x_guide) = snapped_move_axis(center[0], size[0], threshold[0]);
    let (y, y_guide) = snapped_move_axis(center[1], size[1], threshold[1]);
    (
        [x, y],
        SnapGuides {
            x: x_guide,
            y: y_guide,
        },
    )
}

fn snapped_move_axis(center: f32, size: f32, threshold: f32) -> (f32, Option<f32>) {
    let half_size = size * 0.5;
    let mut snapped = center;
    let mut guide = None;
    let mut smallest_adjustment = f32::INFINITY;
    for (position, target) in [
        (center - half_size, 0.0),
        (center, 0.5),
        (center + half_size, 1.0),
    ] {
        let adjustment = target - position;
        if adjustment.abs() <= threshold && adjustment.abs() < smallest_adjustment {
            smallest_adjustment = adjustment.abs();
            snapped = center + adjustment;
            guide = Some(target);
        }
    }
    (snapped, guide)
}

fn normalized_snap_threshold(frame: Bounds<Pixels>) -> [f32; 2] {
    [
        SNAP_THRESHOLD_PX / frame.size.width.as_f32(),
        SNAP_THRESHOLD_PX / frame.size.height.as_f32(),
    ]
}

#[allow(clippy::arithmetic_side_effects)]
fn resize_handles(bounds: Bounds<Pixels>) -> [(ResizeHandle, Point<Pixels>); 8] {
    let left = bounds.origin.x;
    let top = bounds.origin.y;
    let center_x = left + bounds.size.width * 0.5;
    let center_y = top + bounds.size.height * 0.5;
    let right = left + bounds.size.width;
    let bottom = top + bounds.size.height;
    [
        (ResizeHandle::TopLeft, Point::new(left, top)),
        (ResizeHandle::Top, Point::new(center_x, top)),
        (ResizeHandle::TopRight, Point::new(right, top)),
        (ResizeHandle::Right, Point::new(right, center_y)),
        (ResizeHandle::BottomRight, Point::new(right, bottom)),
        (ResizeHandle::Bottom, Point::new(center_x, bottom)),
        (ResizeHandle::BottomLeft, Point::new(left, bottom)),
        (ResizeHandle::Left, Point::new(left, center_y)),
    ]
}

fn resize_handle_at(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<ResizeHandle> {
    resize_handles(bounds)
        .into_iter()
        .find(|(_, center)| resize_handle_bounds(*center).contains(&position))
        .map(|(handle, _)| handle)
}

#[allow(clippy::arithmetic_side_effects)]
fn resize_handle_bounds(center: Point<Pixels>) -> Bounds<Pixels> {
    let extent = px(10.0);
    Bounds::new(
        Point::new(center.x - extent * 0.5, center.y - extent * 0.5),
        size(extent, extent),
    )
}

#[allow(clippy::arithmetic_side_effects)]
fn corner_radius_handle_bounds(bounds: Bounds<Pixels>, radius: Pixels) -> Bounds<Pixels> {
    let maximum_inset = bounds.size.width.min(bounds.size.height).as_f32() * 0.5;
    let fixed_inset = CORNER_HANDLE_INSET.as_f32();
    let inset = if maximum_inset <= fixed_inset {
        maximum_inset
    } else {
        fixed_inset
            + (radius.as_f32() / maximum_inset).clamp(0.0, 1.0) * (maximum_inset - fixed_inset)
    };
    let inset = px(inset);
    let center = Point::new(
        bounds.origin.x + bounds.size.width - inset,
        bounds.origin.y + inset,
    );
    let extent = px(10.0);
    Bounds::new(
        Point::new(center.x - extent * 0.5, center.y - extent * 0.5),
        size(extent, extent),
    )
}

#[allow(clippy::arithmetic_side_effects)]
fn corner_radius_from_position(
    bounds: Bounds<Pixels>,
    preview_scale: f32,
    position: Point<Pixels>,
) -> f32 {
    let maximum_inset = bounds.size.width.min(bounds.size.height).as_f32() * 0.5;
    let fixed_inset = CORNER_HANDLE_INSET.as_f32();
    if maximum_inset <= fixed_inset {
        return 0.0;
    }
    let right = bounds.origin.x + bounds.size.width;
    let pointer_inset =
        ((right - position.x).as_f32() + (position.y - bounds.origin.y).as_f32()) * 0.5;
    let ratio = ((pointer_inset - fixed_inset) / (maximum_inset - fixed_inset)).clamp(0.0, 1.0);
    maximum_inset * ratio / preview_scale
}

fn content_frame_bounds(
    container: Bounds<Pixels>,
    (content_width, content_height): (usize, usize),
) -> Option<Bounds<Pixels>> {
    let width = i32::try_from(content_width).ok()?;
    let height = i32::try_from(content_height).ok()?;
    Some(ObjectFit::Contain.get_bounds(
        container,
        size(DevicePixels::from(width), DevicePixels::from(height)),
    ))
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
fn item_corner_radius(
    frame: Bounds<Pixels>,
    (content_width, content_height): (usize, usize),
    transform: ItemTransform,
) -> Pixels {
    let canvas_size = [content_width as f32, content_height as f32];
    let preview_scale = frame.size.width / px(canvas_size[0]);
    px(transform.clamped_corner_radius(canvas_size) * preview_scale)
}

#[allow(clippy::arithmetic_side_effects)]
fn normalized_position(position: Pixels, origin: Pixels, extent: Pixels) -> f32 {
    ((position - origin) / extent).clamp(0.0, 1.0)
}

#[allow(clippy::arithmetic_side_effects)]
fn normalized_position_unclamped(position: Pixels, origin: Pixels, extent: Pixels) -> f32 {
    (position - origin) / extent
}

#[allow(clippy::arithmetic_side_effects)]
fn nudged_center(center: [f32; 2], delta: [f32; 2], canvas: [f32; 2]) -> [f32; 2] {
    [
        (center[0] + delta[0] / canvas[0]).clamp(0.0, 1.0),
        (center[1] + delta[1] / canvas[1]).clamp(0.0, 1.0),
    ]
}

fn color_rgb([red, green, blue]: [u8; 3]) -> u32 {
    u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
}

#[allow(clippy::arithmetic_side_effects)]
fn clamped_scene_drag_origin(root: Point<Pixels>, bounds: Bounds<Pixels>) -> Point<Pixels> {
    Point::new(
        bounds.origin.x,
        root.y.clamp(
            bounds.origin.y,
            (bounds.origin.y + bounds.size.height - px(32.0)).max(bounds.origin.y),
        ),
    )
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn scene_drag_render_index(
    pointer_y: Pixels,
    grab_offset_y: Pixels,
    bounds: Bounds<Pixels>,
    item_count: usize,
) -> Option<usize> {
    let maximum_display_index = item_count.checked_sub(1)?;
    let root = Point::new(bounds.origin.x, pointer_y - grab_offset_y);
    let origin = clamped_scene_drag_origin(root, bounds);
    let center = origin.y + px(16.0);
    let display_index = (((center - bounds.origin.y).as_f32() / SCENE_ROW_STRIDE).floor() as usize)
        .min(maximum_display_index);
    maximum_display_index.checked_sub(display_index)
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
fn scene_item_origin(
    scene: &Scene,
    item: ElementId,
    bounds: Bounds<Pixels>,
) -> Option<Point<Pixels>> {
    let render_index = scene
        .elements
        .iter()
        .position(|element| element.id == item)?;
    let display_index = scene
        .elements
        .len()
        .checked_sub(render_index.saturating_add(1))?;
    Some(Point::new(
        bounds.origin.x,
        bounds.origin.y + px(display_index as f32 * SCENE_ROW_STRIDE),
    ))
}

#[allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]
fn scene_animation_remaining(elapsed: Duration) -> f32 {
    let progress =
        (elapsed.as_secs_f32() / SCENE_ROW_ANIMATION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    (1.0 - progress).powi(5)
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
fn scene_reorder_offsets(
    previous_order: &[ElementId],
    scene: &Scene,
    dragged: ElementId,
) -> HashMap<ElementId, f32> {
    let previous_indices = previous_order
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect::<HashMap<_, _>>();
    scene
        .elements
        .iter()
        .enumerate()
        .filter_map(|(new_index, element)| {
            if element.id == dragged {
                return None;
            }
            let old_index = *previous_indices.get(&element.id)?;
            let displacement = i32::try_from(new_index).ok()? - i32::try_from(old_index).ok()?;
            (displacement != 0).then_some((element.id, displacement as f32 * SCENE_ROW_STRIDE))
        })
        .collect()
}

fn retain_screen_pixel_buffer(frame: &VideoFrame) -> CVPixelBuffer {
    let pixel_buffer = ptr::from_ref(frame.image_buffer())
        .cast_mut()
        .cast::<core_video::buffer::__CVBuffer>();
    // SAFETY: Both bindings represent the same retained CoreVideo pixel-buffer object.
    // The get-rule constructor retains it before the `VideoFrame` can be dropped.
    unsafe { CVPixelBuffer::wrap_under_get_rule(pixel_buffer) }
}

fn retain_camera_pixel_buffer(frame: &CameraFrame) -> CVPixelBuffer {
    let pixel_buffer = ptr::from_ref(frame.image_buffer())
        .cast_mut()
        .cast::<core_video::buffer::__CVBuffer>();
    // SAFETY: Both bindings represent the same retained CoreVideo pixel-buffer object.
    // The get-rule constructor retains it before the `CameraFrame` can be dropped.
    unsafe { CVPixelBuffer::wrap_under_get_rule(pixel_buffer) }
}

fn camera_id(unique_id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    unique_id.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_fit_preserves_tall_source_ratio() {
        let size = aspect_fit_size([1.0, 1.0], (3456.0, 2234.0), (1920.0, 1080.0));
        let rendered_aspect = f64::from(size[0]) * 1920.0 / (f64::from(size[1]) * 1080.0);

        assert!((rendered_aspect - 3456.0 / 2234.0).abs() < 0.000_001);
        assert!((size[1] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn aspect_fit_preserves_wide_source_ratio_inside_overlay() {
        let size = aspect_fit_size([0.5, 0.5], (2560.0, 1080.0), (1920.0, 1080.0));
        let rendered_aspect = f64::from(size[0]) * 1920.0 / (f64::from(size[1]) * 1080.0);

        assert!((rendered_aspect - 2560.0 / 1080.0).abs() < 0.000_001);
        assert!((size[0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn edge_resize_keeps_the_opposite_edge_anchored() {
        let initial = ItemTransform::new([0.5, 0.5], [0.5, 0.25]);
        let anchor = [0.25, 0.5];
        let (center, ratio) = resized_item(
            ResizeHandle::Right,
            initial,
            anchor,
            [0.875, 0.5],
            MIN_ITEM_SCALE,
        );

        assert!((ratio - 1.25).abs() < f32::EPSILON);
        assert!((center[0] - 0.5625).abs() < f32::EPSILON);
        assert!((center[0] - initial.size[0] * ratio * 0.5 - anchor[0]).abs() < f32::EPSILON);
    }

    #[test]
    fn corner_resize_uses_uniform_scale_and_keeps_anchor_fixed() {
        let initial = ItemTransform::new([0.5, 0.5], [0.4, 0.2]);
        let anchor = [0.3, 0.4];
        let (center, ratio) = resized_item(
            ResizeHandle::BottomRight,
            initial,
            anchor,
            [1.1, 0.8],
            MIN_ITEM_SCALE,
        );
        let resized_size = scaled_size(initial.size, ratio);

        assert!((ratio - 2.0).abs() < 0.000_001);
        assert!((center[0] - resized_size[0] * 0.5 - anchor[0]).abs() < 0.000_001);
        assert!((center[1] - resized_size[1] * 0.5 - anchor[1]).abs() < 0.000_001);
        assert!((resized_size[0] / resized_size[1] - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn movement_snaps_item_edges_and_center_to_canvas() {
        let (leading_and_center, leading_guides) =
            snapped_move_center([0.104, 0.506], [0.2, 0.2], [0.01, 0.01]);
        let (unchanged, unchanged_guides) =
            snapped_move_center([0.85, 0.75], [0.2, 0.2], [0.01, 0.01]);
        let (trailing, trailing_guides) =
            snapped_move_center([0.896, 0.75], [0.2, 0.2], [0.01, 0.01]);

        assert!((leading_and_center[0] - 0.1).abs() < f32::EPSILON);
        assert!((leading_and_center[1] - 0.5).abs() < f32::EPSILON);
        assert!((unchanged[0] - 0.85).abs() < f32::EPSILON);
        assert!((unchanged[1] - 0.75).abs() < f32::EPSILON);
        assert!((trailing[0] - 0.9).abs() < f32::EPSILON);
        assert!((trailing[1] - 0.75).abs() < f32::EPSILON);
        assert!(leading_guides.x.is_some_and(|x| x.abs() < f32::EPSILON));
        assert!(
            leading_guides
                .y
                .is_some_and(|y| (y - 0.5).abs() < f32::EPSILON)
        );
        assert!(unchanged_guides.x.is_none());
        assert!(unchanged_guides.y.is_none());
        assert!(
            trailing_guides
                .x
                .is_some_and(|x| (x - 1.0).abs() < f32::EPSILON)
        );
        assert!(trailing_guides.y.is_none());
    }

    #[test]
    fn resizing_snaps_dragged_border_to_canvas() {
        let initial = ItemTransform::new([0.5, 0.5], [0.5, 0.25]);
        let anchor = [0.25, 0.5];
        let (center, ratio, guides) = resized_item_with_border_snap(
            ResizeHandle::Right,
            initial,
            anchor,
            [0.994, 0.5],
            0.1,
            [0.01, 0.01],
        );

        assert!((ratio - 1.5).abs() < f32::EPSILON);
        assert!((center[0] + initial.size[0] * ratio * 0.5 - 1.0).abs() < f32::EPSILON);
        assert!((center[1] - initial.center[1]).abs() < f32::EPSILON);
        assert!(guides.x.is_some_and(|x| (x - 1.0).abs() < f32::EPSILON));
        assert!(guides.y.is_none());
    }

    #[test]
    fn corner_radius_handle_maps_between_zero_and_maximum_radius() {
        let bounds = Bounds::new(Point::new(px(0.0), px(0.0)), size(px(200.0), px(100.0)));
        let preview_scale = 0.5;

        let zero =
            corner_radius_from_position(bounds, preview_scale, Point::new(px(184.0), px(16.0)));
        let half =
            corner_radius_from_position(bounds, preview_scale, Point::new(px(167.0), px(33.0)));
        let maximum =
            corner_radius_from_position(bounds, preview_scale, Point::new(px(150.0), px(50.0)));

        assert!(zero.abs() < f32::EPSILON);
        assert!((half - 50.0).abs() < 0.000_1);
        assert!((maximum - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn element_transform_carries_corner_radius() {
        let mut layout = inset_layout();
        layout.corner_radius = 36.0;

        let transform = element_transform(layout, (1920.0, 1080.0), (1920, 1080));

        assert!(transform.is_ok());
        if let Ok(transform) = transform {
            assert!((transform.corner_radius - 36.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn scene_allows_elements_to_share_one_source() {
        let source = SourceId::Display(1);
        let mut scene = Scene::new();

        let first = scene.add(source, full_canvas_layout());
        let second = scene.add(source, inset_layout());

        assert_ne!(first, second);
        assert_eq!(scene.elements.len(), 2);
        assert!(
            scene
                .elements
                .iter()
                .all(|element| element.source == source)
        );
    }

    #[test]
    fn color_sources_have_stable_ids_and_normalized_values() {
        let source = SourceId::Color(7);
        let mut scene = Scene::new();
        let item = scene.add(source, full_canvas_layout());
        let [red, green, blue, alpha] = normalized_color([255, 128, 0]);

        assert_eq!(
            scene.element(item).map(|element| element.source),
            Some(source)
        );
        assert!((red - 1.0).abs() < f32::EPSILON);
        assert!((green - 128.0 / 255.0).abs() < f32::EPSILON);
        assert!(blue.abs() < f32::EPSILON);
        assert!((alpha - 1.0).abs() < f32::EPSILON);
        assert_eq!(color_rgb([0x12, 0x34, 0x56]), 0x0012_3456);
    }

    #[test]
    fn camera_sources_have_stable_device_ids() {
        assert_eq!(camera_id("camera-a"), camera_id("camera-a"));
        assert_ne!(camera_id("camera-a"), camera_id("camera-b"));
    }

    #[test]
    fn camera_status_reports_observed_frame_rate() {
        let started = Instant::now();
        let mut status = CameraStatus {
            window_started: started,
            frames: 59,
            measured_fps: None,
            dimensions: (0, 0),
            dropped_frames: 0,
        };

        status.record(started + Duration::from_secs(1), (1920, 1080));

        assert!(
            status
                .measured_fps
                .is_some_and(|fps| (fps - 60.0).abs() < f32::EPSILON)
        );
        assert_eq!(status.dimensions, (1920, 1080));
    }

    #[test]
    fn render_queue_replaces_pending_work_with_latest_scene() {
        let queue = RenderQueue::default();
        for source in [SourceId::Camera(1), SourceId::Camera(2)] {
            queue.submit(RenderRequest {
                elements: vec![SceneElement {
                    id: 1,
                    source,
                    layout: inset_layout(),
                }],
                frames: HashMap::new(),
                colors: HashMap::new(),
                locked_dimensions: None,
            });
        }

        assert_eq!(
            queue.take().elements.first().map(|element| element.source),
            Some(SourceId::Camera(2))
        );
    }

    #[test]
    fn composition_hub_replaces_completed_frames_before_delivery() -> anyhow::Result<()> {
        let (sender, receiver) = async_channel::unbounded();
        let hub = CompositionHub {
            latest: Mutex::new(None),
            event_pending: AtomicBool::new(false),
            sender,
        };
        for id in [1, 2] {
            let image = CVPixelBuffer::new(
                core_video::pixel_buffer::kCVPixelFormatType_32BGRA,
                1,
                1,
                None,
            )
            .map_err(|status| anyhow::anyhow!("failed to create test pixel buffer: {status}"))?;
            hub.submit(ComposedFrame {
                image,
                dimensions: (1, 1),
                elements: vec![RenderedElement {
                    id,
                    transform: ItemTransform::new([0.5, 0.5], [1.0, 1.0]),
                }],
            });
        }

        assert!(matches!(
            receiver.try_recv(),
            Ok(ViewerEvent::CompositionReady)
        ));
        assert!(receiver.try_recv().is_err());
        let frame = hub
            .take()
            .ok_or_else(|| anyhow::anyhow!("composition hub has no frame"))?;
        assert_eq!(frame.elements.first().map(|element| element.id), Some(2));
        Ok(())
    }

    #[test]
    fn removing_elements_releases_source_after_last_reference() {
        let source = SourceId::Display(1);
        let mut scene = Scene::new();
        let first = scene.add(source, full_canvas_layout());
        let second = scene.add(source, inset_layout());

        assert!(scene.remove(first).is_some());
        assert!(scene.uses_source(source));
        assert!(scene.remove(second).is_some());
        assert!(!scene.uses_source(source));
    }

    #[test]
    fn scene_reordering_changes_render_order() {
        let source = SourceId::Display(1);
        let mut scene = Scene::new();
        let back = scene.add(source, full_canvas_layout());
        let front = scene.add(source, inset_layout());

        assert!(scene.move_to_index(back, 1));
        assert_eq!(
            scene
                .elements
                .iter()
                .map(|element| element.id)
                .collect::<Vec<_>>(),
            [front, back]
        );
        assert!(!scene.move_to_index(back, 1));
        assert!(scene.move_to_index(back, 0));
        assert_eq!(
            scene
                .elements
                .iter()
                .map(|element| element.id)
                .collect::<Vec<_>>(),
            [back, front]
        );
    }

    #[test]
    fn scene_reorder_offsets_preserve_previous_row_positions() {
        let source = SourceId::Display(1);
        let mut scene = Scene::new();
        let back = scene.add(source, inset_layout());
        let middle = scene.add(source, inset_layout());
        let front = scene.add(source, inset_layout());
        let previous = [back, middle, front];

        assert!(scene.move_to_index(back, 2));
        let offsets = scene_reorder_offsets(&previous, &scene, back);

        assert_eq!(offsets.get(&middle), Some(&-SCENE_ROW_STRIDE));
        assert_eq!(offsets.get(&front), Some(&-SCENE_ROW_STRIDE));
        assert!(!offsets.contains_key(&back));
    }

    #[test]
    fn drag_slots_are_stable_for_three_or_more_items() {
        let bounds = Bounds::new(Point::new(px(0.0), px(0.0)), size(px(256.0), px(108.0)));
        let grab_offset = px(16.0);

        assert_eq!(
            scene_drag_render_index(px(-100.0), grab_offset, bounds, 3),
            Some(2)
        );
        assert_eq!(
            scene_drag_render_index(px(52.0), grab_offset, bounds, 3),
            Some(1)
        );
        assert_eq!(
            scene_drag_render_index(px(500.0), grab_offset, bounds, 3),
            Some(0)
        );
    }

    #[test]
    fn scene_animation_remaining_matches_row_easing() {
        assert!((scene_animation_remaining(Duration::ZERO) - 1.0).abs() < f32::EPSILON);
        assert!(
            (scene_animation_remaining(SCENE_ROW_ANIMATION_DURATION / 2) - 0.03125).abs()
                < f32::EPSILON
        );
        assert!(scene_animation_remaining(SCENE_ROW_ANIMATION_DURATION).abs() < f32::EPSILON);
    }

    #[test]
    fn keyboard_nudge_moves_one_canvas_pixel() {
        let center = nudged_center([0.5, 0.5], [1.0, -1.0], [1920.0, 1080.0]);

        assert!((center[0] - (0.5 + 1.0 / 1920.0)).abs() < f32::EPSILON);
        assert!((center[1] - (0.5 - 1.0 / 1080.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn canvas_drag_positions_are_not_clamped_to_canvas_bounds() {
        let before = normalized_position_unclamped(px(-20.0), px(0.0), px(100.0));
        let after = normalized_position_unclamped(px(120.0), px(0.0), px(100.0));

        assert!((before + 0.2).abs() < f32::EPSILON);
        assert!((after - 1.2).abs() < f32::EPSILON);
    }

    #[test]
    fn scene_drag_origin_is_x_locked_and_y_clamped() {
        let bounds = Bounds::new(Point::new(px(12.0), px(40.0)), size(px(256.0), px(100.0)));

        let above = clamped_scene_drag_origin(Point::new(px(80.0), px(0.0)), bounds);
        let inside = clamped_scene_drag_origin(Point::new(px(120.0), px(64.0)), bounds);
        let below = clamped_scene_drag_origin(Point::new(px(200.0), px(200.0)), bounds);

        assert_eq!(above, Point::new(px(12.0), px(40.0)));
        assert_eq!(inside, Point::new(px(12.0), px(64.0)));
        assert_eq!(below, Point::new(px(12.0), px(108.0)));
    }

    #[test]
    fn render_requests_are_coalesced_until_drained() {
        let (sender, receiver) = async_channel::unbounded();
        let hub = FrameHub {
            frames: Mutex::new(HashMap::new()),
            camera_statuses: Mutex::new(HashMap::new()),
            removed_sources: Mutex::new(Vec::new()),
            render_pending: AtomicBool::new(false),
            sender,
        };

        hub.request_render();
        hub.request_render();

        assert!(matches!(receiver.try_recv(), Ok(ViewerEvent::RenderReady)));
        assert!(receiver.try_recv().is_err());
        hub.drain();
        hub.request_render();
        assert!(matches!(receiver.try_recv(), Ok(ViewerEvent::RenderReady)));
    }

    #[test]
    fn capture_restart_ignores_stale_generations_and_unused_sources() {
        assert!(should_restart_capture(true, Some(2), 2));
        assert!(!should_restart_capture(true, Some(3), 2));
        assert!(!should_restart_capture(false, Some(2), 2));
        assert!(!should_restart_capture(true, None, 2));
    }

    #[test]
    fn pending_capture_ignores_stale_completions_and_deleted_sources() {
        assert!(should_finish_pending_capture(Some(4), 4, true));
        assert!(!should_finish_pending_capture(Some(5), 4, true));
        assert!(!should_finish_pending_capture(Some(4), 4, false));
        assert!(!should_finish_pending_capture(None, 4, true));
    }
}

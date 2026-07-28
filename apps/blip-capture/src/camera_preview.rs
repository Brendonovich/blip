use std::{
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use blip_avfoundation::{CameraCapturer, CameraDevice, CameraFrame};
use blip_compositor::{
    CompositorItem, CompositorItemContent, CompositorSource, ContentRect, FrameCompositor,
    ItemTransform,
};
use core_foundation::base::TCFType as _;
use core_video::pixel_buffer::CVPixelBuffer;
use gpui::{
    Animation, AnimationExt, Bounds, Context, CursorStyle, DispatchPhase, Div, IntoElement,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    Point, Render, Stateful, Window, canvas, div, point, prelude::*, px, rgb, rgba, size, surface,
    svg,
};

use crate::{
    CameraMenuAction,
    assets::{CLOSE, CORNER_CIRCLE, CORNER_SQUIRCLE, SHAPE_FRAME},
    set_camera_window_bounds,
};

const CONTROL_STRIP_HEIGHT: f32 = 42.0;
const CORNER_RADIUS: f32 = 32.0;
const CONTROL_ANIMATION_DURATION: Duration = Duration::from_millis(140);

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewStyle {
    Circle,
    Squircle,
    Squirectangle,
}

impl PreviewStyle {
    fn from_atomic(value: u8) -> Self {
        match value {
            0 => Self::Circle,
            1 => Self::Squircle,
            _ => Self::Squirectangle,
        }
    }

    const fn atomic_value(self) -> u8 {
        match self {
            Self::Circle => 0,
            Self::Squircle => 1,
            Self::Squirectangle => 2,
        }
    }
}

#[derive(Clone, Copy)]
enum ResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

struct ResizeDrag {
    corner: ResizeCorner,
    start_bounds: Bounds<Pixels>,
    start_pointer: Point<Pixels>,
    aspect_ratio: f32,
}

pub(crate) struct CameraPreview {
    #[allow(dead_code)]
    capturer: Option<CameraCapturer>,
    frame: Option<CVPixelBuffer>,
    error: Option<String>,
    style: PreviewStyle,
    requested_style: PreviewStyle,
    shared_style: Arc<AtomicU8>,
    processor_sender: async_channel::Sender<CameraPixelBuffer>,
    latest_camera_frame: Arc<Mutex<Option<CameraPixelBuffer>>>,
    camera_sender: async_channel::Sender<CameraMenuAction>,
    resize_drag: Option<ResizeDrag>,
    pointer_inside: bool,
    hovered: bool,
    controls_visible: bool,
    controls_transition: usize,
}

impl CameraPreview {
    pub(crate) fn new(
        device: CameraDevice,
        camera_sender: async_channel::Sender<CameraMenuAction>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (processor_sender, processor_receiver) = async_channel::bounded(1);
        let (processed_sender, processed_receiver) = async_channel::bounded(1);
        let shared_style = Arc::new(AtomicU8::new(PreviewStyle::Squirectangle.atomic_value()));
        let processor_style = Arc::clone(&shared_style);
        let latest_camera_frame = Arc::new(Mutex::new(None));
        let processor_error = std::thread::Builder::new()
            .name("camera-preview-compositor".into())
            .spawn(move || {
                process_camera_frames(&processor_receiver, &processed_sender, &processor_style);
            })
            .err()
            .map(|error| format!("Failed to start camera compositor: {error}"));
        let capture_sender = processor_sender.clone();
        let capture_latest_frame = Arc::clone(&latest_camera_frame);
        cx.spawn(async move |preview, cx| {
            let started = cx
                .background_executor()
                .spawn(async move {
                    let capturer = CameraCapturer::new(&device, 30, move |frame| {
                        let frame = CameraPixelBuffer(retain_pixel_buffer(&frame));
                        *capture_latest_frame
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(frame.clone());
                        capture_sender.force_send(frame).ok();
                    })?;
                    capturer.start()?;
                    Ok::<_, blip_avfoundation::CameraError>(capturer)
                })
                .await;

            if preview
                .update(cx, |preview, _| match started {
                    Ok(capturer) => preview.capturer = Some(capturer),
                    Err(error) => preview.error = Some(error.to_string()),
                })
                .is_err()
            {
                return;
            }

            while let Ok(result) = processed_receiver.recv().await {
                if preview
                    .update(cx, |preview, cx| {
                        match result {
                            Ok(frame) => {
                                preview.style = frame.style;
                                preview.frame = Some(frame.pixel_buffer);
                                preview.error = None;
                            }
                            Err(error) => preview.error = Some(error),
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            capturer: None,
            frame: None,
            error: processor_error,
            style: PreviewStyle::Squirectangle,
            requested_style: PreviewStyle::Squirectangle,
            shared_style,
            processor_sender,
            latest_camera_frame,
            camera_sender,
            resize_drag: None,
            pointer_inside: false,
            hovered: false,
            controls_visible: false,
            controls_transition: 0,
        }
    }

    fn cycle_style(&mut self, cx: &mut Context<Self>) {
        self.requested_style = match self.requested_style {
            PreviewStyle::Circle => PreviewStyle::Squircle,
            PreviewStyle::Squircle => PreviewStyle::Squirectangle,
            PreviewStyle::Squirectangle => PreviewStyle::Circle,
        };
        self.shared_style
            .store(self.requested_style.atomic_value(), Ordering::Relaxed);
        if let Some(frame) = self
            .latest_camera_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            self.processor_sender.force_send(frame).ok();
        }
        cx.notify();
    }

    fn hide_controls(&mut self, cx: &mut Context<Self>) {
        self.hovered = false;
        self.controls_transition = self.controls_transition.saturating_add(1);
        let transition = self.controls_transition;
        cx.notify();
        cx.spawn(async move |preview, cx| {
            cx.background_executor()
                .timer(CONTROL_ANIMATION_DURATION)
                .await;
            preview
                .update(cx, |preview, cx| {
                    if !preview.hovered && preview.controls_transition == transition {
                        preview.controls_visible = false;
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn preview_bounds(&self, window_bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let content_height = window_bounds.size.height - px(CONTROL_STRIP_HEIGHT);
        let width = match self.style {
            PreviewStyle::Circle | PreviewStyle::Squircle => content_height,
            PreviewStyle::Squirectangle => window_bounds.size.width,
        };
        Bounds::new(
            point(
                window_bounds.origin.x + (window_bounds.size.width - width) / 2.0,
                window_bounds.origin.y + px(CONTROL_STRIP_HEIGHT),
            ),
            size(width, content_height),
        )
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn begin_resize(
        &mut self,
        corner: ResizeCorner,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let window_bounds = window.bounds();
        let start_bounds = self.preview_bounds(window_bounds);
        self.resize_drag = Some(ResizeDrag {
            corner,
            start_bounds,
            start_pointer: window_bounds.origin + event.position,
            aspect_ratio: match self.style {
                PreviewStyle::Circle | PreviewStyle::Squircle => 1.0,
                PreviewStyle::Squirectangle => 16.0 / 9.0,
            },
        });
        self.pointer_inside = true;
        if !self.hovered {
            self.hovered = true;
            self.controls_visible = true;
            self.controls_transition = self.controls_transition.saturating_add(1);
        }
        cx.notify();
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn resize(&mut self, event: &MouseMoveEvent, window: &mut Window) {
        let Some(drag) = &self.resize_drag else {
            return;
        };
        let current_pointer = window.bounds().origin + event.position;
        let delta = current_pointer - drag.start_pointer;
        let start = drag.start_bounds;
        let right = start.origin.x + start.size.width;
        let bottom = start.origin.y + start.size.height;
        let horizontal_width = match drag.corner {
            ResizeCorner::TopLeft | ResizeCorner::BottomLeft => start.size.width - delta.x,
            ResizeCorner::TopRight | ResizeCorner::BottomRight => start.size.width + delta.x,
        };
        let vertical_content_height = match drag.corner {
            ResizeCorner::TopLeft | ResizeCorner::TopRight => start.size.height - delta.y,
            ResizeCorner::BottomLeft | ResizeCorner::BottomRight => start.size.height + delta.y,
        };
        let minimum_width = px(160.0);
        let minimum_content_height = minimum_width / drag.aspect_ratio;
        let start_content_height = start.size.height;
        let horizontal_change = (horizontal_width - start.size.width).abs();
        let vertical_change =
            (vertical_content_height - start_content_height).abs() * drag.aspect_ratio;
        let (width, content_height) = if horizontal_change >= vertical_change {
            let width = horizontal_width.max(minimum_width);
            (width, width / drag.aspect_ratio)
        } else {
            let content_height = vertical_content_height.max(minimum_content_height);
            (content_height * drag.aspect_ratio, content_height)
        };
        let preview_left = match drag.corner {
            ResizeCorner::TopLeft | ResizeCorner::BottomLeft => right - width,
            ResizeCorner::TopRight | ResizeCorner::BottomRight => start.origin.x,
        };
        let preview_top = match drag.corner {
            ResizeCorner::TopLeft | ResizeCorner::TopRight => bottom - content_height,
            ResizeCorner::BottomLeft | ResizeCorner::BottomRight => start.origin.y,
        };
        let window_width = content_height * (16.0 / 9.0);
        let preview_center_x = preview_left + width / 2.0;
        set_camera_window_bounds(
            window,
            Bounds::new(
                point(
                    preview_center_x - window_width / 2.0,
                    preview_top - px(CONTROL_STRIP_HEIGHT),
                ),
                size(window_width, content_height + px(CONTROL_STRIP_HEIGHT)),
            ),
        );
    }
}

impl Render for CameraPreview {
    #[allow(clippy::arithmetic_side_effects)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let window_size = window.bounds().size;
        let content_height = window_size.height - px(CONTROL_STRIP_HEIGHT);
        let horizontal_inset = match self.style {
            PreviewStyle::Circle | PreviewStyle::Squircle => {
                (window_size.width - content_height) / 2.0
            }
            PreviewStyle::Squirectangle => px(0.0),
        };
        let resize_handlers = self.resize_drag.is_some().then(|| {
            let preview = cx.entity();
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let preview_for_move = preview.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            preview_for_move.update(cx, |preview, _| preview.resize(event, window));
                        }
                    });
                    let preview_for_up = preview.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                            preview_for_up.update(cx, |preview, cx| {
                                preview.resize_drag = None;
                                if !preview.pointer_inside {
                                    preview.hide_controls(cx);
                                    return;
                                }
                                cx.notify();
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_full()
        });
        let content = div()
            .absolute()
            .left(horizontal_inset)
            .right(horizontal_inset)
            .top(px(CONTROL_STRIP_HEIGHT))
            .bottom_0()
            .overflow_hidden()
            .when(self.frame.is_none(), |content| content.bg(rgb(0x0010_1114)))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .when_some(self.frame.clone(), |preview, frame| {
                preview.child(surface(frame).size_full().object_fit(ObjectFit::Cover))
            })
            .when_some(self.error.clone(), |preview, error| {
                preview.child(
                    div()
                        .size_full()
                        .p_4()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_center()
                        .text_sm()
                        .text_color(rgb(0x00f2_f2f4))
                        .child(error),
                )
            });
        div()
            .size_full()
            .on_mouse_move(cx.listener(|preview, _: &MouseMoveEvent, _, cx| {
                preview.pointer_inside = true;
                if !preview.hovered {
                    preview.hovered = true;
                    preview.controls_visible = true;
                    preview.controls_transition = preview.controls_transition.saturating_add(1);
                    cx.notify();
                }
            }))
            .on_mouse_exit(cx.listener(|preview, _: &MouseExitEvent, _, cx| {
                preview.pointer_inside = false;
                if !preview.hovered {
                    return;
                }
                if preview.resize_drag.is_none() {
                    preview.hide_controls(cx);
                }
            }))
            .when_some(resize_handlers, |preview, handlers| preview.child(handlers))
            .child(content)
            .when(self.controls_visible, |preview| {
                let entering = self.hovered;
                let transition = self.controls_transition;
                preview.child(
                    div()
                        .absolute()
                        .top(px(4.0))
                        .left_0()
                        .right_0()
                        .h(px(34.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .relative()
                                .h(px(34.0))
                                .px_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .flex_none()
                                .rounded_lg()
                                .bg(rgba(0x1011_14dc))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    control_button(
                                        "camera-style",
                                        match self.requested_style {
                                            PreviewStyle::Circle => CORNER_CIRCLE,
                                            PreviewStyle::Squircle => CORNER_SQUIRCLE,
                                            PreviewStyle::Squirectangle => SHAPE_FRAME,
                                        },
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |preview, _, _, cx| {
                                            preview.cycle_style(cx);
                                        },
                                    )),
                                )
                                .child(control_button("camera-close", CLOSE, false).on_click(
                                    cx.listener(|preview, _, _, _| {
                                        preview.camera_sender.try_send(CameraMenuAction(None)).ok();
                                    }),
                                ))
                                .with_animation(
                                    format!("camera-controls-transition-{transition}"),
                                    Animation::new(CONTROL_ANIMATION_DURATION)
                                        .with_easing(gpui::ease_out_quint()),
                                    move |bar, delta| {
                                        let visibility = if entering { delta } else { 1.0 - delta };
                                        bar.top(px(4.0 * (1.0 - visibility))).opacity(visibility)
                                    },
                                ),
                        ),
                )
            })
            .child(
                resize_handle(
                    "resize-top-left",
                    ResizeCorner::TopLeft,
                    CursorStyle::ResizeUpLeftDownRight,
                    cx,
                )
                .left(horizontal_inset)
                .top(px(CONTROL_STRIP_HEIGHT)),
            )
            .child(
                resize_handle(
                    "resize-top-right",
                    ResizeCorner::TopRight,
                    CursorStyle::ResizeUpRightDownLeft,
                    cx,
                )
                .right(horizontal_inset)
                .top(px(CONTROL_STRIP_HEIGHT)),
            )
            .child(
                resize_handle(
                    "resize-bottom-left",
                    ResizeCorner::BottomLeft,
                    CursorStyle::ResizeUpRightDownLeft,
                    cx,
                )
                .left(horizontal_inset)
                .bottom_0(),
            )
            .child(
                resize_handle(
                    "resize-bottom-right",
                    ResizeCorner::BottomRight,
                    CursorStyle::ResizeUpLeftDownRight,
                    cx,
                )
                .right(horizontal_inset)
                .bottom_0(),
            )
    }
}

fn resize_handle(
    id: &'static str,
    corner: ResizeCorner,
    cursor: CursorStyle,
    cx: &mut Context<CameraPreview>,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .size(px(16.0))
        .cursor(cursor)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |preview, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                preview.begin_resize(corner, event, window, cx);
            }),
        )
}

fn control_button(id: &'static str, icon: &'static str, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(rgba(if selected { 0xffff_ff28 } else { 0x0000_0000 }))
        .hover(|button| button.bg(rgba(0xffff_ff20)))
        .cursor_pointer()
        .child(svg().size(px(17.0)).path(icon).text_color(rgb(0x00ff_ffff)))
}

struct ProcessedFrame {
    pixel_buffer: CVPixelBuffer,
    style: PreviewStyle,
}

// SAFETY: CVPixelBuffers use thread-safe Core Foundation ownership and are immutable after GPU
// composition completes.
unsafe impl Send for ProcessedFrame {}

#[derive(Clone)]
struct CameraPixelBuffer(CVPixelBuffer);

// SAFETY: Captured CVPixelBuffers are immutable while retained and CoreVideo permits them to move
// between processing queues.
unsafe impl Send for CameraPixelBuffer {}

fn process_camera_frames(
    receiver: &async_channel::Receiver<CameraPixelBuffer>,
    sender: &async_channel::Sender<Result<ProcessedFrame, String>>,
    style: &AtomicU8,
) {
    let mut compositor = match FrameCompositor::new() {
        Ok(compositor) => compositor,
        Err(error) => {
            sender
                .force_send(Err(format!(
                    "Failed to initialize camera compositor: {error}"
                )))
                .ok();
            return;
        }
    };
    while let Ok(frame) = receiver.recv_blocking() {
        let style = PreviewStyle::from_atomic(style.load(Ordering::Relaxed));
        let result = render_camera_frame(&mut compositor, &frame.0, style)
            .map(|pixel_buffer| ProcessedFrame {
                pixel_buffer,
                style,
            })
            .map_err(|error| format!("Failed to render camera preview: {error}"));
        if sender.force_send(result).is_err() {
            break;
        }
    }
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::integer_division
)]
fn render_camera_frame(
    compositor: &mut FrameCompositor,
    frame: &CVPixelBuffer,
    style: PreviewStyle,
) -> anyhow::Result<CVPixelBuffer> {
    let source_width = frame.get_width();
    let source_height = frame.get_height();
    let output_dimensions = match style {
        PreviewStyle::Circle | PreviewStyle::Squircle => {
            let side = source_width.min(source_height);
            (side, side)
        }
        PreviewStyle::Squirectangle if source_width * 9 >= source_height * 16 => {
            (source_height * 16 / 9, source_height)
        }
        PreviewStyle::Squirectangle => (source_width, source_width * 9 / 16),
    };
    let crop_width = output_dimensions.0 as f64;
    let crop_height = output_dimensions.1 as f64;
    let corner_radius = match style {
        PreviewStyle::Circle => output_dimensions.0.min(output_dimensions.1) as f32 / 2.0,
        PreviewStyle::Squircle | PreviewStyle::Squirectangle => {
            output_dimensions.0.min(output_dimensions.1) as f32 * CORNER_RADIUS / 180.0
        }
    };
    let transform = ItemTransform::new([0.5, 0.5], [1.0, 1.0])
        .with_corner_radius(corner_radius)
        .with_squircle(style != PreviewStyle::Circle);
    compositor.render_transparent(
        &[CompositorSource {
            pixel_buffer: frame,
            content_rect: Some(ContentRect {
                x: (source_width as f64 - crop_width) / 2.0,
                y: (source_height as f64 - crop_height) / 2.0,
                width: crop_width,
                height: crop_height,
            }),
        }],
        &[CompositorItem {
            content: CompositorItemContent::Source(0),
            transform,
        }],
        output_dimensions,
    )
}

fn retain_pixel_buffer(frame: &CameraFrame) -> CVPixelBuffer {
    let pixel_buffer = ptr::from_ref(frame.image_buffer())
        .cast_mut()
        .cast::<core_video::buffer::__CVBuffer>();
    // SAFETY: Both bindings wrap the same retained CoreVideo pixel buffer. This constructor
    // retains the buffer before the camera frame is released.
    unsafe { CVPixelBuffer::wrap_under_get_rule(pixel_buffer) }
}

#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division
)]

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, ObjectFit, Pixels, Render, SharedString, Window, WindowOptions, canvas, div,
    prelude::*, px, relative, rgb, rgba, surface,
};

use crate::bundle::BlipBundle;

const BACKGROUND: u32 = 0x0013_1417;
const PANEL: u32 = 0x001b_1d21;
const CONTROL: u32 = 0x0028_2b31;
const TEXT: u32 = 0x00f2_f2f4;
const MUTED: u32 = 0x009a_9da6;
const ACCENT: u32 = 0x00ff_4f58;

pub(crate) struct BundleEditor {
    path: PathBuf,
    bundle: BlipBundle,
    selected_input: usize,
    decoders: Vec<blip_avfoundation::VideoDecoder>,
    compositor: blip_compositor::FrameCompositor,
    duration_secs: f64,
    current_time_secs: f64,
    cursor_time_secs: Option<f64>,
    current_frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    timeline_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl BundleEditor {
    pub(crate) fn open(path: PathBuf, cx: &mut App) -> Result<(), String> {
        let bundle = BlipBundle::load(&path)?;
        tracing::info!(
            path = %path.display(),
            inputs = bundle.inputs.len(),
            "Opening bundle editor"
        );
        let mut decoders = Vec::new();
        let mut max_duration = 0.0;
        for input in &bundle.inputs {
            let media_path = path.join(&input.media);
            match blip_avfoundation::VideoDecoder::open(&media_path) {
                Ok(decoder) => {
                    let dur = decoder.duration().as_secs_f64();
                    if dur > max_duration {
                        max_duration = dur;
                    }
                    decoders.push(decoder);
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
        let compositor = blip_compositor::FrameCompositor::new()
            .map_err(|e| format!("failed to create compositor: {e}"))?;

        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Blip Bundle")
            .to_owned();
        cx.open_window(WindowOptions::default(), move |window, cx| {
            window.set_window_title(&title);
            cx.new(|cx| {
                let mut editor = Self {
                    path,
                    bundle,
                    selected_input: 0,
                    decoders,
                    compositor,
                    duration_secs: max_duration,
                    current_time_secs: 0.0,
                    cursor_time_secs: None,
                    current_frame: None,
                    timeline_bounds: Rc::new(Cell::new(Bounds::default())),
                };
                editor.update_preview(cx);
                editor
            })
        })
        .map_err(|error| format!("failed to open bundle editor: {error}"))?;
        cx.activate(true);
        Ok(())
    }

    fn update_preview(&mut self, cx: &mut Context<Self>) {
        let mut active_frames = Vec::new();
        for (i, decoder) in self.decoders.iter_mut().enumerate() {
            if let Ok(pb) = decoder.frame_at(self.current_time_secs) {
                let is_camera = self
                    .bundle
                    .inputs
                    .get(i)
                    .is_some_and(|inp| input_is_camera(inp, i));
                active_frames.push((i, pb, decoder.width(), decoder.height(), is_camera));
            }
        }
        if active_frames.is_empty() {
            eprintln!(
                "blip-capture: no frames decoded at {}s",
                self.current_time_secs
            );
            return;
        }
        active_frames.sort_by_key(|(_, _, _, _, is_cam)| if *is_cam { 1 } else { 0 });

        let (canvas_w, canvas_h) = active_frames
            .iter()
            .find(|(_, _, _, _, is_cam)| !*is_cam)
            .or_else(|| active_frames.first())
            .map(|(_, _, w, h, _)| (*w, *h))
            .unwrap_or((1920, 1080));

        let mut sources = Vec::with_capacity(active_frames.len());
        let mut items = Vec::with_capacity(active_frames.len());
        for (idx, (_, pb, w, h, is_camera)) in active_frames.iter().enumerate() {
            sources.push(blip_compositor::CompositorSource {
                pixel_buffer: pb,
                content_rect: None,
            });
            let transform = if *is_camera {
                camera_transform((*w as f64, *h as f64), (canvas_w as f64, canvas_h as f64))
            } else {
                blip_compositor::ItemTransform::new([0.5, 0.5], [1.0, 1.0])
            };
            items.push(blip_compositor::CompositorItem {
                content: blip_compositor::CompositorItemContent::Source(idx),
                transform,
            });
        }

        match self.compositor.render(&sources, &items, (canvas_w, canvas_h)) {
            Ok(composed) => {
                self.current_frame = Some(composed);
                cx.notify();
            }
            Err(e) => {
                eprintln!("blip-capture: error composing frame: {e}");
            }
        }
    }

    fn select_input(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.bundle.inputs.get(index).is_some() {
            self.selected_input = index;
            self.update_preview(cx);
        }
    }

    fn scrub_to(&mut self, time_secs: f64, cx: &mut Context<Self>) {
        self.current_time_secs = time_secs.clamp(0.0, self.duration_secs);
        self.update_preview(cx);
    }

    fn timeline(&self, cx: &mut Context<Self>) -> Div {
        let bounds_cell = Rc::clone(&self.timeline_bounds);
        let duration_secs = self.duration_secs;
        let fraction = if duration_secs > 0.0 {
            (self.current_time_secs / duration_secs).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };

        let mut tracks = div().flex().flex_col().gap_2();
        for (index, input) in self.bundle.inputs.iter().enumerate() {
            let bounds_cell = Rc::clone(&bounds_cell);
            let selected = index == self.selected_input;
            tracks = tracks.child(
                div()
                    .h(px(46.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(("track-label", index))
                            .w(px(110.0))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgba(if selected { 0xffff_ff18 } else { 0x0000_0000 }))
                            .hover(|row| row.bg(rgba(0xffff_ff10)))
                            .text_sm()
                            .text_color(if selected { rgb(TEXT) } else { rgb(MUTED) })
                            .on_click(cx.listener(move |editor, _, _, cx| {
                                editor.select_input(index, cx);
                            }))
                            .child(input.name.clone()),
                    )
                    .child(
                        div()
                            .h(px(34.0))
                            .flex_1()
                            .relative()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                                    editor.select_input(index, cx);
                                    let bounds = editor.timeline_bounds.get();
                                    if bounds.size.width > Pixels::ZERO {
                                        let x = (event.position.x - bounds.origin.x)
                                            / bounds.size.width;
                                        let fraction = x.clamp(0.0, 1.0) as f64;
                                        let time_secs = fraction * editor.duration_secs;
                                        editor.cursor_time_secs = Some(time_secs);
                                        editor.scrub_to(time_secs, cx);
                                    }
                                }),
                            )
                            .child(
                                canvas(
                                    move |bounds, _, _| {
                                        bounds_cell.set(bounds);
                                        bounds
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .size_full()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .bg(rgb(0x003b_2630))
                                    .border_1()
                                    .border_color(if selected {
                                        rgba(0xff4f_58a0)
                                    } else {
                                        rgba(0xff4f_5840)
                                    })
                                    .text_sm()
                                    .child(media_name(input.media.as_path())),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(relative(fraction))
                                    .w(px(2.0))
                                    .bg(rgb(TEXT))
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(-3.0))
                                            .left(px(-4.0))
                                            .size(px(10.0))
                                            .rounded_full()
                                            .bg(rgb(TEXT)),
                                    ),
                            ),
                    ),
            );
        }

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
            .h(px(190.0))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .border_t_1()
            .border_color(rgba(0xffff_ff16))
            .bg(rgb(PANEL))
            .on_mouse_move(cx.listener(|editor, event: &MouseMoveEvent, _, cx| {
                let bounds = editor.timeline_bounds.get();
                if bounds.size.width > Pixels::ZERO {
                    if event.position.x >= bounds.origin.x - px(6.0) {
                        let x = (event.position.x - bounds.origin.x) / bounds.size.width;
                        let fraction = x.clamp(0.0, 1.0) as f64;
                        let time_secs = fraction * editor.duration_secs;
                        if editor.cursor_time_secs != Some(time_secs) {
                            editor.cursor_time_secs = Some(time_secs);
                            cx.notify();
                        }
                        if (time_secs - editor.current_time_secs).abs() > 0.03 {
                            editor.scrub_to(time_secs, cx);
                        }
                    } else if editor.cursor_time_secs.is_some() {
                        editor.cursor_time_secs = None;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                    let bounds = editor.timeline_bounds.get();
                    if bounds.size.width > Pixels::ZERO
                        && event.position.x >= bounds.origin.x - px(6.0)
                    {
                        let x = (event.position.x - bounds.origin.x) / bounds.size.width;
                        let fraction = x.clamp(0.0, 1.0) as f64;
                        let time_secs = fraction * editor.duration_secs;
                        editor.cursor_time_secs = Some(time_secs);
                        editor.scrub_to(time_secs, cx);
                    }
                }),
            )
            .on_mouse_exit(cx.listener(|editor, _: &MouseExitEvent, _, cx| {
                if editor.cursor_time_secs.is_some() {
                    editor.cursor_time_secs = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().text_color(rgb(MUTED)).child("TIMELINE"))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(time_label)),
            )
            .child(tracks)
    }
}

impl Render for BundleEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.bundle.inputs.get(self.selected_input);
        let bundle_path = self.path.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .h(px(54.0))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgba(0xffff_ff16))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Blip Bundle Editor"),
                    )
                    .child(
                        div()
                            .id("reveal-bundle")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(CONTROL))
                            .hover(|button| button.bg(rgba(0xffff_ff20)))
                            .cursor_pointer()
                            .text_sm()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.reveal_path(&bundle_path);
                            }))
                            .child("Show in Finder"),
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
                            .p_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if let Some(frame) = &self.current_frame {
                                div()
                                    .size_full()
                                    .max_w(px(1280.0))
                                    .max_h(px(720.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        surface(frame.clone())
                                            .size_full()
                                            .object_fit(ObjectFit::Contain),
                                    )
                            } else {
                                div()
                                    .w_full()
                                    .max_w(px(720.0))
                                    .aspect_ratio(1.777_777_8)
                                    .rounded_lg()
                                    .bg(rgb(0x0007_080a))
                                    .border_1()
                                    .border_color(rgba(0xffff_ff16))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_3()
                                    .child(div().size(px(12.0)).rounded_full().bg(rgb(ACCENT)))
                                    .child(div().text_color(rgb(MUTED)).child(
                                        selected.map_or_else(
                                            || SharedString::from("No input selected"),
                                            |input| media_name(input.media.as_path()),
                                        ),
                                    ))
                            }),
                    ),
            )
            .child(self.timeline(cx))
    }
}

fn media_name(path: &std::path::Path) -> SharedString {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Recording")
        .to_owned()
        .into()
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

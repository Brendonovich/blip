use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use blip_sck::{CaptureError, Display, ShareableContent, Window as CaptureWindow};
use chrono::Local;
use core_graphics::window::{
    create_window_list, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionOnScreenOnly,
};
use dispatch2::DispatchQueue;
use gpui::{
    AnyWindowHandle, App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, Div, Entity,
    ExternalPaths, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, point, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use objc2::rc::Retained;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSControlStateValueOn, NSMenu, NSMenuItem,
    NSRunningApplication, NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[path = "../../blip-studio/src/numeric_input.rs"]
#[allow(dead_code)]
mod numeric_input;
mod recording;
mod theme;

use numeric_input::{NumericInput, NumericInputEvent};
use recording::{CaptureSpec, RecordingEvent};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowLevelForKey(key: i32) -> i32;
}

const CG_MAXIMUM_WINDOW_LEVEL_KEY: i32 = 10;

const TEXT: u32 = 0x00f2_f2f4;
const MUTED: u32 = 0x00a6_a8b0;
const CONTROL: u32 = 0x0040_4249;
const ACCENT: u32 = 0x00ff_4f58;
const OVERLAY_BLACK: u32 = 0x0000_0060;
const OVERLAY_BLUE_TINT: u32 = 0x184d_8280;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(250);
const TOOLBAR_SIZE: (f32, f32) = (556.0, 58.0);
type RegionSelection = (u32, f64, f64, f64, f64);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Display,
    Window,
    Region,
}

enum Status {
    Idle,
    Starting,
    Recording,
    Finalizing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveDestination {
    Desktop,
    Documents,
    Downloads,
    Clipboard,
}

impl SaveDestination {
    const ALL: [Self; 4] = [
        Self::Desktop,
        Self::Documents,
        Self::Downloads,
        Self::Clipboard,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Desktop => "Desktop",
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
            Self::Clipboard => "Clipboard",
        }
    }
}

enum DestinationMenuAction {
    SelectDestination(SaveDestination),
    ToggleOpenFinder,
}

struct DestinationMenuHandlerIvars {
    sender: async_channel::Sender<DestinationMenuAction>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = DestinationMenuHandlerIvars]
    struct DestinationMenuHandler;

    unsafe impl NSObjectProtocol for DestinationMenuHandler {}

    impl DestinationMenuHandler {
        #[unsafe(method(selectDestination:))]
        fn select_destination(&self, item: &NSMenuItem) {
            let Ok(index) = usize::try_from(item.tag()) else {
                return;
            };
            if let Some(destination) = SaveDestination::ALL.get(index).copied() {
                self.ivars()
                    .sender
                    .try_send(DestinationMenuAction::SelectDestination(destination))
                    .ok();
            }
        }

        #[unsafe(method(toggleOpenFinder:))]
        fn toggle_open_finder(&self, _: &NSMenuItem) {
            self.ivars()
                .sender
                .try_send(DestinationMenuAction::ToggleOpenFinder)
                .ok();
        }
    }
);

impl DestinationMenuHandler {
    fn new(sender: async_channel::Sender<DestinationMenuAction>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DestinationMenuHandlerIvars { sender });
        // SAFETY: The object has fully initialized ivars and NSObject permits `init`.
        unsafe { msg_send![super(this), init] }
    }
}

struct SelectionState {
    mode: Cell<Option<Mode>>,
    display: Cell<u32>,
    window: Cell<Option<u32>>,
    hovered_window: Cell<Option<u32>>,
    region: Cell<Option<RegionSelection>>,
    recording: Cell<bool>,
}

#[derive(Clone)]
struct CaptureTargets {
    displays: Vec<Display>,
    windows: Vec<CaptureWindow>,
}

impl CaptureTargets {
    fn from_content(content: &ShareableContent) -> Self {
        Self {
            displays: content.displays(),
            windows: content.application_windows(),
        }
    }
}

struct CaptureApp {
    controller_window: AnyWindowHandle,
    displays: Vec<Display>,
    windows: Vec<CaptureWindow>,
    mode: Option<Mode>,
    selected_display: u32,
    selected_window: Option<u32>,
    region: Option<RegionSelection>,
    selection: Rc<SelectionState>,
    selection_windows: Vec<AnyWindowHandle>,
    visual_windows: Vec<AnyWindowHandle>,
    status: Status,
    started_at: Option<Instant>,
    stop_sender: Option<mpsc::Sender<()>>,
    event_sender: async_channel::Sender<RecordingEvent>,
    visible: Rc<Cell<bool>>,
    escape_hotkey: Rc<Cell<*mut c_void>>,
    destination: SaveDestination,
    destination_sender: async_channel::Sender<DestinationMenuAction>,
    open_finder_after_recording: bool,
    error: Option<String>,
}

impl CaptureApp {
    fn new(
        targets: &CaptureTargets,
        controller_window: AnyWindowHandle,
        visible: Rc<Cell<bool>>,
        escape_hotkey: Rc<Cell<*mut c_void>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let displays = targets.displays.clone();
        let selected_display = 0;
        let windows = targets.windows.clone();
        let selected_window = None;
        let selection = Rc::new(SelectionState {
            mode: Cell::new(Some(Mode::Display)),
            display: Cell::new(selected_display),
            window: Cell::new(selected_window),
            hovered_window: Cell::new(None),
            region: Cell::new(None),
            recording: Cell::new(false),
        });
        let (event_sender, receiver) = async_channel::unbounded();
        let (destination_sender, destination_receiver) = async_channel::unbounded();
        cx.spawn(async move |app, cx| {
            while let Ok(event) = receiver.recv().await {
                let finished = app
                    .update(cx, |app, cx| app.handle_event(event, cx))
                    .unwrap_or(false);
                if finished {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |app, cx| {
            while let Ok(action) = destination_receiver.recv().await {
                if app
                    .update(cx, |app, cx| {
                        match action {
                            DestinationMenuAction::SelectDestination(destination) => {
                                app.destination = destination;
                            }
                            DestinationMenuAction::ToggleOpenFinder => {
                                app.open_finder_after_recording = !app.open_finder_after_recording;
                            }
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
        cx.spawn(async move |app, cx| {
            loop {
                cx.background_executor().timer(WINDOW_POLL_INTERVAL).await;
                let active = app
                    .update(cx, |app, _| {
                        app.mode == Some(Mode::Window)
                            && matches!(app.status, Status::Idle)
                            && app.visible.get()
                    })
                    .unwrap_or(false);
                if !active {
                    continue;
                }
                let snapshot = cx
                    .background_executor()
                    .spawn(async { ShareableContent::current(CAPTURE_TIMEOUT) })
                    .await;
                let Ok(snapshot) = snapshot else {
                    continue;
                };
                let windows = snapshot.application_windows();
                if app
                    .update(cx, |app, cx| {
                        app.update_window_snapshot(&windows, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            controller_window,
            displays,
            windows,
            mode: Some(Mode::Display),
            selected_display,
            selected_window,
            region: None,
            selection,
            selection_windows: Vec::new(),
            visual_windows: Vec::new(),
            status: Status::Idle,
            started_at: None,
            stop_sender: None,
            event_sender,
            visible,
            escape_hotkey,
            destination: SaveDestination::Desktop,
            destination_sender,
            open_finder_after_recording: true,
            error: None,
        }
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Some(mode) {
            self.mode = None;
            self.selection.mode.set(None);
            self.region = None;
            self.selection.region.set(None);
            self.error = None;
            self.close_selection_windows(cx);
            cx.notify();
            return;
        }
        self.mode = Some(mode);
        self.selection.mode.set(Some(mode));
        self.selection.hovered_window.set(None);
        self.region = None;
        self.selection.region.set(None);
        self.error = None;
        if self.selection_windows.is_empty() {
            self.open_selection_windows(window, cx);
        } else {
            self.refresh_selection_windows(cx);
            self.activate_toolbar(cx);
        }
        cx.notify();
    }

    fn open_selection_windows(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.close_selection_windows(cx);
        let controller = cx.entity();
        for display in self.displays.clone() {
            let Some(screen) = cx
                .displays()
                .into_iter()
                .find(|screen| u64::from(screen.id()) == u64::from(display.id()))
            else {
                continue;
            };
            let overlay_bounds = Bounds::new(point(px(0.0), px(0.0)), screen.bounds().size);
            let windows: Vec<_> = self
                .windows
                .iter()
                .filter(|window| {
                    window
                        .display()
                        .is_some_and(|owner| owner.id() == display.id())
                })
                .cloned()
                .collect();
            for visual in [false, true] {
                let options = selection_window_options(screen.id(), overlay_bounds);
                let selection = Rc::clone(&self.selection);
                let display = display.clone();
                let windows = windows.clone();
                match cx.open_window(options, {
                    let controller = controller.clone();
                    move |_, cx| {
                        cx.new(|cx| {
                            SelectionOverlay::new(
                                display, windows, controller, selection, visual, cx,
                            )
                        })
                    }
                }) {
                    Ok(handle) => {
                        handle
                            .update(cx, |_, window, _| {
                                configure_selection_window(window, visual);
                            })
                            .ok();
                        if visual {
                            self.visual_windows.push(handle.into());
                        } else {
                            self.selection_windows.push(handle.into());
                        }
                    }
                    Err(error) => {
                        self.error = Some(format!("Failed to open selection overlay: {error}"));
                    }
                }
            }
        }
        self.controller_window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }

    fn close_selection_windows(&mut self, cx: &mut Context<Self>) {
        for handle in self
            .selection_windows
            .drain(..)
            .chain(self.visual_windows.drain(..))
        {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    }

    fn close_interaction_windows(&mut self, cx: &mut Context<Self>) {
        for handle in self.selection_windows.drain(..) {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    }

    fn show_destination_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if schedule_destination_menu(
            window,
            self.destination,
            self.open_finder_after_recording,
            self.destination_sender.clone(),
        )
        .is_err()
        {
            self.error = Some("Failed to open the destination menu".into());
            cx.notify();
        }
    }

    fn select_display(&mut self, display_id: u32, cx: &mut Context<Self>) {
        self.selected_display = display_id;
        self.region = None;
        self.selection.display.set(display_id);
        self.selection.region.set(None);
        self.refresh_selection_windows(cx);
        self.activate_toolbar(cx);
    }

    fn select_window(&mut self, window_id: u32, cx: &mut Context<Self>) {
        self.selected_window = Some(window_id);
        self.selection.window.set(Some(window_id));
        self.refresh_selection_windows(cx);
        self.activate_toolbar(cx);
    }

    fn select_region(&mut self, display_id: u32, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.selected_display = display_id;
        self.region = Some((
            display_id,
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.y),
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
        ));
        self.selection.display.set(display_id);
        self.selection.region.set(self.region);
        self.refresh_selection_windows(cx);
        self.activate_toolbar(cx);
    }

    fn refresh_selection_windows(&self, cx: &mut Context<Self>) {
        for handle in self.selection_windows.iter().chain(&self.visual_windows) {
            handle.update(cx, |_, window, _| window.refresh()).ok();
        }
        cx.notify();
    }

    fn update_window_snapshot(&mut self, windows: &[CaptureWindow], cx: &mut Context<Self>) {
        if self
            .selected_window
            .is_some_and(|id| !windows.iter().any(|window| window.id() == id))
        {
            self.selected_window = None;
            self.selection.window.set(None);
        }
        if self
            .selection
            .hovered_window
            .get()
            .is_some_and(|id| !windows.iter().any(|window| window.id() == id))
        {
            self.selection.hovered_window.set(None);
        }
        self.windows.clear();
        self.windows.extend_from_slice(windows);
        for window_handle in self.selection_windows.iter().chain(&self.visual_windows) {
            let Some(window_handle) = window_handle.downcast::<SelectionOverlay>() else {
                continue;
            };
            let windows = windows.to_vec();
            window_handle
                .update(cx, move |overlay, _, cx| {
                    let display_id = overlay.display.id();
                    overlay.windows = windows
                        .into_iter()
                        .filter(|window| {
                            window
                                .display()
                                .is_some_and(|display| display.id() == display_id)
                        })
                        .collect();
                    cx.notify();
                })
                .ok();
        }
        cx.notify();
    }

    fn activate_toolbar(&self, cx: &mut Context<Self>) {
        self.controller_window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }

    fn record(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Idle) {
            return;
        }
        let spec = match self.mode {
            Some(Mode::Display) if self.selected_display != 0 => {
                CaptureSpec::Display(self.selected_display)
            }
            Some(Mode::Window) => {
                let Some(window_id) = self.selected_window else {
                    self.error = Some("Select a window first".into());
                    cx.notify();
                    return;
                };
                CaptureSpec::Window(window_id)
            }
            Some(Mode::Region) => {
                let Some((display_id, x, y, width, height)) = self.region else {
                    self.error = Some("Drag to select a recording area".into());
                    cx.notify();
                    return;
                };
                CaptureSpec::Region {
                    display_id,
                    x,
                    y,
                    width,
                    height,
                }
            }
            Some(Mode::Display) => {
                self.error = Some("Select a display first".into());
                cx.notify();
                return;
            }
            None => {
                self.error = Some("Choose a capture mode".into());
                cx.notify();
                return;
            }
        };
        let sender = match recording::spawn(
            spec,
            output_path(self.destination),
            self.event_sender.clone(),
        ) {
            Ok(sender) => sender,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.selection.recording.set(true);
        self.refresh_selection_windows(cx);
        self.close_interaction_windows(cx);
        self.stop_sender = Some(sender);
        self.status = Status::Starting;
        self.error = None;
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Recording) {
            return;
        }
        self.status = Status::Finalizing;
        if let Some(sender) = self.stop_sender.take() {
            sender.send(()).ok();
        }
        cx.notify();
    }

    fn close_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let application = topmost_application_pid(&self.windows);
        self.selection.recording.set(false);
        self.close_selection_windows(cx);
        self.visible.set(false);
        unregister_escape_hotkey(&self.escape_hotkey);
        window.remove_window();
        cx.defer(move |_| {
            if let Some(process_id) = application
                && let Some(application) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(process_id)
            {
                application.activateWithOptions(NSApplicationActivationOptions::empty());
            }
        });
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.take().is_some() {
            self.selected_display = 0;
            self.selected_window = None;
            self.region = None;
            self.selection.mode.set(None);
            self.selection.display.set(0);
            self.selection.window.set(None);
            self.selection.hovered_window.set(None);
            self.selection.region.set(None);
            self.close_selection_windows(cx);
            cx.notify();
        } else {
            self.close_windows(window, cx);
        }
    }

    fn handle_event(&mut self, event: RecordingEvent, cx: &mut Context<Self>) -> bool {
        match event {
            RecordingEvent::Started => {
                self.status = Status::Recording;
                self.started_at = Some(Instant::now());
                self.resize_toolbar((300.0, 56.0), cx);
                cx.spawn(async move |app, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                        let keep_ticking = app
                            .update(cx, |app, cx| {
                                cx.notify();
                                matches!(app.status, Status::Recording | Status::Finalizing)
                            })
                            .unwrap_or(false);
                        if !keep_ticking {
                            break;
                        }
                    }
                })
                .detach();
            }
            RecordingEvent::Finished(path) => {
                if self.destination == SaveDestination::Clipboard {
                    cx.write_to_clipboard(ClipboardItem {
                        entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                            std::iter::once(path.clone()).collect(),
                        ))],
                    });
                } else if self.open_finder_after_recording {
                    cx.reveal_path(&path);
                }
                self.selection.recording.set(false);
                self.close_selection_windows(cx);
                self.visible.set(false);
                unregister_escape_hotkey(&self.escape_hotkey);
                let controller = self.controller_window;
                cx.defer(move |cx| {
                    controller
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                });
                return true;
            }
            RecordingEvent::Failed(message) => {
                self.status = Status::Idle;
                self.started_at = None;
                self.stop_sender = None;
                self.selection.recording.set(false);
                self.error = Some(message);
                self.resize_toolbar(TOOLBAR_SIZE, cx);
                let controller = self.controller_window;
                cx.defer(move |cx| {
                    let Some(controller) = controller.downcast::<CaptureApp>() else {
                        return;
                    };
                    controller
                        .update(cx, |app, window, cx| {
                            app.close_selection_windows(cx);
                            app.open_selection_windows(window, cx);
                        })
                        .ok();
                });
            }
        }
        cx.notify();
        false
    }

    fn resize_toolbar(&self, dimensions: (f32, f32), cx: &mut Context<Self>) {
        self.controller_window
            .update(cx, |_, window, _| {
                window.resize(size(px(dimensions.0), px(dimensions.1)));
            })
            .ok();
    }

    fn mode_button(
        &self,
        mode: Mode,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.mode == Some(mode);
        div()
            .id(label)
            .px_3()
            .h(px(36.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(rgba(if selected { 0xffff_ff18 } else { 0x0000_0000 }))
            .bg(rgba(if selected { 0xffff_ff20 } else { 0x0000_0000 }))
            .text_sm()
            .text_color(rgb(if selected { TEXT } else { MUTED }))
            .font_weight(if selected {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            })
            .hover(|button| button.bg(rgba(0xffff_ff14)).text_color(rgb(TEXT)))
            .cursor_pointer()
            .on_click(cx.listener(move |app, _, window, cx| app.set_mode(mode, window, cx)))
            .child(label)
    }
}

impl Render for CaptureApp {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = div()
            .size_full()
            .px_2()
            .py_1()
            .flex()
            .items_center()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0xffff_ff20))
            .bg(rgba(0x1819_1ca0))
            .text_color(rgb(TEXT))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move();
            });
        if matches!(self.status, Status::Idle) {
            return shell
                .child(
                    div()
                        .id("close")
                        .w(px(36.0))
                        .h(px(36.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .text_base()
                        .text_color(rgb(MUTED))
                        .hover(|button| button.bg(rgba(0xffff_ff12)).text_color(rgb(TEXT)))
                        .cursor_pointer()
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.close_windows(window, cx);
                        }))
                        .child(div().relative().top(px(-1.0)).child("×")),
                )
                .child(self.mode_button(Mode::Display, "Display", cx))
                .child(self.mode_button(Mode::Window, "Window", cx))
                .child(self.mode_button(Mode::Region, "Region", cx))
                .child(
                    div()
                        .mx_1()
                        .w(px(1.0))
                        .h(px(28.0))
                        .flex_none()
                        .bg(rgba(0xffff_ff30)),
                )
                .child(
                    overlay_secondary_button("destination", "Options").on_click(cx.listener(
                        |app, _, window, cx| {
                            app.show_destination_menu(window, cx);
                        },
                    )),
                )
                .child(
                    overlay_start_button("record")
                        .on_click(cx.listener(|app, _, _, cx| app.record(cx))),
                );
        }

        let elapsed = self
            .started_at
            .map_or(Duration::ZERO, |start| start.elapsed());
        let seconds = elapsed.as_secs();
        let time = format!(
            "{:02}:{:02}",
            seconds.div_euclid(60),
            seconds.rem_euclid(60)
        );
        let disabled = matches!(self.status, Status::Starting | Status::Finalizing);
        let label = match self.status {
            Status::Starting => "Starting…",
            Status::Finalizing => "Finishing…",
            Status::Idle | Status::Recording => "Stop",
        };
        shell
            .justify_center()
            .child(div().size(px(9.0)).rounded_full().bg(rgb(ACCENT)))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(time),
            )
            .child(
                div()
                    .id("stop")
                    .px_4()
                    .py_2()
                    .rounded_full()
                    .bg(rgb(if disabled { CONTROL } else { ACCENT }))
                    .opacity(if disabled { 0.55 } else { 1.0 })
                    .when(!disabled, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|app, _, _, cx| app.stop(cx)))
                    })
                    .child(label),
            )
    }
}

struct SelectionOverlay {
    mode: Option<Mode>,
    display: Display,
    windows: Vec<CaptureWindow>,
    controller: Entity<CaptureApp>,
    selection: Rc<SelectionState>,
    visual: bool,
    region_drag: Option<RegionDrag>,
    region_inputs: RegionInputs,
}

struct RegionInputs {
    x: Entity<NumericInput>,
    y: Entity<NumericInput>,
    width: Entity<NumericInput>,
    height: Entity<NumericInput>,
}

#[derive(Clone, Copy)]
enum RegionField {
    X,
    Y,
    Width,
    Height,
}

#[derive(Clone, Copy)]
enum RegionDrag {
    Create {
        start: Point<Pixels>,
    },
    Move {
        pointer_start: Point<Pixels>,
        original: Bounds<Pixels>,
    },
    Resize {
        handle: ResizeHandle,
        pointer_start: Point<Pixels>,
        original: Bounds<Pixels>,
    },
}

#[derive(Clone, Copy)]
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

impl SelectionOverlay {
    fn new(
        display: Display,
        windows: Vec<CaptureWindow>,
        controller: Entity<CaptureApp>,
        selection: Rc<SelectionState>,
        visual: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let region_inputs = RegionInputs {
            x: region_input("X", RegionField::X, cx),
            y: region_input("Y", RegionField::Y, cx),
            width: region_input("W", RegionField::Width, cx),
            height: region_input("H", RegionField::Height, cx),
        };
        Self {
            mode: selection.mode.get(),
            display,
            windows,
            controller,
            selection,
            visual,
            region_drag: None,
            region_inputs,
        }
    }

    fn begin_region(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.mode.get() != Some(Mode::Region) {
            return;
        }
        let existing = self
            .selection
            .region
            .get()
            .and_then(|(display_id, x, y, width, height)| {
                (display_id == self.display.id()).then(|| bounds_from_f64(x, y, width, height))
            });
        self.region_drag = Some(existing.map_or(
            RegionDrag::Create {
                start: event.position,
            },
            |original| {
                region_handle_at(event.position, original).map_or_else(
                    || {
                        if original.contains(&event.position) {
                            RegionDrag::Move {
                                pointer_start: event.position,
                                original,
                            }
                        } else {
                            RegionDrag::Create {
                                start: event.position,
                            }
                        }
                    },
                    |handle| RegionDrag::Resize {
                        handle,
                        pointer_start: event.position,
                        original,
                    },
                )
            },
        ));
        if matches!(self.region_drag, Some(RegionDrag::Create { .. })) {
            self.selection.region.set(None);
        }
        cx.notify();
    }

    fn drag_region(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(operation) = self.region_drag {
            let bounds = rounded_bounds(match operation {
                RegionDrag::Create { start } => normalized_bounds(start, event.position),
                RegionDrag::Move {
                    pointer_start,
                    original,
                } => moved_bounds(
                    original,
                    event.position,
                    pointer_start,
                    window.viewport_size(),
                ),
                RegionDrag::Resize {
                    handle,
                    pointer_start,
                    original,
                } => resized_bounds(
                    original,
                    handle,
                    event.position,
                    pointer_start,
                    window.viewport_size(),
                ),
            });
            self.selection.region.set(Some((
                self.display.id(),
                f64::from(bounds.origin.x),
                f64::from(bounds.origin.y),
                f64::from(bounds.size.width),
                f64::from(bounds.size.height),
            )));
            self.controller
                .update(cx, |app, cx| app.refresh_selection_windows(cx));
        }
    }

    fn finish_region(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(_) = self.region_drag.take() else {
            return;
        };
        let Some((display_id, x, y, width, height)) = self.selection.region.get() else {
            return;
        };
        let bounds = rounded_bounds(bounds_from_f64(x, y, width, height));
        if bounds.size.width < px(16.0) || bounds.size.height < px(16.0) {
            self.selection.region.set(None);
            self.controller
                .update(cx, |app, cx| app.refresh_selection_windows(cx));
            cx.notify();
            return;
        }
        self.controller
            .update(cx, |app, cx| app.select_region(display_id, bounds, cx));
        cx.notify();
    }

    fn hover_window(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hovered = self.topmost_window_at(event.position, window);
        if hovered != self.selection.hovered_window.get() {
            self.selection.hovered_window.set(hovered);
            self.controller
                .update(cx, |app, cx| app.refresh_selection_windows(cx));
        }
    }

    fn select_hovered_window(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(window_id) = self.selection.hovered_window.get() else {
            return;
        };
        self.controller
            .update(cx, |app, cx| app.select_window(window_id, cx));
        cx.notify();
    }

    fn start_display_recording(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let display_id = self.display.id();
        self.controller
            .update(cx, |app, cx| app.select_display(display_id, cx));
        self.defer_recording(cx);
    }

    fn start_window_recording(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(window_id) = self.selection.hovered_window.get() else {
            return;
        };
        self.controller
            .update(cx, |app, cx| app.select_window(window_id, cx));
        self.defer_recording(cx);
    }

    fn start_region_recording(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection.region.get().is_some() {
            self.defer_recording(cx);
        }
    }

    fn set_region_field(&mut self, field: RegionField, value: f32, cx: &mut Context<Self>) {
        let Some((display_id, mut x, mut y, mut width, mut height)) = self.selection.region.get()
        else {
            return;
        };
        let (display_width, display_height) = self.display.logical_size();
        let (Ok(display_width), Ok(display_height)) =
            (i32::try_from(display_width), i32::try_from(display_height))
        else {
            return;
        };
        let display_width = f64::from(display_width);
        let display_height = f64::from(display_height);
        let value = f64::from(value.round());
        match field {
            RegionField::X => x = value.max(0.0).min((display_width - width).max(0.0)),
            RegionField::Y => y = value.max(0.0).min((display_height - height).max(0.0)),
            RegionField::Width => width = value.max(1.0).min((display_width - x).max(1.0)),
            RegionField::Height => height = value.max(1.0).min((display_height - y).max(1.0)),
        }
        let bounds = bounds_from_f64(x, y, width, height);
        self.controller
            .update(cx, |app, cx| app.select_region(display_id, bounds, cx));
    }

    fn defer_recording(&self, cx: &mut Context<Self>) {
        let controller = self.controller.clone();
        cx.defer(move |cx| {
            controller.update(cx, CaptureApp::record);
        });
    }

    fn topmost_window_at(&self, position: Point<Pixels>, overlay: &Window) -> Option<u32> {
        let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
        let window_ids = create_window_list(options, kCGNullWindowID)?;
        let origin = overlay.bounds().origin;
        window_ids.iter().find_map(|window_id| {
            self.windows
                .iter()
                .find(|window| window.id() == *window_id)
                .filter(|window| {
                    window_bounds(window, f64::from(origin.x), f64::from(origin.y))
                        .contains(&position)
                })
                .map(CaptureWindow::id)
        })
    }
}

impl Render for SelectionOverlay {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.selection.mode.get();
        if mode != self.mode {
            self.mode = mode;
            self.region_drag = None;
            self.selection.hovered_window.set(None);
        }
        let selected_display = self.selection.display.get();
        let selected_region = self.selection.region.get();
        let selected = selected_display == self.display.id();
        let recording = self.selection.recording.get();
        let background = if self.visual {
            match mode {
                Some(Mode::Display) if recording && selected => 0x0000_0000,
                Some(Mode::Display) if recording => OVERLAY_BLACK,
                Some(Mode::Display) => OVERLAY_BLUE_TINT,
                Some(Mode::Region | Mode::Window) | None => 0x0000_0000,
            }
        } else {
            0x0000_0000
        };
        let mut root = div()
            .size_full()
            .relative()
            .bg(rgba(background))
            .when(mode == Some(Mode::Display) && !recording, |root| {
                let (width, height) = self.display.logical_size();
                let name = if self.display.is_main() {
                    "Main Display".into()
                } else {
                    format!("Display {}", self.display.id())
                };
                let button = overlay_start_button("start-display").when(!self.visual, |button| {
                    button.on_click(cx.listener(Self::start_display_recording))
                });
                root.child(centered_overlay_card(
                    name,
                    format!("{width} × {height}"),
                    button,
                    self.visual,
                ))
            })
            .when(mode == Some(Mode::Region) && !self.visual, |root| {
                root.cursor_crosshair()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_region))
                    .on_mouse_move(cx.listener(Self::drag_region))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_region))
            })
            .when(mode == Some(Mode::Window) && !self.visual, |root| {
                root.cursor_pointer()
                    .on_mouse_move(cx.listener(Self::hover_window))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::select_hovered_window))
            });

        if mode == Some(Mode::Window) {
            let overlay_origin = window.bounds().origin;
            let overlay_x = f64::from(overlay_origin.x);
            let overlay_y = f64::from(overlay_origin.y);
            let active_window = (if recording {
                self.selection.window.get()
            } else {
                self.selection
                    .hovered_window
                    .get()
                    .or(self.selection.window.get())
            })
            .and_then(|window_id| self.windows.iter().find(|window| window.id() == window_id));
            if let Some(target) = active_window {
                let bounds = window_bounds(target, overlay_x, overlay_y);
                let application = target
                    .application()
                    .map_or_else(|| "Unknown app".into(), |app| app.name());
                let title = target.title().unwrap_or_else(|| "Untitled window".into());
                let (width, height) = target.logical_size();
                let button = overlay_start_button("start-window").when(!self.visual, |button| {
                    button.on_click(cx.listener(Self::start_window_recording))
                });
                if self.visual {
                    root = root.children(dim_around(bounds, window.viewport_size()));
                }
                if !recording {
                    root = root.child(
                        div()
                            .absolute()
                            .left(bounds.origin.x)
                            .top(bounds.origin.y)
                            .w(bounds.size.width)
                            .h(bounds.size.height)
                            .bg(rgba(if self.visual {
                                OVERLAY_BLUE_TINT
                            } else {
                                0x0000_0000
                            }))
                            .child(centered_overlay_card(
                                format!("{application} — {title}"),
                                format!("{width:.0} × {height:.0}"),
                                button,
                                self.visual,
                            )),
                    );
                }
            } else if self.visual {
                root = root.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(rgba(OVERLAY_BLACK)),
                );
            }
        }

        if mode == Some(Mode::Region) {
            let region = selected_region.and_then(|(display_id, x, y, width, height)| {
                (display_id == self.display.id()).then(|| bounds_from_f64(x, y, width, height))
            });
            if let Some(bounds) = region {
                if self.visual {
                    root = root.children(dim_around(bounds, window.viewport_size()));
                }
                if self.visual && !recording {
                    root = root.child(
                        div()
                            .absolute()
                            .left(bounds.origin.x)
                            .top(bounds.origin.y)
                            .w(bounds.size.width)
                            .h(bounds.size.height)
                            .border_2()
                            .border_dashed()
                            .border_color(rgb(0x00ff_ffff)),
                    );
                }
                if !recording {
                    if !self.visual {
                        root = root.child(
                            div()
                                .absolute()
                                .left(bounds.origin.x)
                                .top(bounds.origin.y)
                                .w(bounds.size.width)
                                .h(bounds.size.height)
                                .cursor(
                                    if matches!(self.region_drag, Some(RegionDrag::Move { .. })) {
                                        CursorStyle::ClosedHand
                                    } else {
                                        CursorStyle::OpenHand
                                    },
                                ),
                        );
                    }
                    root = root.children(region_resize_handles(bounds, self.visual));
                    sync_numeric_input(&self.region_inputs.x, bounds.origin.x, window, cx);
                    sync_numeric_input(&self.region_inputs.y, bounds.origin.y, window, cx);
                    sync_numeric_input(&self.region_inputs.width, bounds.size.width, window, cx);
                    sync_numeric_input(&self.region_inputs.height, bounds.size.height, window, cx);
                    let button = overlay_start_button("start-region")
                        .when(!self.visual, |button| {
                            button.on_click(cx.listener(Self::start_region_recording))
                        });
                    root = root.child(region_overlay_card(
                        bounds,
                        window.viewport_size(),
                        &self.region_inputs,
                        button,
                        self.visual,
                    ));
                }
            } else if self.visual {
                root = root.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(rgba(OVERLAY_BLACK)),
                );
            }
        }
        root
    }
}

#[allow(clippy::arithmetic_side_effects)]
fn dim_around(bounds: Bounds<Pixels>, viewport: gpui::Size<Pixels>) -> Vec<Div> {
    let zero = px(0.0);
    let left = bounds.origin.x.max(zero).min(viewport.width);
    let top = bounds.origin.y.max(zero).min(viewport.height);
    let right = bounds.right().max(left).min(viewport.width);
    let bottom = bounds.bottom().max(top).min(viewport.height);
    vec![
        mask_rect(zero, zero, viewport.width, top),
        mask_rect(zero, bottom, viewport.width, viewport.height - bottom),
        mask_rect(zero, top, left, bottom - top),
        mask_rect(right, top, viewport.width - right, bottom - top),
    ]
}

fn mask_rect(left: Pixels, top: Pixels, width: Pixels, height: Pixels) -> Div {
    div()
        .absolute()
        .left(left)
        .top(top)
        .w(width)
        .h(height)
        .bg(rgba(OVERLAY_BLACK))
}

fn centered_overlay_card(
    name: String,
    resolution: String,
    button: impl IntoElement,
    visible: bool,
) -> Div {
    div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .flex()
        .items_center()
        .justify_center()
        .opacity(if visible { 1.0 } else { 0.0 })
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .px_5()
                .py_4()
                .rounded_lg()
                .bg(rgba(0x1011_14d0))
                .text_color(rgb(TEXT))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(div().text_sm().text_color(rgb(MUTED)).child(resolution))
                .child(button),
        )
}

#[allow(clippy::arithmetic_side_effects)]
fn region_overlay_card(
    selection: Bounds<Pixels>,
    viewport: gpui::Size<Pixels>,
    inputs: &RegionInputs,
    button: impl IntoElement,
    visible: bool,
) -> Div {
    let width = px(272.0);
    let height = px(136.0);
    let gap = px(10.0);
    let margin = px(8.0);
    let max_left = (viewport.width - width - margin).max(margin);
    let left = (selection.center().x - width / 2.0)
        .max(margin)
        .min(max_left);
    let below = selection.bottom() + gap;
    let top = if below + height + margin <= viewport.height {
        below
    } else {
        (selection.bottom() - height - gap).max(margin)
    };
    div()
        .absolute()
        .left(left)
        .top(top)
        .w(width)
        .h(height)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_3()
        .rounded_lg()
        .bg(rgba(if visible { 0x1011_14d0 } else { 0x0000_0000 }))
        .text_color(rgba(if visible { 0xffff_ffff } else { 0x0000_0000 }))
        .capture_any_mouse_down(|_, window, _| window.activate_window())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(region_input_row(
            "Location",
            inputs.x.clone(),
            inputs.y.clone(),
        ))
        .child(region_input_row(
            "Size",
            inputs.width.clone(),
            inputs.height.clone(),
        ))
        .child(button)
}

fn region_input_row(
    label: &'static str,
    first: Entity<NumericInput>,
    second: Entity<NumericInput>,
) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(56.0))
                .flex_none()
                .text_xs()
                .whitespace_nowrap()
                .text_color(rgb(MUTED))
                .child(label),
        )
        .child(div().w(px(82.0)).flex_none().child(first))
        .child(div().w(px(82.0)).flex_none().child(second))
}

fn sync_numeric_input(
    input: &Entity<NumericInput>,
    value: Pixels,
    window: &Window,
    cx: &mut Context<SelectionOverlay>,
) {
    let focused = input.read(cx).focus_handle().is_focused(window);
    input.update(cx, |input, cx| {
        input.set_value(value.as_f32().round(), focused, cx);
    });
}

fn region_input(
    label: &'static str,
    field: RegionField,
    cx: &mut Context<SelectionOverlay>,
) -> Entity<NumericInput> {
    let input = cx.new(|cx| NumericInput::new(label, cx));
    cx.subscribe(&input, move |overlay, _, event: &NumericInputEvent, cx| {
        if let NumericInputEvent::Changed(value) = event {
            overlay.set_region_field(field, *value, cx);
        }
    })
    .detach();
    input
}

fn overlay_start_button(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .rounded_sm()
        .border_1()
        .border_color(rgb(0x00d2_d2d2))
        .bg(rgb(0x00d2_d2d2))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(0x0017_1717))
        .hover(|button| button.opacity(0.9))
        .active(|button| button.opacity(0.72))
        .cursor_pointer()
        .child("Start Recording")
}

fn overlay_secondary_button(id: &'static str, label: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .rounded_sm()
        .border_1()
        .border_color(rgb(0x0024_2424))
        .bg(rgb(0x0009_0909))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT))
        .hover(|button| button.bg(rgb(0x0015_1515)))
        .active(|button| button.opacity(0.72))
        .cursor_pointer()
        .child(label)
}

#[allow(clippy::arithmetic_side_effects)]
fn normalized_bounds(a: Point<Pixels>, b: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(a.x.min(b.x), a.y.min(b.y)),
        size((a.x - b.x).abs(), (a.y - b.y).abs()),
    )
}

fn rounded_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(
            px(bounds.origin.x.as_f32().round()),
            px(bounds.origin.y.as_f32().round()),
        ),
        size(
            px(bounds.size.width.as_f32().round()),
            px(bounds.size.height.as_f32().round()),
        ),
    )
}

#[allow(clippy::arithmetic_side_effects)]
fn moved_bounds(
    original: Bounds<Pixels>,
    pointer: Point<Pixels>,
    pointer_start: Point<Pixels>,
    viewport: gpui::Size<Pixels>,
) -> Bounds<Pixels> {
    let zero = px(0.0);
    let max_x = (viewport.width - original.size.width).max(zero);
    let max_y = (viewport.height - original.size.height).max(zero);
    Bounds::new(
        point(
            (original.origin.x + pointer.x - pointer_start.x)
                .max(zero)
                .min(max_x),
            (original.origin.y + pointer.y - pointer_start.y)
                .max(zero)
                .min(max_y),
        ),
        original.size,
    )
}

fn region_handle_points(bounds: Bounds<Pixels>) -> [(ResizeHandle, Point<Pixels>); 8] {
    let center = bounds.center();
    [
        (ResizeHandle::TopLeft, point(bounds.left(), bounds.top())),
        (ResizeHandle::Top, point(center.x, bounds.top())),
        (ResizeHandle::TopRight, point(bounds.right(), bounds.top())),
        (ResizeHandle::Right, point(bounds.right(), center.y)),
        (
            ResizeHandle::BottomRight,
            point(bounds.right(), bounds.bottom()),
        ),
        (ResizeHandle::Bottom, point(center.x, bounds.bottom())),
        (
            ResizeHandle::BottomLeft,
            point(bounds.left(), bounds.bottom()),
        ),
        (ResizeHandle::Left, point(bounds.left(), center.y)),
    ]
}

const fn resize_handle_cursor(handle: ResizeHandle) -> CursorStyle {
    match handle {
        ResizeHandle::TopLeft | ResizeHandle::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeHandle::TopRight | ResizeHandle::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        ResizeHandle::Top | ResizeHandle::Bottom => CursorStyle::ResizeUpDown,
        ResizeHandle::Left | ResizeHandle::Right => CursorStyle::ResizeLeftRight,
    }
}

#[allow(clippy::arithmetic_side_effects)]
fn region_handle_at(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<ResizeHandle> {
    let radius = px(9.0);
    region_handle_points(bounds)
        .into_iter()
        .find(|(_, handle)| {
            (position.x - handle.x).abs() <= radius && (position.y - handle.y).abs() <= radius
        })
        .map(|(handle, _)| handle)
}

#[allow(clippy::arithmetic_side_effects)]
fn region_resize_handles(bounds: Bounds<Pixels>, visible: bool) -> Vec<Div> {
    let handle_size = px(10.0);
    region_handle_points(bounds)
        .into_iter()
        .map(|(handle, position)| {
            div()
                .absolute()
                .left(position.x - handle_size / 2.0)
                .top(position.y - handle_size / 2.0)
                .size(handle_size)
                .rounded_sm()
                .bg(rgba(if visible { 0xffff_ffff } else { 0x0000_0000 }))
                .cursor(resize_handle_cursor(handle))
        })
        .collect()
}

#[allow(clippy::arithmetic_side_effects)]
fn resized_bounds(
    original: Bounds<Pixels>,
    handle: ResizeHandle,
    pointer: Point<Pixels>,
    pointer_start: Point<Pixels>,
    viewport: gpui::Size<Pixels>,
) -> Bounds<Pixels> {
    let zero = px(0.0);
    let minimum = px(16.0);
    let delta_x = pointer.x - pointer_start.x;
    let delta_y = pointer.y - pointer_start.y;
    let mut left = original.left();
    let mut right = original.right();
    let mut top = original.top();
    let mut bottom = original.bottom();
    if matches!(
        handle,
        ResizeHandle::TopLeft | ResizeHandle::Left | ResizeHandle::BottomLeft
    ) {
        left = (left + delta_x).max(zero).min(right - minimum);
    }
    if matches!(
        handle,
        ResizeHandle::TopRight | ResizeHandle::Right | ResizeHandle::BottomRight
    ) {
        right = (right + delta_x).max(left + minimum).min(viewport.width);
    }
    if matches!(
        handle,
        ResizeHandle::TopLeft | ResizeHandle::Top | ResizeHandle::TopRight
    ) {
        top = (top + delta_y).max(zero).min(bottom - minimum);
    }
    if matches!(
        handle,
        ResizeHandle::BottomLeft | ResizeHandle::Bottom | ResizeHandle::BottomRight
    ) {
        bottom = (bottom + delta_y).max(top + minimum).min(viewport.height);
    }
    Bounds::new(point(left, top), size(right - left, bottom - top))
}

#[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
fn bounds_from_f64(x: f64, y: f64, width: f64, height: f64) -> Bounds<Pixels> {
    Bounds::new(
        point(px(x as f32), px(y as f32)),
        size(px(width as f32), px(height as f32)),
    )
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation
)]
fn window_bounds(window: &CaptureWindow, display_x: f64, display_y: f64) -> Bounds<Pixels> {
    let (x, y, width, height) = window.frame();
    Bounds::new(
        point(px((x - display_x) as f32), px((y - display_y) as f32)),
        size(px(width as f32), px(height as f32)),
    )
}

fn topmost_application_pid(windows: &[CaptureWindow]) -> Option<i32> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let window_ids = create_window_list(options, kCGNullWindowID)?;
    let own_process_id = i32::try_from(std::process::id()).ok()?;
    window_ids.iter().find_map(|window_id| {
        windows
            .iter()
            .find(|window| window.id() == *window_id)
            .and_then(CaptureWindow::application)
            .map(|application| application.process_id())
            .filter(|process_id| *process_id != own_process_id)
    })
}

fn selection_window_options(display_id: gpui::DisplayId, bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id: Some(display_id),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        focus: false,
        ..Default::default()
    }
}

fn configure_selection_window(window: &Window, visual: bool) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points to the live NSView owned by this main-thread window.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(window) = view.window() else {
        return;
    };
    window.setStyleMask(if visual {
        NSWindowStyleMask::NonactivatingPanel
    } else {
        NSWindowStyleMask::empty()
    });
    window.setHasShadow(false);
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    // SAFETY: CoreGraphics accepts this documented window-level key without additional state.
    let maximum_level = unsafe { CGWindowLevelForKey(CG_MAXIMUM_WINDOW_LEVEL_KEY) };
    let Ok(level) = isize::try_from(maximum_level.saturating_sub(if visual { 2 } else { 3 }))
    else {
        return;
    };
    window.setLevel(level);
    window.setIgnoresMouseEvents(visual);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenPrimary,
    );
    if let Some(screen) = window.screen() {
        window.setFrame_display(screen.frame(), true);
    }
    window.orderFrontRegardless();
}

fn configure_toolbar_window(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points to the live NSView owned by this main-thread window.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(window) = view.window() else {
        return;
    };
    window.setStyleMask(NSWindowStyleMask::NonactivatingPanel);
    window.setHasShadow(false);
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    // SAFETY: CoreGraphics accepts this documented window-level key without additional state.
    let maximum_level = unsafe { CGWindowLevelForKey(CG_MAXIMUM_WINDOW_LEVEL_KEY) };
    let Ok(level) = isize::try_from(maximum_level.saturating_sub(1)) else {
        return;
    };
    window.setLevel(level);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    window.orderFrontRegardless();
}

#[allow(clippy::arithmetic_side_effects)]
#[allow(clippy::as_conversions)]
fn schedule_destination_menu(
    window: &Window,
    selected: SaveDestination,
    open_finder_after_recording: bool,
    sender: async_channel::Sender<DestinationMenuAction>,
) -> Result<(), ()> {
    let handle = HasWindowHandle::window_handle(window).map_err(|_| ())?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(());
    };
    let view_address = handle.ns_view.as_ptr() as usize;
    DispatchQueue::main().exec_async(move || {
        let Some(view) = NonNull::new(view_address as *mut c_void) else {
            return;
        };
        native_destination_menu(view, selected, open_finder_after_recording, sender);
    });
    Ok(())
}

#[allow(clippy::arithmetic_side_effects)]
fn native_destination_menu(
    view: NonNull<c_void>,
    selected: SaveDestination,
    open_finder_after_recording: bool,
    sender: async_channel::Sender<DestinationMenuAction>,
) -> Option<()> {
    // SAFETY: GPUI's AppKit handle points to the live NSView owned by this main-thread window.
    let view = unsafe { view.cast::<NSView>().as_ref() };
    let main_thread = MainThreadMarker::new()?;
    let view_bounds = view.bounds();
    let menu = NSMenu::new(main_thread);
    menu.setAutoenablesItems(false);
    menu.setMinimumWidth(112.0);
    let handler = DestinationMenuHandler::new(sender);
    let empty = NSString::from_str("");
    for (index, destination) in SaveDestination::ALL.into_iter().enumerate() {
        // SAFETY: The handler implements `selectDestination:` with the NSMenuItem signature.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(main_thread),
                &NSString::from_str(destination.label()),
                Some(sel!(selectDestination:)),
                &empty,
            )
        };
        item.setTag(isize::try_from(index).ok()?);
        // SAFETY: `handler` implements the selector configured above and outlives menu tracking.
        unsafe { item.setTarget(Some(&handler)) };
        if destination == selected {
            item.setState(NSControlStateValueOn);
        }
        menu.addItem(&item);
    }
    menu.addItem(&NSMenuItem::separatorItem(main_thread));
    // SAFETY: The handler implements `toggleOpenFinder:` with the NSMenuItem signature.
    let open_finder_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(main_thread),
            &NSString::from_str("Open Finder After Recording"),
            Some(sel!(toggleOpenFinder:)),
            &empty,
        )
    };
    // SAFETY: `handler` implements the selector configured above and outlives menu tracking.
    unsafe { open_finder_item.setTarget(Some(&handler)) };
    if open_finder_after_recording {
        open_finder_item.setState(NSControlStateValueOn);
    }
    menu.addItem(&open_finder_item);
    menu.update();
    let menu_size = menu.size();
    let location = NSPoint::new(
        316.0,
        view_bounds.size.height.midpoint(30.0) + 6.0 + menu_size.height,
    );
    menu.popUpMenuPositioningItem_atLocation_inView(None, location, Some(view));
    Some(())
}

#[allow(clippy::arithmetic_side_effects)]
fn toolbar_options(cx: &App) -> WindowOptions {
    let toolbar_size = size(px(TOOLBAR_SIZE.0), px(TOOLBAR_SIZE.1));
    let bounds = cx.primary_display().map_or_else(
        || Bounds::centered(None, toolbar_size, cx),
        |display| {
            let screen = display.bounds();
            Bounds::new(
                point(
                    screen.origin.x + (screen.size.width - toolbar_size.width) / 2.0,
                    screen.origin.y + screen.size.height - toolbar_size.height - px(48.0),
                ),
                toolbar_size,
            )
        },
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Blurred,
        kind: WindowKind::PopUp,
        is_movable: true,
        app_owns_titlebar_drag: true,
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    }
}

fn output_path(destination: SaveDestination) -> PathBuf {
    let name = format!(
        "Blip Capture {}.mp4",
        Local::now().format("%Y-%m-%d at %H.%M.%S")
    );
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    match destination {
        SaveDestination::Desktop => home.join("Desktop"),
        SaveDestination::Documents => home.join("Documents"),
        SaveDestination::Downloads => home.join("Downloads"),
        SaveDestination::Clipboard => std::env::temp_dir().join("blip-capture"),
    }
    .join(name)
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

type EventHandler = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[derive(Clone, Copy)]
enum HotkeyAction {
    Reopen,
    Escape,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandler,
        event_type_count: u32,
        event_types: *const EventTypeSpec,
        user_data: *mut c_void,
        handler_ref: *mut *mut c_void,
    ) -> i32;
    fn GetApplicationEventTarget() -> *mut c_void;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hotkey_id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        hotkey_ref: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey_ref: *mut c_void) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: u32,
        actual_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

unsafe extern "C" fn hotkey_handler(
    _: *mut c_void,
    event: *mut c_void,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: Registration stores this boxed sender for the process lifetime.
    let sender = unsafe { &*user_data.cast::<async_channel::Sender<HotkeyAction>>() };
    let mut hotkey_id = EventHotKeyId {
        signature: 0,
        id: 0,
    };
    // SAFETY: Carbon writes an EventHotKeyId into the correctly sized output buffer.
    let status = unsafe {
        GetEventParameter(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"hkid"),
            ptr::null_mut(),
            u32::try_from(std::mem::size_of::<EventHotKeyId>()).unwrap_or_default(),
            ptr::null_mut(),
            (&raw mut hotkey_id).cast(),
        )
    };
    if status == 0 {
        let action = match hotkey_id.id {
            1 => Some(HotkeyAction::Reopen),
            2 => Some(HotkeyAction::Escape),
            _ => None,
        };
        if let Some(action) = action {
            sender.try_send(action).ok();
        }
    }
    0
}

fn register_reopen_hotkey(sender: async_channel::Sender<HotkeyAction>) -> Result<(), i32> {
    const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const EVENT_HOTKEY_PRESSED: u32 = 5;
    const COMMAND_KEY: u32 = 1 << 8;
    const SHIFT_KEY: u32 = 1 << 9;
    const KEY_8: u32 = 0x1c;

    let sender = Box::into_raw(Box::new(sender));
    let event_type = EventTypeSpec {
        event_class: EVENT_CLASS_KEYBOARD,
        event_kind: EVENT_HOTKEY_PRESSED,
    };
    let mut handler_ref = ptr::null_mut();
    // SAFETY: Carbon copies the event specification and retains the process-lifetime user data.
    let install_status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            hotkey_handler,
            1,
            &raw const event_type,
            sender.cast(),
            &raw mut handler_ref,
        )
    };
    if install_status != 0 {
        // SAFETY: Installation failed, so Carbon did not retain the sender.
        drop(unsafe { Box::from_raw(sender) });
        return Err(install_status);
    }

    let mut hotkey_ref = ptr::null_mut();
    // SAFETY: The application event target and handler remain alive for the process lifetime.
    let register_status = unsafe {
        RegisterEventHotKey(
            KEY_8,
            COMMAND_KEY | SHIFT_KEY,
            EventHotKeyId {
                signature: u32::from_be_bytes(*b"BLIP"),
                id: 1,
            },
            GetApplicationEventTarget(),
            0,
            &raw mut hotkey_ref,
        )
    };
    if register_status != 0 {
        return Err(register_status);
    }
    Ok(())
}

fn register_escape_hotkey() -> Result<*mut c_void, i32> {
    const KEY_ESCAPE: u32 = 0x35;
    let mut hotkey_ref = ptr::null_mut();
    // SAFETY: The application event target and installed handler live for the process lifetime.
    let status = unsafe {
        RegisterEventHotKey(
            KEY_ESCAPE,
            0,
            EventHotKeyId {
                signature: u32::from_be_bytes(*b"BLIP"),
                id: 2,
            },
            GetApplicationEventTarget(),
            0,
            &raw mut hotkey_ref,
        )
    };
    if status == 0 {
        Ok(hotkey_ref)
    } else {
        Err(status)
    }
}

fn unregister_escape_hotkey(hotkey: &Cell<*mut c_void>) {
    let hotkey_ref = hotkey.replace(ptr::null_mut());
    if !hotkey_ref.is_null() {
        // SAFETY: This reference came from a successful RegisterEventHotKey call.
        unsafe { UnregisterEventHotKey(hotkey_ref) };
    }
}

fn open_capture(
    cx: &mut App,
    visible: &Rc<Cell<bool>>,
    escape_hotkey: &Rc<Cell<*mut c_void>>,
    target_cache: &Rc<RefCell<Option<CaptureTargets>>>,
) {
    if visible.get() {
        return;
    }
    if !blip_sck::has_permission() {
        let _ = blip_sck::request_permission();
        eprintln!("blip-capture: {}", CaptureError::PermissionDenied);
        return;
    }
    let targets = if let Some(targets) = target_cache.borrow().clone() {
        targets
    } else {
        let content = match ShareableContent::current(CAPTURE_TIMEOUT) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("blip-capture: {error}");
                return;
            }
        };
        let targets = CaptureTargets::from_content(&content);
        target_cache.replace(Some(targets.clone()));
        targets
    };
    let options = toolbar_options(cx);
    let app = match cx.open_window(options, |window, cx| {
        let window_handle = Window::window_handle(window);
        cx.new(|cx| {
            CaptureApp::new(
                &targets,
                window_handle,
                Rc::clone(visible),
                Rc::clone(escape_hotkey),
                cx,
            )
        })
    }) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("blip-capture: failed to open toolbar: {error}");
            return;
        }
    };
    match register_escape_hotkey() {
        Ok(hotkey) => escape_hotkey.set(hotkey),
        Err(status) => eprintln!("blip-capture: failed to register Escape ({status})"),
    }
    visible.set(true);
    app.update(cx, |app, window, cx| {
        configure_toolbar_window(window);
        app.open_selection_windows(window, cx);
    })
    .ok();
    cx.activate(true);
}

fn handle_escape(cx: &mut App) {
    for window in cx.windows() {
        let Some(controller) = window.downcast::<CaptureApp>() else {
            continue;
        };
        controller.update(cx, CaptureApp::escape).ok();
        break;
    }
}

fn main() {
    application().run(|cx| {
        NumericInput::bind_keys(cx);
        let visible = Rc::new(Cell::new(false));
        let escape_hotkey = Rc::new(Cell::new(ptr::null_mut()));
        let target_cache = Rc::new(RefCell::new(None));
        let (hotkey_sender, hotkey_receiver) = async_channel::unbounded();
        if let Err(status) = register_reopen_hotkey(hotkey_sender) {
            eprintln!("blip-capture: failed to register Cmd-Shift-8 ({status})");
        }
        let hotkey_visible = Rc::clone(&visible);
        let hotkey_escape = Rc::clone(&escape_hotkey);
        let hotkey_target_cache = Rc::clone(&target_cache);
        cx.spawn(async move |cx| {
            while let Ok(action) = hotkey_receiver.recv().await {
                match action {
                    HotkeyAction::Reopen => {
                        let visible = Rc::clone(&hotkey_visible);
                        let escape_hotkey = Rc::clone(&hotkey_escape);
                        let target_cache = Rc::clone(&hotkey_target_cache);
                        cx.update(|cx| {
                            open_capture(cx, &visible, &escape_hotkey, &target_cache);
                        });
                    }
                    HotkeyAction::Escape => cx.update(handle_escape),
                }
            }
        })
        .detach();
        open_capture(cx, &visible, &escape_hotkey, &target_cache);
    });
}

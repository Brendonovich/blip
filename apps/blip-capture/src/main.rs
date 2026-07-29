use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::ptr;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use blip_avfoundation::{CameraDevice, list_video_devices, request_camera_access};
use blip_sck::{CaptureError, Display, ShareableContent, Window as CaptureWindow};
use chrono::Local;
use clap::Parser;
use core_foundation::{base::TCFType, number::CFNumber, string::CFString};
use core_graphics::window::{
    create_description_from_array, create_window_list, kCGNullWindowID,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
};
use dispatch2::DispatchQueue;
use gpui::{
    AnyWindowHandle, App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, Div, Entity,
    ExternalPaths, FontWeight, IntoElement, KeyBinding, Menu, MenuItem, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, actions, div, point,
    prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use objc2::rc::Retained;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSControlStateValueOn, NSMenu, NSMenuItem, NSRunningApplication, NSScreen, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSView, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

mod assets;
mod bundle;
mod camera_preview;
mod editor;
mod headless;
#[path = "../../blip-studio/src/numeric_input.rs"]
#[allow(dead_code)]
mod numeric_input;
mod profiles;
mod recording;
mod theme;
mod upload;

use bundle::BlipBundle;
use camera_preview::CameraPreview;
use editor::BundleEditor;
use numeric_input::{NumericInput, NumericInputEvent};
use profiles::{
    CompletionAction, RecordingFormat, RecordingProfile, RecordingProfiles, RecordingTarget,
    join_server_url, split_server_url,
};
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
const CAMERA_PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(250);
const CAMERA_WINDOW_AUTOSAVE_NAME: &str = "Camera Preview";

actions!(
    blip_capture,
    [
        CloseWindow,
        CloseAllWindows,
        TogglePlayback,
        ToggleCutMode,
        DeleteSelected,
        CloseExportDialog
    ]
);

#[allow(clippy::arithmetic_side_effects)]
fn toolbar_dimensions_for_label(label: &str) -> (f32, f32) {
    let char_count = label.chars().count().clamp(3, 30);
    let width = 448.0 + (char_count as f32 * 8.0);
    (width, 56.0)
}
type RegionSelection = (u32, f64, f64, f64, f64);

#[derive(Debug, Parser)]
#[command(name = "blip-capture", about = "Record and share your Mac screen")]
struct CaptureArgs {
    /// Path to a Blip bundle or file to open.
    #[arg(value_name = "PATH", conflicts_with = "headless")]
    path: Option<PathBuf>,

    /// Record and upload without opening the capture interface.
    #[arg(long)]
    headless: bool,

    /// Blip server URL, including the capture key after `#`.
    #[arg(long, value_name = "URL", requires = "headless")]
    server_url: Option<String>,

    /// Display ID reported by `blip-cli displays list`. Defaults to the main display.
    #[arg(long, value_name = "ID", requires = "headless")]
    display: Option<u32>,

    /// Number of seconds to record.
    #[arg(
        long,
        default_value_t = 5,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..),
        requires = "headless"
    )]
    duration: u64,

    /// Recording format to exercise during the upload.
    #[arg(long, value_enum, default_value_t, requires = "headless")]
    format: headless::HeadlessFormat,
}

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
    Uploading,
}

enum DestinationMenuAction {
    SelectProfile(usize),
    ToggleOpenRecording,
    OpenSettings,
}

struct CameraMenuAction(Option<usize>);

struct CameraMenuHandlerIvars {
    sender: async_channel::Sender<CameraMenuAction>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CameraMenuHandlerIvars]
    struct CameraMenuHandler;

    unsafe impl NSObjectProtocol for CameraMenuHandler {}

    impl CameraMenuHandler {
        #[unsafe(method(selectCamera:))]
        fn select_camera(&self, item: &NSMenuItem) {
            let selected = usize::try_from(item.tag())
                .ok()
                .and_then(|tag| tag.checked_sub(1));
            self.ivars()
                .sender
                .try_send(CameraMenuAction(selected))
                .ok();
        }
    }
);

impl CameraMenuHandler {
    fn new(sender: async_channel::Sender<CameraMenuAction>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CameraMenuHandlerIvars { sender });
        // SAFETY: The object has fully initialized ivars and NSObject permits `init`.
        unsafe { msg_send![super(this), init] }
    }
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
            self.ivars()
                .sender
                .try_send(DestinationMenuAction::SelectProfile(index))
                .ok();
        }

        #[unsafe(method(toggleOpenRecording:))]
        fn toggle_open_recording(&self, _: &NSMenuItem) {
            self.ivars()
                .sender
                .try_send(DestinationMenuAction::ToggleOpenRecording)
                .ok();
        }

        #[unsafe(method(openProfileSettings:))]
        fn open_profile_settings(&self, _: &NSMenuItem) {
            self.ivars()
                .sender
                .try_send(DestinationMenuAction::OpenSettings)
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

#[derive(Clone, Copy)]
enum MenuBarAction {
    NewRecording,
    CheckForUpdates,
    Quit,
}

struct MenuBarHandlerIvars {
    sender: async_channel::Sender<MenuBarAction>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = MenuBarHandlerIvars]
    struct MenuBarHandler;

    unsafe impl NSObjectProtocol for MenuBarHandler {}

    impl MenuBarHandler {
        #[unsafe(method(newRecording:))]
        fn new_recording(&self, _: &NSMenuItem) {
            self.ivars().sender.try_send(MenuBarAction::NewRecording).ok();
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _: &NSMenuItem) {
            self.ivars().sender.try_send(MenuBarAction::CheckForUpdates).ok();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _: &NSMenuItem) {
            self.ivars().sender.try_send(MenuBarAction::Quit).ok();
        }
    }
);

impl MenuBarHandler {
    fn new(sender: async_channel::Sender<MenuBarAction>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(MenuBarHandlerIvars { sender });
        // SAFETY: The object has fully initialized ivars and NSObject permits `init`.
        unsafe { msg_send![super(this), init] }
    }
}

struct MenuBarItem {
    _item: Retained<NSStatusItem>,
    _handler: Retained<MenuBarHandler>,
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
    recorded_duration: Duration,
    stop_sender: Option<mpsc::Sender<()>>,
    event_sender: async_channel::Sender<RecordingEvent>,
    visible: Rc<Cell<bool>>,
    escape_hotkey: Rc<Cell<*mut c_void>>,
    profiles: RecordingProfiles,
    destination_sender: async_channel::Sender<DestinationMenuAction>,
    cameras: Vec<CameraDevice>,
    selected_camera: Option<usize>,
    camera_sender: async_channel::Sender<CameraMenuAction>,
    camera_window: Option<AnyWindowHandle>,
    camera_window_id: Option<u32>,
    recording_completion_action: CompletionAction,
    open_recording_when_finished: bool,
    drag_start_window_position: Option<Point<Pixels>>,
    error: Option<String>,
}

impl CaptureApp {
    #[allow(clippy::too_many_lines)]
    fn new(
        targets: &CaptureTargets,
        controller_window: AnyWindowHandle,
        visible: Rc<Cell<bool>>,
        escape_hotkey: Rc<Cell<*mut c_void>>,
        profiles: RecordingProfiles,
        cx: &mut Context<Self>,
    ) -> Self {
        let displays = targets.displays.clone();
        let selected_display = 0;
        let windows = targets.windows.clone();
        let selected_window = None;
        let selection = Rc::new(SelectionState {
            mode: Cell::new(None),
            display: Cell::new(selected_display),
            window: Cell::new(selected_window),
            hovered_window: Cell::new(None),
            region: Cell::new(None),
            recording: Cell::new(false),
        });
        let (event_sender, receiver) = async_channel::unbounded();
        let (destination_sender, destination_receiver) = async_channel::unbounded();
        let cameras = list_video_devices().unwrap_or_else(|error| {
            eprintln!("blip-capture: failed to list cameras: {error}");
            Vec::new()
        });
        let (camera_sender, camera_receiver) = async_channel::unbounded();
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
            while let Ok(CameraMenuAction(selected)) = camera_receiver.recv().await {
                if app
                    .update(cx, |app, cx| app.select_camera(selected, cx))
                    .is_err()
                {
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
                            DestinationMenuAction::SelectProfile(index) => {
                                if let Some(profile) = app.profiles.profiles.get(index) {
                                    app.profiles.selected_profile_id = profile.id.clone();
                                    if let Err(error) = app.profiles.save() {
                                        app.error = Some(error);
                                    }
                                    let dimensions = app.idle_toolbar_dimensions();
                                    app.resize_toolbar(dimensions, cx);
                                }
                            }
                            DestinationMenuAction::ToggleOpenRecording => {
                                app.open_recording_when_finished =
                                    !app.open_recording_when_finished;
                            }
                            DestinationMenuAction::OpenSettings => app.open_profile_settings(cx),
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
            mode: None,
            selected_display,
            selected_window,
            region: None,
            selection,
            selection_windows: Vec::new(),
            visual_windows: Vec::new(),
            status: Status::Idle,
            started_at: None,
            recorded_duration: Duration::ZERO,
            stop_sender: None,
            event_sender,
            visible,
            escape_hotkey,
            profiles,
            destination_sender,
            cameras,
            selected_camera: None,
            camera_sender,
            camera_window: None,
            camera_window_id: None,
            recording_completion_action: CompletionAction::None,
            open_recording_when_finished: true,
            drag_start_window_position: None,
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
            self.profiles
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            self.profiles.selected_index(),
            self.open_recording_when_finished,
            self.destination_sender.clone(),
        )
        .is_err()
        {
            self.error = Some("Failed to open the destination menu".into());
            cx.notify();
        }
    }

    fn show_camera_menu(&mut self, window: &Window, cx: &mut Context<Self>) {
        if schedule_camera_menu(
            window,
            self.cameras
                .iter()
                .map(|camera| camera.localized_name().to_owned())
                .collect(),
            self.selected_camera,
            self.camera_sender.clone(),
        )
        .is_err()
        {
            self.error = Some("Failed to open the camera menu".into());
            cx.notify();
        }
    }

    fn select_camera(&mut self, selected: Option<usize>, cx: &mut Context<Self>) {
        self.close_camera_window(cx);
        self.selected_camera = selected;
        let Some(device) = selected.and_then(|index| self.cameras.get(index)).cloned() else {
            cx.notify();
            return;
        };
        let options = camera_window_options(cx);
        let camera_sender = self.camera_sender.clone();
        match cx.open_window(options, move |_, cx| {
            cx.new(|cx| CameraPreview::new(device, camera_sender, cx))
        }) {
            Ok(preview) => {
                self.camera_window_id = preview
                    .update(cx, |_, window, _| configure_camera_window(window))
                    .ok()
                    .flatten();
                self.camera_window = Some(preview.into());
                self.error = None;
            }
            Err(error) => {
                self.selected_camera = None;
                self.error = Some(format!("Failed to open camera preview: {error}"));
            }
        }
        cx.notify();
    }

    fn close_camera_window(&mut self, cx: &mut Context<Self>) {
        self.camera_window_id = None;
        if let Some(preview) = self.camera_window.take() {
            preview
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    }

    fn open_profile_settings(&mut self, cx: &mut Context<Self>) {
        let controller = cx.entity();
        let profiles = self.profiles.clone();
        let options = profile_settings_options(cx);
        set_activation_policy(NSApplicationActivationPolicy::Regular);
        let settings_window = match cx.open_window(options, move |_, cx| {
            cx.new(|cx| ProfileSettings::new(controller, profiles, cx))
        }) {
            Ok(window) => window,
            Err(error) => {
                refresh_activation_policy(cx);
                self.error = Some(format!(
                    "Failed to open recording profile settings: {error}"
                ));
                cx.notify();
                return;
            }
        };

        self.close_selection_windows(cx);
        unregister_escape_hotkey(&self.escape_hotkey);
        self.controller_window
            .update(cx, |_, window, _| {
                set_toolbar_window_visible(window, false);
            })
            .ok();

        let settings_window_id = AnyWindowHandle::from(settings_window).window_id();
        cx.defer(move |cx| {
            cx.activate(true);
            settings_window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        });
        let controller_window = self.controller_window;
        let restored = Rc::new(Cell::new(false));
        cx.on_window_closed(move |cx, window_id| {
            if window_id != settings_window_id || restored.replace(true) {
                return;
            }
            refresh_activation_policy(cx);
            let Some(controller) = controller_window.downcast::<CaptureApp>() else {
                return;
            };
            controller
                .update(cx, |app, window, cx| {
                    app.restore_after_profile_settings(window, cx);
                })
                .ok();
        })
        .detach();
    }

    fn restore_after_profile_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        set_toolbar_window_visible(window, true);
        if self.mode.is_some() {
            self.open_selection_windows(window, cx);
        }
        match register_escape_hotkey() {
            Ok(hotkey) => self.escape_hotkey.set(hotkey),
            Err(status) => {
                self.error = Some(format!("Failed to register Escape ({status})"));
                cx.notify();
            }
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
        let Some(profile) = self.profiles.selected().cloned() else {
            self.error = Some("Create a recording profile first".into());
            cx.notify();
            return;
        };
        if let Err(error) = profile.validate() {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let target_name = self.capture_target_name(spec);
        let output = match output_destination(&profile, &target_name) {
            Ok(output) => output,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let server_url = match &profile.target {
            RecordingTarget::Remote { server_url } => Some(server_url.clone()),
            RecordingTarget::Local { .. } => None,
        };
        let sender = match recording::spawn(
            spec,
            self.camera_window_id,
            output.media_path,
            output.completed_path,
            output.cleanup_on_failure,
            server_url,
            profile.format,
            self.event_sender.clone(),
        ) {
            Ok(sender) => sender,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.recording_completion_action = profile.completion_action;
        unregister_escape_hotkey(&self.escape_hotkey);
        self.selection.recording.set(true);
        self.refresh_selection_windows(cx);
        self.close_interaction_windows(cx);
        self.stop_sender = Some(sender);
        self.status = Status::Starting;
        self.error = None;
        cx.notify();
    }

    fn capture_target_name(&self, spec: CaptureSpec) -> String {
        match spec {
            CaptureSpec::Window(window_id) => self
                .windows
                .iter()
                .find(|window| window.id() == window_id)
                .map(window_name)
                .unwrap_or_else(|| format!("Window {window_id}")),
            CaptureSpec::Display(display_id) | CaptureSpec::Region { display_id, .. } => {
                display_name(display_id).unwrap_or_else(|| format!("Display {display_id}"))
            }
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Recording) {
            return;
        }
        self.recorded_duration = self
            .started_at
            .take()
            .map_or(Duration::ZERO, |start| start.elapsed());
        self.selection.recording.set(false);
        self.close_selection_windows(cx);
        self.status = Status::Finalizing;
        if let Some(sender) = self.stop_sender.take() {
            sender.send(()).ok();
        }
        cx.notify();
    }

    fn close_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let application = topmost_application_pid();
        self.selection.recording.set(false);
        self.close_selection_windows(cx);
        self.close_camera_window(cx);
        self.visible.set(false);
        unregister_escape_hotkey(&self.escape_hotkey);
        window.remove_window();
        cx.defer(move |_| {
            if let Some(process_id) = application
                && let Some(application) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(process_id)
                && let Some(main_thread) = MainThreadMarker::new()
            {
                let current_application = NSRunningApplication::currentApplication();
                NSApplication::sharedApplication(main_thread)
                    .yieldActivationToApplication(&application);
                application.activateFromApplication_options(
                    &current_application,
                    NSApplicationActivationOptions::ActivateAllWindows,
                );
            }
        });
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Idle) {
            return;
        }

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
                self.recorded_duration = Duration::ZERO;
                self.resize_toolbar((300.0, 56.0), cx);
                cx.spawn(async move |app, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                        let keep_ticking = app
                            .update(cx, |app, cx| {
                                cx.notify();
                                matches!(app.status, Status::Recording)
                            })
                            .unwrap_or(false);
                        if !keep_ticking {
                            break;
                        }
                    }
                })
                .detach();
            }
            RecordingEvent::Uploading => {
                self.status = Status::Uploading;
            }
            RecordingEvent::Finished { path, viewer_url } => {
                if let Some(viewer_url) = viewer_url {
                    cx.write_to_clipboard(ClipboardItem::new_string(viewer_url.clone()));
                    if self.open_recording_when_finished {
                        cx.open_url(&viewer_url);
                    }
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "blip")
                {
                    if let Err(error) = BundleEditor::open(path.clone(), cx) {
                        eprintln!("blip-capture: {error}");
                    }
                } else {
                    match self.recording_completion_action {
                        CompletionAction::CopyToClipboard => {
                            cx.write_to_clipboard(ClipboardItem {
                                entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                                    std::iter::once(path.clone()).collect(),
                                ))],
                            });
                        }
                        CompletionAction::Reveal if self.open_recording_when_finished => {
                            cx.reveal_path(&path);
                        }
                        CompletionAction::Reveal | CompletionAction::None => {}
                    }
                }
                self.selection.recording.set(false);
                self.close_selection_windows(cx);
                self.close_camera_window(cx);
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
                self.recorded_duration = Duration::ZERO;
                self.stop_sender = None;
                self.selection.recording.set(false);
                self.error = Some(message);
                match register_escape_hotkey() {
                    Ok(hotkey) => self.escape_hotkey.set(hotkey),
                    Err(status) => {
                        eprintln!("blip-capture: failed to register Escape ({status})");
                    }
                }
                let dimensions = self.idle_toolbar_dimensions();
                self.resize_toolbar(dimensions, cx);
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

    fn idle_toolbar_dimensions(&self) -> (f32, f32) {
        let label = self
            .profiles
            .selected()
            .map_or("Profile", |profile| profile.name.as_str());
        toolbar_dimensions_for_label(label)
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn was_dragged(&mut self, window: &Window) -> bool {
        let Some(start) = self.drag_start_window_position.take() else {
            return false;
        };
        let current = window.bounds().origin;
        (current.x - start.x).abs() > px(3.0) || (current.y - start.y).abs() > px(3.0)
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
            .on_click(cx.listener(move |app, _, window, cx| {
                if !app.was_dragged(window) {
                    app.set_mode(mode, window, cx);
                }
            }))
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, _| {
                    app.drag_start_window_position = Some(window.bounds().origin);
                    window.start_window_move();
                }),
            );
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
                            if !app.was_dragged(window) {
                                app.close_windows(window, cx);
                            }
                        }))
                        .child(div().relative().top(px(-1.0)).child("×")),
                )
                .child(self.mode_button(Mode::Display, "Display", cx))
                .child(self.mode_button(Mode::Window, "Window", cx))
                .child(self.mode_button(Mode::Region, "Region", cx))
                .child(
                    overlay_secondary_button(
                        "camera",
                        if self.selected_camera.is_some() {
                            "Camera: On"
                        } else {
                            "Camera: Off"
                        },
                    )
                    .on_click(cx.listener(|app, _, window, cx| {
                        if !app.was_dragged(window) {
                            app.show_camera_menu(window, cx);
                        }
                    })),
                )
                .child(
                    div()
                        .mx_1()
                        .w(px(1.0))
                        .h(px(20.0))
                        .flex_none()
                        .bg(rgba(0xffff_ff18)),
                )
                .child(
                    overlay_secondary_button(
                        "destination",
                        self.profiles
                            .selected()
                            .map_or("Profile", |profile| profile.name.as_str()),
                    )
                    .on_click(cx.listener(|app, _, window, cx| {
                        if !app.was_dragged(window) {
                            app.show_destination_menu(window, cx);
                        }
                    })),
                );
        }

        let elapsed = self
            .started_at
            .map_or(self.recorded_duration, |start| start.elapsed());
        let seconds = elapsed.as_secs();
        let time = format!(
            "{:02}:{:02}",
            seconds.div_euclid(60),
            seconds.rem_euclid(60)
        );
        let disabled = matches!(
            self.status,
            Status::Starting | Status::Finalizing | Status::Uploading
        );
        let label = match self.status {
            Status::Starting => "Starting…",
            Status::Finalizing => "Finishing…",
            Status::Uploading => "Uploading…",
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
                            .on_click(cx.listener(|app, _, window, cx| {
                                if !app.was_dragged(window) {
                                    app.stop(cx);
                                }
                            }))
                    })
                    .child(label),
            )
    }
}

struct ProfileSettings {
    controller: Entity<CaptureApp>,
    profiles: RecordingProfiles,
    selected: usize,
    name_input: Entity<NumericInput>,
    destination_input: Entity<NumericInput>,
    token_input: Entity<NumericInput>,
    error: Option<String>,
}

impl ProfileSettings {
    fn new(
        controller: Entity<CaptureApp>,
        profiles: RecordingProfiles,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected = profiles.selected_index();
        let name_input = cx.new(|cx| NumericInput::new_text("Profile name", cx));
        let destination_input =
            cx.new(|cx| NumericInput::new_text("Folder or Blip server URL", cx));
        let token_input = cx.new(|cx| NumericInput::new_text("blip_...", cx));
        let mut settings = Self {
            controller,
            profiles,
            selected,
            name_input,
            destination_input,
            token_input,
            error: None,
        };
        settings.load_inputs(cx);
        settings
    }

    fn commit_inputs(&mut self, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_owned();
        let destination = self.destination_input.read(cx).value().trim().to_owned();
        let token = self.token_input.read(cx).value().trim().to_owned();
        let Some(profile) = self.profiles.profiles.get_mut(self.selected) else {
            return;
        };
        profile.name = name;
        match &mut profile.target {
            RecordingTarget::Local { folder } => *folder = PathBuf::from(destination),
            RecordingTarget::Remote { server_url } => {
                *server_url = join_server_url(&destination, &token);
            }
        }
    }

    fn load_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.profiles.get(self.selected) else {
            return;
        };
        let name = profile.name.clone();
        let (destination, token) = match &profile.target {
            RecordingTarget::Local { folder } => {
                (folder.to_string_lossy().into_owned(), String::new())
            }
            RecordingTarget::Remote { server_url } => split_server_url(server_url),
        };
        self.name_input
            .update(cx, |input, cx| input.set_text(name, cx));
        self.destination_input
            .update(cx, |input, cx| input.set_text(destination, cx));
        self.token_input
            .update(cx, |input, cx| input.set_text(token, cx));
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        if self.profiles.profiles.get(index).is_none() {
            return;
        }
        self.selected = index;
        self.error = None;
        self.load_inputs(cx);
        cx.notify();
    }

    fn blur_input(&mut self, _: &MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        window.blur();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let folder = std::env::var_os("HOME")
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
            .join("Desktop");
        self.profiles
            .profiles
            .push(RecordingProfile::new_local(folder));
        self.selected = self.profiles.profiles.len().saturating_sub(1);
        self.error = None;
        self.load_inputs(cx);
        cx.notify();
    }

    fn duplicate(&mut self, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let Some(profile) = self.profiles.profiles.get(self.selected) else {
            return;
        };
        let duplicate = profile.duplicate();
        self.profiles.profiles.push(duplicate);
        self.selected = self.profiles.profiles.len().saturating_sub(1);
        self.error = None;
        self.load_inputs(cx);
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.profiles.profiles.len() == 1 {
            self.error = Some("At least one recording profile is required".into());
            cx.notify();
            return;
        }
        let Some(removed) = self.profiles.profiles.get(self.selected) else {
            return;
        };
        let removed_id = removed.id.clone();
        self.profiles.profiles.remove(self.selected);
        self.selected = self
            .selected
            .min(self.profiles.profiles.len().saturating_sub(1));
        if self.profiles.selected_profile_id == removed_id
            && let Some(profile) = self.profiles.profiles.first()
        {
            self.profiles.selected_profile_id = profile.id.clone();
        }
        self.error = None;
        self.load_inputs(cx);
        cx.notify();
    }

    fn set_remote(&mut self, remote: bool, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let Some(profile) = self.profiles.profiles.get_mut(self.selected) else {
            return;
        };
        if remote && matches!(profile.target, RecordingTarget::Local { .. }) {
            profile.target = RecordingTarget::Remote {
                server_url: "https://blip.brendonovich.dev/".into(),
            };
            profile.format = RecordingFormat::Mp4;
            profile.completion_action = CompletionAction::None;
        } else if !remote && matches!(profile.target, RecordingTarget::Remote { .. }) {
            let folder = std::env::var_os("HOME")
                .map_or_else(|| PathBuf::from("."), PathBuf::from)
                .join("Desktop");
            profile.target = RecordingTarget::Local { folder };
            profile.format = RecordingFormat::Mp4;
            profile.completion_action = CompletionAction::Reveal;
        }
        self.error = None;
        self.load_inputs(cx);
        cx.notify();
    }

    fn set_completion_action(&mut self, action: CompletionAction, cx: &mut Context<Self>) {
        if let Some(profile) = self.profiles.profiles.get_mut(self.selected) {
            profile.completion_action = action;
            cx.notify();
        }
    }

    fn set_format(&mut self, format: RecordingFormat, cx: &mut Context<Self>) {
        if let Some(profile) = self.profiles.profiles.get_mut(self.selected) {
            profile.format = format;
            cx.notify();
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        if let Some(error) = self.profiles.profiles.iter().find_map(|profile| {
            profile
                .validate()
                .err()
                .map(|error| format!("{}: {error}", profile.name))
        }) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if let Err(error) = self.profiles.save() {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let profiles = self.profiles.clone();
        self.controller.update(cx, move |controller, cx| {
            controller.profiles = profiles;
            controller.error = None;
            let dimensions = controller.idle_toolbar_dimensions();
            controller.resize_toolbar(dimensions, cx);
            cx.notify();
        });
        window.remove_window();
    }
}

impl Render for ProfileSettings {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut profile_list = div().flex().flex_col().gap_1();
        for (index, profile) in self.profiles.profiles.iter().enumerate() {
            let selected = index == self.selected;
            profile_list = profile_list.child(
                div()
                    .id(("profile", index))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgba(if selected { 0xffff_ff16 } else { 0x0000_0000 }))
                    .text_color(rgb(if selected { TEXT } else { MUTED }))
                    .hover(|row| row.bg(rgba(0xffff_ff10)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |settings, _, _, cx| settings.select(index, cx)))
                    .child(profile.name.clone()),
            );
        }
        let profile = self.profiles.profiles.get(self.selected);
        let remote =
            profile.is_some_and(|profile| matches!(profile.target, RecordingTarget::Remote { .. }));
        let format = profile.map_or("—", |profile| profile.format.label());
        let bundle = profile.is_some_and(|profile| profile.format == RecordingFormat::BlipBundle);
        let completion_action = profile.map(|profile| profile.completion_action);
        let completion_controls = if remote {
            div().child("The private viewer link is opened and copied after upload.")
        } else if bundle {
            div().child("The Blip Bundle Editor opens when recording finishes.")
        } else {
            div()
                .flex()
                .gap_2()
                .child(
                    settings_choice(
                        "reveal",
                        "Reveal in Finder",
                        completion_action == Some(CompletionAction::Reveal),
                    )
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.set_completion_action(CompletionAction::Reveal, cx);
                    })),
                )
                .child(
                    settings_choice(
                        "clipboard",
                        "Copy to Clipboard",
                        completion_action == Some(CompletionAction::CopyToClipboard),
                    )
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.set_completion_action(CompletionAction::CopyToClipboard, cx);
                    })),
                )
                .child(
                    settings_choice(
                        "nothing",
                        "Do Nothing",
                        completion_action == Some(CompletionAction::None),
                    )
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.set_completion_action(CompletionAction::None, cx);
                    })),
                )
        };
        div()
            .size_full()
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|action: &CloseAllWindows, _, cx| close_all_windows(action, cx))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::blur_input))
            .flex()
            .bg(rgb(0x0014_1518))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .w(px(210.0))
                    .h_full()
                    .p_4()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgba(0xffff_ff16))
                    .child(
                        div()
                            .mb_3()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Recording Profiles"),
                    )
                    .child(
                        div()
                            .id("profile-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(profile_list),
                    )
                    .child(
                        div()
                            .mt_3()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                settings_button("add-profile", "Add")
                                    .on_click(cx.listener(|settings, _, _, cx| settings.add(cx))),
                            )
                            .child(
                                settings_button("duplicate-profile", "Duplicate").on_click(
                                    cx.listener(|settings, _, _, cx| settings.duplicate(cx)),
                                ),
                            )
                            .child(
                                settings_button("delete-profile", "Delete").on_click(
                                    cx.listener(|settings, _, _, cx| settings.delete(cx)),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .id("profile-settings-pane")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(settings_field("Name", self.name_input.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_label("Destination"))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        settings_choice("local", "Local folder", !remote).on_click(
                                            cx.listener(|settings, _, _, cx| {
                                                settings.set_remote(false, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        settings_choice("remote", "Blip server", remote).on_click(
                                            cx.listener(|settings, _, _, cx| {
                                                settings.set_remote(true, cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(self.destination_input.clone()),
                    )
                    .when(remote, |panel| {
                        panel.child(settings_field("Access token", self.token_input.clone()))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_label("Format"))
                            .child(if remote {
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        settings_choice("format-hls", "HLS", format == "HLS")
                                            .on_click(cx.listener(|settings, _, _, cx| {
                                                settings.set_format(RecordingFormat::Hls, cx);
                                            })),
                                    )
                                    .child(
                                        settings_choice("format-mp4", "MP4", format == "MP4")
                                            .on_click(cx.listener(|settings, _, _, cx| {
                                                settings.set_format(RecordingFormat::Mp4, cx);
                                            })),
                                    )
                            } else {
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(settings_choice("format-mp4", "MP4", !bundle).on_click(
                                        cx.listener(|settings, _, _, cx| {
                                            settings.set_format(RecordingFormat::Mp4, cx);
                                        }),
                                    ))
                                    .child(
                                        settings_choice("format-bundle", "Blip Bundle", bundle)
                                            .on_click(cx.listener(|settings, _, _, cx| {
                                                settings
                                                    .set_format(RecordingFormat::BlipBundle, cx);
                                            })),
                                    )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_label("After recording"))
                            .child(completion_controls),
                    )
                    .child(div().flex_1())
                    .when_some(self.error.clone(), |panel, error| {
                        panel.child(div().text_sm().text_color(rgb(ACCENT)).child(error))
                    })
                    .child(
                        div().flex().justify_end().child(
                            settings_button("save-profiles", "Save Profiles")
                                .bg(rgb(0x00d2_d2d2))
                                .text_color(rgb(0x0017_1717))
                                .on_click(cx.listener(|settings, _, window, cx| {
                                    settings.save(window, cx);
                                })),
                        ),
                    ),
            )
    }
}

fn settings_label(label: &'static str) -> Div {
    div().text_sm().text_color(rgb(MUTED)).child(label)
}

fn settings_field(label: &'static str, input: Entity<NumericInput>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(settings_label(label))
        .child(input)
}

fn settings_button(id: &'static str, label: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgba(0xffff_ff20))
        .text_sm()
        .hover(|button| button.bg(rgba(0xffff_ff12)))
        .cursor_pointer()
        .child(label)
}

fn settings_choice(id: &'static str, label: &'static str, selected: bool) -> gpui::Stateful<Div> {
    settings_button(id, label)
        .bg(rgba(if selected { 0xffff_ff20 } else { 0x0000_0000 }))
        .border_color(rgba(if selected { 0xffff_ff42 } else { 0xffff_ff20 }))
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

fn overlay_secondary_button(
    id: &'static str,
    label: impl Into<SharedString>,
) -> gpui::Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .h(px(36.0))
        .max_w(px(300.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .rounded_md()
        .border_1()
        .border_color(rgba(0xffff_ff18))
        .bg(rgba(0xffff_ff12))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT))
        .hover(|button| button.bg(rgba(0xffff_ff1c)))
        .active(|button| button.opacity(0.72))
        .cursor_pointer()
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label),
        )
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

fn topmost_application_pid() -> Option<i32> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let window_ids = create_window_list(options, kCGNullWindowID)?;
    let window_descriptions = create_description_from_array(window_ids)?;
    let own_process_id = i32::try_from(std::process::id()).ok()?;
    // SAFETY: CoreGraphics exposes this static dictionary key for the process lifetime.
    let owner_pid_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    window_descriptions.iter().find_map(|window| {
        window
            .find(&owner_pid_key)
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|process_id| process_id.to_i32())
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
    window.setStyleMask(NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Resizable);
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

fn configure_camera_window(window: &Window) -> Option<u32> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI's AppKit handle points to the live NSView owned by this main-thread window.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view.window()?;
    let autosave_name = NSString::from_str(CAMERA_WINDOW_AUTOSAVE_NAME);
    window.setFrameUsingName(&autosave_name);
    window.setFrameAutosaveName(&autosave_name);
    window.setStyleMask(NSWindowStyleMask::NonactivatingPanel);
    window.setHasShadow(false);
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    // SAFETY: CoreGraphics accepts this documented window-level key without additional state.
    let maximum_level = unsafe { CGWindowLevelForKey(CG_MAXIMUM_WINDOW_LEVEL_KEY) };
    let level = isize::try_from(maximum_level.saturating_sub(1)).ok()?;
    window.setLevel(level);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    window.orderFrontRegardless();
    u32::try_from(window.windowNumber()).ok()
}

pub(crate) fn set_camera_window_bounds(window: &Window, bounds: Bounds<Pixels>) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    let view_address = handle.ns_view.as_ptr() as usize;
    let origin = bounds.origin.map(f64::from);
    let size = bounds.size.map(f64::from);
    DispatchQueue::main().exec_async(move || {
        let Some(view) = NonNull::new(view_address as *mut c_void) else {
            return;
        };
        // SAFETY: The camera window owns this GPUI view for the duration of an active resize.
        let view = unsafe { view.cast::<NSView>().as_ref() };
        let Some(window) = view.window() else {
            return;
        };
        let Some(screen) = window.screen() else {
            return;
        };
        let screen_frame = NSScreen::frame(&screen);
        let frame = NSRect::new(
            NSPoint::new(
                screen_frame.origin.x + origin.x,
                screen_frame.origin.y + screen_frame.size.height - origin.y - size.height,
            ),
            NSSize::new(size.width, size.height),
        );
        window.setFrame_display(frame, true);
    });
}

fn set_toolbar_window_visible(window: &Window, visible: bool) {
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
    if visible {
        window.orderFrontRegardless();
    } else {
        window.orderOut(None);
    }
}

#[allow(clippy::as_conversions)]
fn schedule_camera_menu(
    window: &Window,
    camera_names: Vec<String>,
    selected: Option<usize>,
    sender: async_channel::Sender<CameraMenuAction>,
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
        native_camera_menu(view, &camera_names, selected, sender);
    });
    Ok(())
}

#[allow(clippy::arithmetic_side_effects)]
fn native_camera_menu(
    view: NonNull<c_void>,
    camera_names: &[String],
    selected: Option<usize>,
    sender: async_channel::Sender<CameraMenuAction>,
) -> Option<()> {
    // SAFETY: GPUI's AppKit handle points to the live NSView owned by this main-thread window.
    let view = unsafe { view.cast::<NSView>().as_ref() };
    let main_thread = MainThreadMarker::new()?;
    let menu = NSMenu::new(main_thread);
    menu.setAutoenablesItems(false);
    menu.setMinimumWidth(180.0);
    let handler = CameraMenuHandler::new(sender);
    let empty = NSString::from_str("");
    for (tag, name) in std::iter::once("No Camera")
        .chain(camera_names.iter().map(String::as_str))
        .enumerate()
    {
        // SAFETY: The handler implements `selectCamera:` with the NSMenuItem signature.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(main_thread),
                &NSString::from_str(name),
                Some(sel!(selectCamera:)),
                &empty,
            )
        };
        item.setTag(isize::try_from(tag).ok()?);
        // SAFETY: `handler` implements the selector and outlives menu tracking.
        unsafe { item.setTarget(Some(&handler)) };
        if selected.map_or(tag == 0, |index| tag == index.saturating_add(1)) {
            item.setState(NSControlStateValueOn);
        }
        menu.addItem(&item);
    }
    menu.update();
    let bounds = view.bounds();
    menu.popUpMenuPositioningItem_atLocation_inView(
        None,
        NSPoint::new(250.0, bounds.size.height + menu.size().height),
        Some(view),
    );
    Some(())
}

#[allow(clippy::arithmetic_side_effects)]
#[allow(clippy::as_conversions)]
fn schedule_destination_menu(
    window: &Window,
    profile_names: Vec<String>,
    selected: usize,
    open_recording_when_finished: bool,
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
        native_destination_menu(
            view,
            &profile_names,
            selected,
            open_recording_when_finished,
            sender,
        );
    });
    Ok(())
}

#[allow(clippy::arithmetic_side_effects)]
fn native_destination_menu(
    view: NonNull<c_void>,
    profile_names: &[String],
    selected: usize,
    open_recording_when_finished: bool,
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
    for (index, profile_name) in profile_names.iter().enumerate() {
        // SAFETY: The handler implements `selectDestination:` with the NSMenuItem signature.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(main_thread),
                &NSString::from_str(profile_name),
                Some(sel!(selectDestination:)),
                &empty,
            )
        };
        item.setTag(isize::try_from(index).ok()?);
        // SAFETY: `handler` implements the selector configured above and outlives menu tracking.
        unsafe { item.setTarget(Some(&handler)) };
        if index == selected {
            item.setState(NSControlStateValueOn);
        }
        menu.addItem(&item);
    }
    menu.addItem(&NSMenuItem::separatorItem(main_thread));
    // SAFETY: The handler implements `toggleOpenRecording:` with the NSMenuItem signature.
    let open_recording_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(main_thread),
            &NSString::from_str("Open Recording When Finished"),
            Some(sel!(toggleOpenRecording:)),
            &empty,
        )
    };
    // SAFETY: `handler` implements the selector configured above and outlives menu tracking.
    unsafe { open_recording_item.setTarget(Some(&handler)) };
    if open_recording_when_finished {
        open_recording_item.setState(NSControlStateValueOn);
    }
    menu.addItem(&open_recording_item);
    menu.addItem(&NSMenuItem::separatorItem(main_thread));
    // SAFETY: The handler implements `openProfileSettings:` with the NSMenuItem signature.
    let settings_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(main_thread),
            &NSString::from_str("Edit Recording Profiles…"),
            Some(sel!(openProfileSettings:)),
            &empty,
        )
    };
    // SAFETY: `handler` implements the selector configured above and outlives menu tracking.
    unsafe { settings_item.setTarget(Some(&handler)) };
    menu.addItem(&settings_item);
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
fn toolbar_options(cx: &App, profiles: &RecordingProfiles) -> WindowOptions {
    let label = profiles
        .selected()
        .map_or("Profile", |profile| profile.name.as_str());
    let (width, height) = toolbar_dimensions_for_label(label);
    let toolbar_size = size(px(width), px(height));
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

#[allow(clippy::arithmetic_side_effects)]
fn camera_window_options(cx: &App) -> WindowOptions {
    let preview_size = size(px(320.0), px(222.0));
    let bounds = cx.primary_display().map_or_else(
        || Bounds::centered(None, preview_size, cx),
        |display| {
            let screen = display.bounds();
            Bounds::new(
                point(
                    screen.origin.x + screen.size.width - preview_size.width - px(32.0),
                    screen.origin.y + screen.size.height - preview_size.height - px(32.0),
                ),
                preview_size,
            )
        },
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::PopUp,
        is_movable: true,
        app_owns_titlebar_drag: true,
        is_resizable: true,
        is_minimizable: false,
        focus: false,
        window_min_size: Some(size(px(160.0), px(132.0))),
        ..Default::default()
    }
}

fn profile_settings_options(cx: &App) -> WindowOptions {
    let window_size = size(px(720.0), px(480.0));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    }
}

struct OutputDestination {
    media_path: PathBuf,
    completed_path: PathBuf,
    cleanup_on_failure: Option<PathBuf>,
}

fn output_destination(
    profile: &RecordingProfile,
    target_name: &str,
) -> Result<OutputDestination, String> {
    let folder = match &profile.target {
        RecordingTarget::Local { folder } => folder.clone(),
        RecordingTarget::Remote { .. } => std::env::temp_dir().join("blip-capture"),
    };
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("failed to create recording folder: {error}"))?;
    let filename = format!(
        "{} - {}",
        sanitize_filename(target_name),
        Local::now().format("%Y-%m-%d")
    );
    if profile.format == RecordingFormat::Hls {
        let output = unique_output_path(&folder, &filename, "hls");
        return Ok(OutputDestination {
            media_path: output.clone(),
            completed_path: output.clone(),
            cleanup_on_failure: Some(output),
        });
    }
    if profile.format == RecordingFormat::BlipBundle {
        let completed_path = unique_output_path(&folder, &filename, "blip");
        let bundle = BlipBundle::create(&completed_path)?;
        let media_path = bundle.media_path(&completed_path)?;
        return Ok(OutputDestination {
            media_path,
            completed_path: completed_path.clone(),
            cleanup_on_failure: Some(completed_path),
        });
    }
    let completed_path = unique_output_path(&folder, &filename, "mp4");
    Ok(OutputDestination {
        media_path: completed_path.clone(),
        completed_path,
        cleanup_on_failure: None,
    })
}

fn display_name(display_id: u32) -> Option<String> {
    let mtm = MainThreadMarker::new()?;
    NSScreen::screens(mtm)
        .iter()
        .find(|screen| screen.CGDirectDisplayID() == display_id)
        .map(|screen| screen.localizedName().to_string())
}

fn window_name(window: &CaptureWindow) -> String {
    let application = window.application().map(|app| app.name());
    let title = window.title().filter(|title| !title.trim().is_empty());
    match (application, title) {
        (Some(application), Some(title)) => format!("{application} - {title}"),
        (Some(application), None) => application,
        (None, Some(title)) => title,
        (None, None) => format!("Window {}", window.id()),
    }
}

fn sanitize_filename(name: &str) -> String {
    let mut sanitized = name
        .trim()
        .chars()
        .map(|character| {
            if character == '/' || character == ':' || character.is_control() {
                '-'
            } else {
                character
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "Recording".into()
    } else {
        let mut end = sanitized.len().min(150);
        while !sanitized.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        sanitized.truncate(end);
        sanitized
    }
}

fn unique_output_path(folder: &Path, filename: &str, extension: &str) -> PathBuf {
    let initial = folder.join(format!("{filename}.{extension}"));
    if !initial.exists() {
        return initial;
    }
    for suffix in 2_u32.. {
        let candidate = folder.join(format!("{filename} {suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
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
    let cached_targets = target_cache.borrow().clone();
    let targets = if let Some(targets) = cached_targets {
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
    let profiles = RecordingProfiles::load();
    let options = toolbar_options(cx, &profiles);
    let app = match cx.open_window(options, move |window, cx| {
        let window_handle = Window::window_handle(window);
        cx.new(|cx| {
            CaptureApp::new(
                &targets,
                window_handle,
                Rc::clone(visible),
                Rc::clone(escape_hotkey),
                profiles,
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
    app.update(cx, |app, window, _| {
        configure_toolbar_window(window);
        let (width, height) = app.idle_toolbar_dimensions();
        window.resize(size(px(width), px(height)));
    })
    .ok();
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

fn close_window(_: &CloseWindow, cx: &mut App) {
    let window_handle = cx.active_window().or_else(|| {
        cx.window_stack()?.into_iter().find(|window| {
            window.downcast::<BundleEditor>().is_some()
                || window.downcast::<ProfileSettings>().is_some()
                || window.downcast::<CaptureApp>().is_some()
        })
    });
    let Some(window_handle) = window_handle else {
        return;
    };
    cx.defer(move |cx| close_window_handle(window_handle, cx));
}

fn close_window_handle(window_handle: AnyWindowHandle, cx: &mut App) {
    if let Some(controller) = window_handle.downcast::<CaptureApp>() {
        controller.update(cx, CaptureApp::close_windows).ok();
    } else {
        window_handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

fn close_all_windows(_: &CloseAllWindows, cx: &mut App) {
    cx.defer(perform_close_all_windows);
}

fn perform_close_all_windows(cx: &mut App) {
    for window_handle in cx.windows() {
        if let Some(controller) = window_handle.downcast::<CaptureApp>() {
            controller.update(cx, CaptureApp::close_windows).ok();
        }
    }
    for window_handle in cx.windows() {
        window_handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

fn import_profile_urls(cx: &mut App, urls: Vec<String>) {
    let mut profile_urls = Vec::new();
    for url_str in urls {
        if let Ok(url) = url::Url::parse(&url_str)
            && url.scheme() == "file"
            && let Ok(path) = url.to_file_path()
            && path.extension().is_some_and(|ext| ext == "blip")
        {
            if let Err(error) = BundleEditor::open(path, cx) {
                eprintln!("blip-capture: {error}");
            }
            continue;
        }
        if Path::new(&url_str)
            .extension()
            .is_some_and(|ext| ext == "blip")
        {
            if let Err(error) = BundleEditor::open(PathBuf::from(url_str), cx) {
                eprintln!("blip-capture: {error}");
            }
            continue;
        }
        profile_urls.push(url_str);
    }
    if profile_urls.is_empty() {
        return;
    }
    let mut profiles = RecordingProfiles::load();
    let result = profile_urls
        .iter()
        .try_for_each(|url| profiles.import_url(url))
        .and_then(|()| profiles.save());

    for window in cx.windows() {
        let Some(controller) = window.downcast::<CaptureApp>() else {
            continue;
        };
        controller
            .update(cx, |controller, _, cx| {
                match &result {
                    Ok(()) => {
                        controller.profiles = profiles.clone();
                        controller.error = None;
                        let dimensions = controller.idle_toolbar_dimensions();
                        controller.resize_toolbar(dimensions, cx);
                    }
                    Err(error) => controller.error = Some(error.clone()),
                }
                cx.notify();
            })
            .ok();
        break;
    }
    if let Err(error) = result {
        eprintln!("blip-capture: failed to import recording profile: {error}");
    }
}

fn create_menu_bar_item(sender: async_channel::Sender<MenuBarAction>) -> Option<MenuBarItem> {
    let main_thread = MainThreadMarker::new()?;
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    item.button(main_thread)?
        .setTitle(&NSString::from_str("Blip"));

    let menu = NSMenu::new(main_thread);
    let handler = MenuBarHandler::new(sender);
    let empty = NSString::from_str("");
    let add_action = |title: &str, action| {
        // SAFETY: `handler` implements each supplied selector with the NSMenuItem signature.
        let menu_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(main_thread),
                &NSString::from_str(title),
                Some(action),
                &empty,
            )
        };
        // SAFETY: `MenuBarItem` retains the handler for as long as the status item is visible.
        unsafe { menu_item.setTarget(Some(&handler)) };
        menu.addItem(&menu_item);
    };
    add_action("New Recording", sel!(newRecording:));
    menu.addItem(&NSMenuItem::separatorItem(main_thread));
    add_action("Check for Updates…", sel!(checkForUpdates:));
    menu.addItem(&NSMenuItem::separatorItem(main_thread));
    add_action("Quit Blip Capture", sel!(quit:));
    item.setMenu(Some(&menu));

    Some(MenuBarItem {
        _item: item,
        _handler: handler,
    })
}

fn run_app(open_path: Option<PathBuf>) {
    let (profile_sender, profile_receiver) = async_channel::unbounded();
    let app = application().with_assets(assets::CaptureAssets);
    app.on_open_urls(move |urls| {
        profile_sender.try_send(urls).ok();
    });
    app.run(move |cx| {
        blip_updater::start();
        cx.background_executor()
            .spawn(async {
                if let Err(error) = request_camera_access(CAMERA_PERMISSION_TIMEOUT) {
                    eprintln!("blip-capture: failed to request camera permission: {error}");
                }
            })
            .detach();
        cx.bind_keys([
            KeyBinding::new("cmd-w", CloseWindow, None),
            KeyBinding::new("cmd-q", CloseAllWindows, None),
            KeyBinding::new("space", TogglePlayback, Some("BundleEditor")),
            KeyBinding::new("c", ToggleCutMode, Some("BundleEditor")),
            KeyBinding::new("delete", DeleteSelected, Some("BundleEditor")),
            KeyBinding::new("backspace", DeleteSelected, Some("BundleEditor")),
            KeyBinding::new("escape", CloseExportDialog, Some("BundleEditor")),
        ]);
        cx.on_action(close_window);
        cx.on_action(close_all_windows);
        cx.set_menus([Menu::new("Blip Capture").items([
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::action("Close All Windows", CloseAllWindows),
        ])]);
        set_activation_policy(NSApplicationActivationPolicy::Accessory);
        cx.on_window_closed(|cx, _| refresh_activation_policy(cx))
            .detach();
        NumericInput::bind_keys(cx);
        let visible = Rc::new(Cell::new(false));
        let escape_hotkey = Rc::new(Cell::new(ptr::null_mut()));
        let target_cache = Rc::new(RefCell::new(None));
        let (menu_bar_sender, menu_bar_receiver) = async_channel::unbounded();
        let menu_bar_item = create_menu_bar_item(menu_bar_sender);
        let (hotkey_sender, hotkey_receiver) = async_channel::unbounded();
        if let Err(status) = register_reopen_hotkey(hotkey_sender) {
            eprintln!("blip-capture: failed to register Cmd-Shift-8 ({status})");
        }
        let hotkey_visible = Rc::clone(&visible);
        let hotkey_escape = Rc::clone(&escape_hotkey);
        let hotkey_target_cache = Rc::clone(&target_cache);
        let menu_bar_visible = Rc::clone(&visible);
        let menu_bar_escape = Rc::clone(&escape_hotkey);
        let menu_bar_target_cache = Rc::clone(&target_cache);
        cx.spawn(async move |cx| {
            let _menu_bar_item = menu_bar_item;
            while let Ok(action) = menu_bar_receiver.recv().await {
                match action {
                    MenuBarAction::NewRecording => {
                        let visible = Rc::clone(&menu_bar_visible);
                        let escape_hotkey = Rc::clone(&menu_bar_escape);
                        let target_cache = Rc::clone(&menu_bar_target_cache);
                        cx.update(|cx| {
                            open_capture(cx, &visible, &escape_hotkey, &target_cache);
                        });
                    }
                    MenuBarAction::CheckForUpdates => blip_updater::check_for_updates(),
                    MenuBarAction::Quit => cx.update(|cx| cx.quit()),
                }
            }
        })
        .detach();
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
        cx.spawn(async move |cx| {
            while let Ok(urls) = profile_receiver.recv().await {
                cx.update(|cx| import_profile_urls(cx, urls));
            }
        })
        .detach();
        if let Some(path) = open_path {
            if let Err(error) = BundleEditor::open(path, cx) {
                eprintln!("blip-capture: {error}");
            }
        } else {
            open_capture(cx, &visible, &escape_hotkey, &target_cache);
        }
    });
}

fn set_activation_policy(policy: NSApplicationActivationPolicy) {
    if let Some(main_thread) = MainThreadMarker::new() {
        NSApplication::sharedApplication(main_thread).setActivationPolicy(policy);
    }
}

fn show_in_dock() {
    set_activation_policy(NSApplicationActivationPolicy::Regular);
}

fn refresh_activation_policy(cx: &App) {
    let has_regular_window = cx.windows().into_iter().any(|window| {
        window.downcast::<BundleEditor>().is_some()
            || window.downcast::<ProfileSettings>().is_some()
    });
    set_activation_policy(if has_regular_window {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    });
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,blip_capture=debug,blip_avfoundation=debug",
                )
            }),
        )
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();
}

fn main() -> ExitCode {
    init_tracing();
    let args = CaptureArgs::parse();
    if !args.headless {
        run_app(args.path);
        return ExitCode::SUCCESS;
    }

    match headless::run(&args) {
        Ok(viewer_url) => {
            println!("{viewer_url}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("blip-capture: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CaptureArgs;
    use clap::Parser as _;

    #[test]
    fn accepts_gui_mode_without_options() {
        let result = CaptureArgs::try_parse_from(["blip-capture"]);
        assert!(result.is_ok());
    }

    #[test]
    fn accepts_headless_upload_options() {
        let result = CaptureArgs::try_parse_from([
            "blip-capture",
            "--headless",
            "--server-url",
            "https://blip.example#secret",
            "--display",
            "1",
            "--duration",
            "10",
            "--format",
            "hls",
        ]);
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_headless_options_in_gui_mode() {
        let result = CaptureArgs::try_parse_from(["blip-capture", "--duration", "10"]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_zero_length_headless_recording() {
        let result = CaptureArgs::try_parse_from([
            "blip-capture",
            "--headless",
            "--server-url",
            "https://blip.example#secret",
            "--duration",
            "0",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn accepts_path_to_open_bundle() {
        let result = CaptureArgs::try_parse_from(["blip-capture", "/tmp/test.blip"]);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().path,
            Some(std::path::PathBuf::from("/tmp/test.blip"))
        );
    }

    #[test]
    fn calculates_dynamic_toolbar_dimensions() {
        assert_eq!(super::toolbar_dimensions_for_label("Dev"), (472.0, 56.0));
        assert_eq!(
            super::toolbar_dimensions_for_label("Prod Server"),
            (536.0, 56.0)
        );
        assert_eq!(
            super::toolbar_dimensions_for_label("A Very Long Server Name That Exceeds Max"),
            (688.0, 56.0)
        );
    }

    #[test]
    fn sanitizes_capture_target_for_filename() {
        assert_eq!(
            super::sanitize_filename("Safari: Docs/Reference\n"),
            "Safari- Docs-Reference"
        );
        assert_eq!(super::sanitize_filename(" \n "), "Recording");
    }
}

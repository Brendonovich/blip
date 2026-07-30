use std::ops::Range;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Context, CursorStyle, Div, Element,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Stateful, Style,
    Subscription, TextAlign, TextRun, Transformation, UTF16Selection, Window, actions, div,
    ease_out_quint, fill, percentage, point, prelude::*, px, relative, rgb, size, svg,
};

pub const DROPDOWN_ANIMATION_DURATION: Duration = Duration::from_millis(150);

#[derive(Clone, Copy)]
pub struct DropdownStyle {
    pub control_background: u32,
    pub control_hover: u32,
    pub control_active: u32,
    pub border_subtle: u32,
    pub border: u32,
    pub text_muted: u32,
    pub trigger_full_width: bool,
    pub trigger_height: Pixels,
    pub menu_top: Pixels,
    pub menu_max_height: Pixels,
    pub option_height: Pixels,
    pub menu_shadow: bool,
}

impl Default for DropdownStyle {
    fn default() -> Self {
        Self {
            control_background: 0x001c_1c1c,
            control_hover: 0x0023_2323,
            control_active: 0x0029_2929,
            border_subtle: 0x0024_2424,
            border: 0x0030_3030,
            text_muted: 0x0099_9999,
            trigger_full_width: true,
            trigger_height: px(30.0),
            menu_top: px(34.0),
            menu_max_height: px(280.0),
            option_height: px(28.0),
            menu_shadow: false,
        }
    }
}

pub fn dropdown_trigger(id: impl Into<ElementId>, style: DropdownStyle) -> Stateful<Div> {
    div()
        .id(id)
        .when(style.trigger_full_width, gpui::Styled::w_full)
        .h(style.trigger_height)
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_sm()
        .bg(rgb(style.control_background))
        .border_1()
        .border_color(rgb(style.border_subtle))
        .hover(move |button| button.bg(rgb(style.control_hover)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .cursor_pointer()
}

pub fn dropdown_chevron(
    path: impl Into<SharedString>,
    open: bool,
    transition: u64,
    style: DropdownStyle,
) -> AnyElement {
    let chevron = svg()
        .size(px(14.0))
        .path(path)
        .text_color(rgb(style.text_muted))
        .flex_none();
    if transition == 0 {
        chevron.into_any_element()
    } else {
        chevron
            .with_animation(
                format!("dropdown-chevron-transition-{transition}"),
                Animation::new(DROPDOWN_ANIMATION_DURATION).with_easing(ease_out_quint()),
                move |chevron, delta| {
                    let progress = if open { delta } else { 1.0 - delta };
                    chevron.with_transformation(Transformation::rotate(percentage(progress * 0.5)))
                },
            )
            .into_any_element()
    }
}

pub fn dropdown_menu(
    id: impl Into<ElementId>,
    open: bool,
    transition: u64,
    style: DropdownStyle,
    children: impl IntoIterator<Item = AnyElement>,
) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .left_0()
        .right_0()
        .max_h(style.menu_max_height)
        .p_1()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_y_scroll()
        .rounded_sm()
        .bg(rgb(style.control_background))
        .border_1()
        .border_color(rgb(style.border))
        .when(style.menu_shadow, gpui::Styled::shadow_lg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(children)
        .with_animation(
            format!("dropdown-menu-transition-{transition}"),
            Animation::new(DROPDOWN_ANIMATION_DURATION).with_easing(ease_out_quint()),
            #[allow(clippy::arithmetic_side_effects)]
            move |menu, delta| {
                let visibility = if open { delta } else { 1.0 - delta };
                menu.top(style.menu_top - px(6.0) * (1.0 - visibility))
                    .opacity(visibility)
            },
        )
        .into_any_element()
}

pub fn dropdown_option(
    id: impl Into<ElementId>,
    selected: bool,
    style: DropdownStyle,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .min_h(style.option_height)
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .text_xs()
        .when(selected, |option| option.bg(rgb(style.control_active)))
        .hover(move |option| option.bg(rgb(style.control_hover)))
        .cursor_pointer()
}

actions!(
    numeric_input,
    [
        Backspace, Delete, Left, Right, Increment, Decrement, SelectAll, Paste
    ]
);

pub enum NumericInputEvent {
    Changed(f32),
    FocusChanged(bool),
}

pub struct NumericInputStyle {
    pub control_background: u32,
    pub border_subtle: u32,
    pub text: u32,
    pub text_muted: u32,
    pub text_dim: u32,
    pub focus: u32,
    pub selection_fill: u32,
}

impl Default for NumericInputStyle {
    fn default() -> Self {
        Self {
            control_background: 0x001c_1c1c,
            border_subtle: 0x0024_2424,
            text: 0x00e8_e8e8,
            text_muted: 0x0099_9999,
            text_dim: 0x0068_6868,
            focus: 0x007e_8ea6,
            selection_fill: 0x0039_4352,
        }
    }
}

pub struct NumericInput {
    label: Option<&'static str>,
    placeholder: SharedString,
    numeric: bool,
    focus_handle: FocusHandle,
    content: SharedString,
    selected_range: Range<usize>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    focus_subscriptions: Vec<Subscription>,
    style: NumericInputStyle,
}

impl NumericInput {
    pub fn new(label: &'static str, cx: &mut Context<Self>) -> Self {
        Self {
            label: Some(label),
            placeholder: "".into(),
            numeric: true,
            focus_handle: cx.focus_handle(),
            content: "0".into(),
            selected_range: 0..1,
            last_layout: None,
            last_bounds: None,
            focus_subscriptions: Vec::new(),
            style: NumericInputStyle::default(),
        }
    }

    pub fn new_text(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            label: None,
            placeholder: placeholder.into(),
            numeric: false,
            focus_handle: cx.focus_handle(),
            content: "".into(),
            selected_range: 0..0,
            last_layout: None,
            last_bounds: None,
            focus_subscriptions: Vec::new(),
            style: NumericInputStyle::default(),
        }
    }

    #[must_use]
    pub fn with_style(mut self, style: NumericInputStyle) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.content
    }

    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("NumericInput")),
            KeyBinding::new("delete", Delete, Some("NumericInput")),
            KeyBinding::new("left", Left, Some("NumericInput")),
            KeyBinding::new("right", Right, Some("NumericInput")),
            KeyBinding::new("up", Increment, Some("NumericInput")),
            KeyBinding::new("down", Decrement, Some("NumericInput")),
            KeyBinding::new("cmd-a", SelectAll, Some("NumericInput")),
            KeyBinding::new("cmd-v", Paste, Some("NumericInput")),
        ]);
    }

    pub fn set_value(&mut self, value: f32, focused: bool, cx: &mut Context<Self>) {
        if focused {
            return;
        }
        let content = format!("{value:.0}");
        if self.content.as_ref() != content {
            self.content = content.into();
            self.selected_range = self.content.len()..self.content.len();
            cx.notify();
        }
    }

    pub fn set_text(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = value.into();
        self.selected_range = self.content.len()..self.content.len();
        cx.notify();
    }

    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.selected_range.is_empty() && self.selected_range.start > 0 {
            self.selected_range.start = self.selected_range.start.saturating_sub(1);
        }
        self.replace_selection("", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.selected_range.is_empty() && self.selected_range.end < self.content.len() {
            self.selected_range.end = self.selected_range.end.saturating_add(1);
        }
        self.replace_selection("", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.selected_range.start.saturating_sub(1);
        self.selected_range = offset..offset;
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self
            .selected_range
            .end
            .saturating_add(1)
            .min(self.content.len());
        self.selected_range = offset..offset;
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        cx.notify();
    }

    fn increment(&mut self, _: &Increment, _: &mut Window, cx: &mut Context<Self>) {
        self.adjust_value(1.0, cx);
    }

    fn decrement(&mut self, _: &Decrement, _: &mut Window, cx: &mut Context<Self>) {
        self.adjust_value(-1.0, cx);
    }

    fn adjust_value(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Ok(value) = self.content.parse::<f32>() else {
            return;
        };
        self.content = format!("{:.0}", value + delta).into();
        self.selected_range = 0..self.content.len();
        cx.emit(NumericInputEvent::Changed(value + delta));
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(text.trim(), window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus_handle, cx);
        if self.numeric {
            self.selected_range = 0..self.content.len();
        } else {
            let offset = self.index_for_mouse_position(event.position);
            self.selected_range = offset..offset;
        }
        cx.notify();
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn replace_selection(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let candidate = self.content[..self.selected_range.start].to_owned()
            + text
            + &self.content[self.selected_range.end..];
        if self.numeric && !is_numeric_candidate(&candidate) {
            window.play_system_bell();
            return;
        }
        let cursor = self.selected_range.start.saturating_add(text.len());
        self.content = candidate.into();
        self.selected_range = cursor..cursor;
        if self.numeric
            && let Ok(value) = self.content.parse()
        {
            cx.emit(NumericInputEvent::Changed(value));
        }
        cx.notify();
    }
}

impl EventEmitter<NumericInputEvent> for NumericInput {}

impl EntityInputHandler for NumericInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        actual_range.replace(range.clone());
        self.content.get(range).map(ToOwned::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.selected_range.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = range {
            self.selected_range = range;
        }
        self.replace_selection(text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        line.index_for_x(point.x - bounds.left())
    }
}

struct NumericTextElement {
    input: Entity<NumericInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for NumericTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for NumericTextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    #[allow(clippy::arithmetic_side_effects)]
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let display_text = if content.is_empty() {
            input.placeholder.clone()
        } else {
            content.clone()
        };
        let selection = input.selected_range.clone();
        let focused = input.focus_handle.is_focused(window);
        let run = TextRun {
            len: display_text.len(),
            font: window.text_style().font(),
            color: if content.is_empty() {
                rgb(input.style.text_dim).into()
            } else {
                window.text_style().color
            },
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &[run], None);
        let cursor_x = line.x_for_index(selection.end);
        let (selection_quad, cursor) = if !focused {
            (None, None)
        } else if selection.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, bounds.top()),
                        size(px(1.0), bounds.size.height),
                    ),
                    rgb(input.style.focus),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selection.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selection.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgb(input.style.selection_fill),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection: selection_quad,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = state.selection.take() {
            window.paint_quad(selection);
        }
        let Some(line) = state.line.take() else {
            return;
        };
        let _ = line.paint(
            bounds.origin,
            window.line_height(),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        if focus_handle.is_focused(window)
            && let Some(cursor) = state.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for NumericInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_subscriptions.is_empty() {
            let focus_handle = self.focus_handle.clone();
            self.focus_subscriptions
                .push(cx.on_focus(&focus_handle, window, |_, _, cx| {
                    cx.notify();
                    cx.emit(NumericInputEvent::FocusChanged(true));
                }));
            self.focus_subscriptions
                .push(cx.on_blur(&focus_handle, window, |input, _, cx| {
                    let cursor = input.selected_range.end;
                    input.selected_range = cursor..cursor;
                    cx.notify();
                    cx.emit(NumericInputEvent::FocusChanged(false));
                }));
        }
        let input = div()
            .key_context("NumericInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::increment))
            .on_action(cx.listener(Self::decrement))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .w_full()
            .h(px(28.0))
            .px_2()
            .flex()
            .items_center()
            .rounded_sm()
            .bg(rgb(self.style.control_background))
            .border_1()
            .border_color(if self.focus_handle.is_focused(window) {
                rgb(self.style.focus)
            } else {
                rgb(self.style.border_subtle)
            })
            .text_color(rgb(self.style.text))
            .text_sm();
        let input = if let Some(label) = self.label {
            input.child(
                div()
                    .w(px(18.0))
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(self.style.text_muted))
                    .child(label),
            )
        } else {
            input
        };
        input.child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(NumericTextElement { input: cx.entity() }),
        )
    }
}

impl Focusable for NumericInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn is_numeric_candidate(value: &str) -> bool {
    if matches!(value, "" | "-" | "." | "-.") {
        return true;
    }
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let mut decimal = false;
    unsigned.chars().all(|character| {
        if character == '.' && !decimal {
            decimal = true;
            true
        } else {
            character.is_ascii_digit()
        }
    }) && unsigned.chars().any(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::is_numeric_candidate;

    #[test]
    fn accepts_numeric_editing_states() {
        for value in ["", "-", ".", "-.", "0", "-12", "12.5", ".5", "-.5"] {
            assert!(is_numeric_candidate(value), "{value} should be accepted");
        }
    }

    #[test]
    fn rejects_non_numeric_input() {
        for value in ["a", "1a", "1.2.3", "--1", "+1", "1 2"] {
            assert!(!is_numeric_candidate(value), "{value} should be rejected");
        }
    }
}

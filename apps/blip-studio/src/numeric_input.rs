use std::ops::Range;

use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, Subscription, TextAlign, TextRun, UTF16Selection, Window,
    actions, div, fill, point, prelude::*, px, relative, rgb, size,
};

use crate::theme;

actions!(
    numeric_input,
    [
        Backspace, Delete, Left, Right, Increment, Decrement, SelectAll, Paste
    ]
);

pub(crate) enum NumericInputEvent {
    Changed(f32),
    FocusChanged(bool),
}

pub(crate) struct NumericInput {
    label: Option<&'static str>,
    placeholder: SharedString,
    numeric: bool,
    focus_handle: FocusHandle,
    content: SharedString,
    selected_range: Range<usize>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    focus_subscriptions: Vec<Subscription>,
}

impl NumericInput {
    pub(crate) fn new(label: &'static str, cx: &mut Context<Self>) -> Self {
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
        }
    }

    pub(crate) fn new_text(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
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
        }
    }

    pub(crate) fn value(&self) -> &str {
        &self.content
    }

    pub(crate) fn bind_keys(cx: &mut App) {
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

    pub(crate) fn set_value(&mut self, value: f32, focused: bool, cx: &mut Context<Self>) {
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

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
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

    fn on_mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.focus(&self.focus_handle, cx);
        self.selected_range = 0..self.content.len();
        cx.notify();
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
                rgb(theme::TEXT_DIM).into()
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
                    rgb(theme::FOCUS),
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
                    rgb(theme::SELECTION_FILL),
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
            .bg(rgb(theme::CONTROL_BACKGROUND))
            .border_1()
            .border_color(if self.focus_handle.is_focused(window) {
                rgb(theme::FOCUS)
            } else {
                rgb(theme::BORDER_SUBTLE)
            })
            .text_color(rgb(theme::TEXT))
            .text_sm();
        let input = if let Some(label) = self.label {
            input.child(
                div()
                    .w(px(18.0))
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_MUTED))
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

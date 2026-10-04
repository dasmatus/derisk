//! The Calculator as a GPUI view.
//!
//! The arithmetic, history and keypad come from `derisk-calculator`, so this
//! version and the egui one compute the same thing; only the drawing differs.

use derisk_calculator::{CalculatorApp, KEYS, evaluate, format_number};
use gpui::{
    Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Subscription, Window,
    div, prelude::*, px,
};
use mcsapi_components_gpui::{Button, ButtonSize, ButtonVariant, Input, TextInput, Tokens};

use crate::run::DesktopTheme;

/// The Calculator window's root view.
pub struct Calculator {
    app: CalculatorApp,
    input: Entity<TextInput>,
    _edits: Subscription,
    _enter: Subscription,
}

impl Calculator {
    /// A calculator with an empty expression, focused for typing.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(cx).placeholder("2 × (3 + 4)"));
        // Typing goes to the field; the model follows it so the preview and
        // the keypad see what was typed.
        let edits = cx.observe(&input, |this: &mut Self, input, cx| {
            let text = input.read(cx).text();
            if text != this.app.input {
                this.app.input = text.to_owned();
                cx.notify();
            }
        });
        // Enter submits. The field binds Enter to its own action, which
        // gpui dispatches instead of key listeners, so catch the keystroke
        // before bindings are matched. This process has only this window.
        let this = cx.entity().downgrade();
        let enter = cx.intercept_keystrokes(move |event, _, cx| {
            if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
                let _ = this.update(cx, |this, cx| {
                    this.app.submit();
                    this.sync(cx);
                });
                cx.stop_propagation();
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        // Wayland's xdg_toplevel title, which the shell shows in its top bar.
        window.set_window_title("Calculator");
        Self {
            app: CalculatorApp::default(),
            input,
            _edits: edits,
            _enter: enter,
        }
    }

    /// Applies a keypad key, then shows the model's expression in the field.
    fn press(&mut self, key: &str, cx: &mut Context<Self>) {
        self.app.press(key);
        self.sync(cx);
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let text = self.app.input.clone();
        self.input.update(cx, |input, cx| {
            if input.text() != text {
                input.set_text(text, cx);
            }
        });
        cx.notify();
    }

    fn preview(&self) -> (String, bool) {
        match self.app.error() {
            Some(error) => (error.to_string(), true),
            None => match evaluate(&self.app.input, self.app.ans()) {
                Ok(value) => (format!("= {}", format_number(value)), false),
                Err(_) => (String::new(), false),
            },
        }
    }
}

impl Render for Calculator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        let fonts = cx
            .try_global::<DesktopTheme>()
            .map(|theme| theme.0.fonts.clone())
            .unwrap_or_default();
        let (preview, is_error) = self.preview();

        let mut keypad = div().flex().flex_col().gap(px(8.0));
        for row in KEYS {
            let mut line = div().flex().gap(px(8.0));
            for key in row {
                let variant = match key {
                    "=" => ButtonVariant::Default,
                    "C" | "⬅" => ButtonVariant::Outline,
                    _ => ButtonVariant::Secondary,
                };
                line = line.child(
                    div().w(px(72.0)).flex().justify_center().child(
                        Button::new(key)
                            .variant(variant)
                            .size(ButtonSize::Lg)
                            .on_click(cx.listener(move |this, _, _, cx| this.press(key, cx))),
                    ),
                );
            }
            keypad = keypad.child(line);
        }

        let mut history = div()
            .id("history")
            .flex()
            .flex_col()
            .gap(px(4.0))
            .flex_1()
            .overflow_y_scroll();
        for (index, (expression, value)) in self.app.history.iter().enumerate() {
            let reuse = expression.clone();
            history = history.child(
                div()
                    .id(("entry", index))
                    .p(px(6.0))
                    .rounded(t.radius)
                    .hover(|style| style.bg(t.hover))
                    .cursor_pointer()
                    .child(
                        div()
                            .text_color(t.muted_foreground)
                            .child(expression.clone()),
                    )
                    .child(format!("= {}", format_number(*value)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.input = reuse.clone();
                        this.sync(cx);
                    })),
            );
        }

        div()
            .size_full()
            .flex()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(fonts.sans.clone())
            .text_size(px(fonts.size))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .child(Input::new(&self.input))
                    .child(
                        div()
                            .h(px(28.0))
                            .text_size(px(18.0))
                            .text_color(if is_error {
                                t.destructive
                            } else {
                                t.foreground
                            })
                            .child(preview),
                    )
                    .child(keypad),
            )
            .child(
                div()
                    .w(px(220.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .border_l_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("History"),
                            )
                            .child(
                                Button::new("Clear")
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.app.history.clear();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(history),
            )
    }
}

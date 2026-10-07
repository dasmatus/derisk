//! A consent dialog for the portal: "Take a screenshot?" with a button that
//! grants and one that declines, in an mcsapi `NativeDialog` window on the
//! desktop theme. `derisk-gpui ask` runs it and answers with its exit status,
//! so the portal, an async D-Bus service with no windows of its own, shows it
//! by spawning a process.

use mcsapi_ui::dialog::ParentWindow;

/// What the dialog asks, as the portal's `Access` dialog takes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Question {
    /// The window's title, such as the asking app's name.
    pub app: String,
    /// The heading, a question.
    pub title: String,
    /// The line under it.
    pub subtitle: String,
    /// Smaller print.
    pub body: String,
    /// The label on the button that grants.
    pub grant: String,
    /// The label on the button that declines.
    pub deny: String,
    /// The asking app's window, as the portal request names it.
    pub parent: ParentWindow,
}

impl Question {
    /// Reads `--title`, `--subtitle`, `--body`, `--grant`, `--deny`,
    /// `--app` and `--parent`, each followed by its value. The title is
    /// required; the buttons default to "Allow" and "Deny".
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut question = Self {
            grant: "Allow".into(),
            deny: "Deny".into(),
            ..Self::default()
        };
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--title" => question.title = value,
                "--subtitle" => question.subtitle = value,
                "--body" => question.body = value,
                "--grant" => question.grant = value,
                "--deny" => question.deny = value,
                "--app" => question.app = value,
                "--parent" => {
                    question.parent = value.parse().map_err(|e| format!("--parent: {e}"))?;
                }
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        if question.title.is_empty() {
            return Err("--title is required".into());
        }
        Ok(question)
    }

    /// The arguments [`Question::from_args`] reads back into this question.
    pub fn to_args(&self) -> impl Iterator<Item = String> + '_ {
        [
            ("--title", &self.title),
            ("--subtitle", &self.subtitle),
            ("--body", &self.body),
            ("--grant", &self.grant),
            ("--deny", &self.deny),
            ("--app", &self.app),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .flat_map(|(flag, value)| [flag.to_owned(), value.clone()])
        .chain(
            (!self.parent.is_none())
                .then(|| ["--parent".to_owned(), self.parent.to_string()])
                .into_iter()
                .flatten(),
        )
    }
}

#[cfg(feature = "gpui")]
mod dialog {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        App, Context, FocusHandle, IntoElement, ParentElement, Render, Styled, Window, div, px,
    };
    use mcsapi_components_gpui::{NativeDialog, Tokens, typography};
    use mcsapi_ui::dialog::{ActionRole, DialogAction};

    use super::Question;
    use crate::ThemeWatch;

    struct Consent {
        question: Question,
        focus: FocusHandle,
        granted: Rc<Cell<bool>>,
    }

    impl Consent {
        fn answer(&self, granted: bool, window: &mut Window) {
            self.granted.set(granted);
            window.remove_window();
        }
    }

    impl Render for Consent {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let t = Tokens::get(cx);
            let q = &self.question;
            let mut content = div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(typography::large(&t, q.title.clone()));
            if !q.subtitle.is_empty() {
                content = content.child(typography::p(&t, q.subtitle.clone()));
            }
            if !q.body.is_empty() {
                content = content.child(typography::muted(&t, q.body.clone()));
            }
            // Declining comes first and granting is the default, so Enter
            // grants only when the person reaches for it as for any OK.
            let actions = [
                DialogAction::new(q.deny.clone(), ActionRole::Cancel),
                DialogAction::new(q.grant.clone(), ActionRole::Default),
            ];
            let this = cx.entity().downgrade();
            let row = NativeDialog::actions(&actions, move |index, window, cx| {
                let granted = *index == 1;
                let _ = this.update(cx, |this, _| this.answer(granted, window));
            })
            .mt(px(8.0));
            let content = content.child(row);
            let focus = self.focus.clone();
            let this = cx.entity().downgrade();
            NativeDialog::frame(window, cx, &focus, content, move |window, cx| {
                let _ = this.update(cx, |this, _| this.answer(false, window));
            })
        }
    }

    /// Shows the dialog until the person answers or closes it: `true` only
    /// when they grant.
    pub fn ask(question: Question) -> bool {
        let granted = Rc::new(Cell::new(false));
        let answer = granted.clone();
        gpui_platform::application()
            .with_assets(mcsapi_components_gpui::Assets::new())
            .run(move |cx: &mut App| {
                mcsapi_components_gpui::bind_text_input_keys(cx);
                crate::run::install(ThemeWatch::default().poll().unwrap_or_default(), cx);
                let title = if question.app.is_empty() {
                    question.title.clone()
                } else {
                    question.app.clone()
                };
                let opened = NativeDialog::new(title)
                    .parent(question.parent.clone())
                    .open(cx, |_, cx| Consent {
                        question,
                        focus: cx.focus_handle(),
                        granted: answer,
                    });
                if let Err(error) = opened {
                    tracing::error!("opening the consent dialog: {error}");
                    cx.quit();
                    return;
                }
                // Closing the window from the window system declines.
                cx.on_window_closed(|cx, _| cx.quit()).detach();
                cx.activate(true);
            });
        granted.get()
    }
}

#[cfg(feature = "gpui")]
pub use dialog::ask;

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reads_what_it_writes() {
        let question = Question {
            app: "Firefox".into(),
            title: "Take a screenshot?".into(),
            subtitle: "Firefox wants a picture of the whole screen.".into(),
            body: String::new(),
            grant: "Take Screenshot".into(),
            deny: "Cancel".into(),
            parent: ParentWindow::Wayland("abc".into()),
        };
        assert_eq!(Question::from_args(question.to_args()), Ok(question));
    }

    #[test]
    fn needs_a_title_and_values() {
        assert!(Question::from_args(args(&[])).is_err());
        assert!(Question::from_args(args(&["--title"])).is_err());
        assert!(Question::from_args(args(&["--title", "x", "--bogus", "y"])).is_err());
        let question = Question::from_args(args(&["--title", "x"])).unwrap();
        assert_eq!(
            (question.grant.as_str(), question.deny.as_str()),
            ("Allow", "Deny")
        );
    }
}

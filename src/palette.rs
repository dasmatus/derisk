//! The command palette: one search box for everything derisk can do.
//!
//! Super+Space opens it. Typing searches, in one ranked list:
//!
//! - apps (the core apps and anything the host registers),
//! - open windows on every workspace,
//! - commands for the focused window, including its app's own global-menu
//!   items, so apps get palette commands just by registering menus,
//! - shell commands (overview, layouts, workspaces, moving windows),
//! - session commands (lock, suspend, log out, ...), tray items and failed
//!   user units,
//! - settings pages and files.
//!
//! Anything that matches nothing (or reads like a sentence the assistant
//! understands, such as "open firefox and snap it left") goes to the
//! built-in assistant as a natural-language request.
//!
//! A leading character narrows the search: `>` commands only, `@` windows,
//! `/` or `~` files, `?` ask the assistant. Picks are remembered, so things
//! you use often rise to the top, and an empty query lists them first.
//!
//! This module is UI-free; [`crate::ui`] draws it.
//!
//! ```
//! use derisk::{geom::rect, palette::{self, History}, shell::Shell};
//!
//! let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
//! shell.map_window("kitty", "~");
//! let entries = palette::entries(&shell, &[], &[]);
//! let hits = palette::search(&entries, "max", &History::default());
//! assert_eq!(entries[hits[0]].title, "Maximize");
//! ```

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    action::{Action, LayoutKind},
    assistant,
    desktop::DesktopEntry,
    menu::{self, MenuEntry},
    shell::Shell,
    systemd::SessionOp,
};

/// What an entry is, which decides its group heading and search prefix.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum Category {
    /// An application to launch.
    App,
    /// An app's desktop action, such as "New Private Window".
    AppAction,
    /// An open window to switch to.
    Window,
    /// A command from the focused app's own menus.
    AppCommand,
    /// A shell command (window management, overview, layouts).
    Command,
    /// Switch to a workspace or move the window there.
    Workspace,
    /// A settings page.
    Setting,
    /// Lock, suspend, log out, reboot, power off.
    Session,
    /// A tray item or a failed service.
    System,
    /// A file or folder.
    File,
    /// Ask the assistant.
    Ask,
}

impl Category {
    /// Group heading shown above entries of this kind.
    pub fn heading(self) -> &'static str {
        match self {
            Self::App => "Apps",
            Self::AppAction => "App actions",
            Self::Window => "Windows",
            Self::AppCommand => "App commands",
            Self::Command => "Commands",
            Self::Workspace => "Workspaces",
            Self::Setting => "Settings",
            Self::Session => "Session",
            Self::System => "System",
            Self::File => "Files",
            Self::Ask => "Assistant",
        }
    }

    fn is_command(self) -> bool {
        matches!(
            self,
            Self::AppAction
                | Self::AppCommand
                | Self::Command
                | Self::Workspace
                | Self::Session
                | Self::System
        )
    }
}

/// One thing the palette can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Kind of entry.
    pub category: Category,
    /// Main text.
    pub title: String,
    /// Secondary text (app summary, window title, path, ...).
    pub detail: String,
    /// Extra search terms, not shown.
    pub keywords: String,
    /// An emoji icon from egui's built-in font.
    pub icon: String,
    /// Keyboard shortcut hint, if the command has one.
    pub shortcut: Option<String>,
    /// What choosing the entry does.
    pub actions: Vec<Action>,
    /// Ends the session or loses work: needs a second Enter.
    pub confirm: bool,
}

impl Entry {
    /// An entry running `actions`, with no detail, keywords or shortcut.
    pub fn new(
        category: Category,
        icon: &str,
        title: impl Into<String>,
        actions: Vec<Action>,
    ) -> Self {
        Self {
            category,
            title: title.into(),
            detail: String::new(),
            keywords: String::new(),
            icon: icon.to_owned(),
            shortcut: None,
            actions,
            confirm: false,
        }
    }

    /// Sets the secondary text.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// Sets extra search terms.
    pub fn keywords(mut self, keywords: impl Into<String>) -> Self {
        self.keywords = keywords.into();
        self
    }

    /// Sets the shortcut hint.
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// The key [`History`] remembers this entry by.
    pub fn key(&self) -> String {
        format!("{:?}:{}:{}", self.category, self.title, self.detail)
    }

    /// An app launcher entry.
    pub fn app(id: &str, name: &str, summary: &str, icon: &str, keywords: &[&str]) -> Self {
        Self::new(
            Category::App,
            icon,
            name,
            vec![Action::Launch { app: id.to_owned() }],
        )
        .detail(summary)
        .keywords(format!("{id} {}", keywords.join(" ")))
    }
}

/// Entries for an app from its `.desktop` file: the app itself, then one
/// per desktop action.
pub fn desktop_app(app: &DesktopEntry, icon: &str) -> Vec<Entry> {
    let summary = if app.comment.is_empty() {
        &app.generic_name
    } else {
        &app.comment
    };
    let mut out = vec![
        Entry::new(
            Category::App,
            icon,
            app.name.clone(),
            vec![Action::Launch {
                app: app.id.clone(),
            }],
        )
        .detail(summary.clone())
        .keywords(app.search_terms()),
    ];
    out.extend(app.actions.iter().map(|action| {
        Entry::new(
            Category::AppAction,
            icon,
            action.name.clone(),
            vec![Action::LaunchAction {
                app: app.id.clone(),
                id: action.id.clone(),
            }],
        )
        .detail(app.name.clone())
        .keywords(format!("{} {}", app.name, action.id).to_lowercase())
    }));
    out
}

/// How often each entry was chosen, so frequent picks rank first.
#[derive(Clone, Debug, Default)]
pub struct History {
    uses: BTreeMap<String, u32>,
    clock: u32,
    last: BTreeMap<String, u32>,
}

impl History {
    /// Remembers that `entry` was chosen.
    pub fn record(&mut self, entry: &Entry) {
        let key = entry.key();
        self.clock += 1;
        *self.uses.entry(key.clone()).or_default() += 1;
        self.last.insert(key, self.clock);
    }

    /// Ranking boost for an entry: grows with use and recency.
    pub fn boost(&self, entry: &Entry) -> u32 {
        let key = entry.key();
        let uses = self.uses.get(&key).copied().unwrap_or(0).min(10);
        let recency = self
            .last
            .get(&key)
            .map_or(0, |&t| 20u32.saturating_sub(self.clock - t));
        uses * 20 + recency * 3
    }
}

/// Scores how well `query` matches `text`, ignoring case; `None` if some
/// query character is missing.
///
/// Contiguous matches beat scattered ones, and matches at the start of the
/// text or of a word beat matches inside a word.
pub fn fuzzy(text: &str, query: &str) -> Option<u32> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    let word_start = |i: usize| i == 0 || !text[i - 1].is_alphanumeric();
    // A contiguous substring, best at a word start.
    let substring = (0..text.len())
        .filter(|&i| text[i..].starts_with(&query))
        .map(|i| {
            let mut score = 100 + 10 * query.len() as u32;
            if i == 0 {
                score += 60;
            } else if word_start(i) {
                score += 40;
            }
            score.saturating_sub(i as u32)
        })
        .max();
    if substring.is_some() {
        return substring;
    }
    // Otherwise every character in order, rewarding runs and word starts.
    let mut score = 0u32;
    let mut at = 0usize;
    let mut previous: Option<usize> = None;
    for &c in &query {
        let i = (at..text.len()).find(|&i| text[i] == c)?;
        score += 4;
        if previous == Some(i.wrapping_sub(1)) {
            score += 8;
        }
        if word_start(i) {
            score += 10;
        }
        previous = Some(i);
        at = i + 1;
    }
    Some(score)
}

/// Scores an entry for a (possibly multi-word) query; every word must match
/// the title, detail or keywords. Title matches count double.
fn score(entry: &Entry, query: &str) -> Option<u32> {
    query.split_whitespace().try_fold(0u32, |total, word| {
        let title = fuzzy(&entry.title, word).map(|s| s * 2);
        let detail = fuzzy(&entry.detail, word);
        let keywords = entry
            .keywords
            .split_whitespace()
            .filter_map(|k| fuzzy(k, word))
            .max();
        [title, detail, keywords]
            .into_iter()
            .flatten()
            .max()
            .map(|s| total + s)
    })
}

/// A search prefix that narrows the palette.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// Everything.
    All,
    /// `>`: commands only.
    Commands,
    /// `@`: open windows.
    Windows,
    /// `/` or `~`: files.
    Files,
    /// `?`: straight to the assistant.
    Ask,
}

/// Splits a leading scope character off a query.
pub fn scope(query: &str) -> (Scope, &str) {
    let trimmed = query.trim_start();
    let scope = match trimmed.chars().next() {
        Some('>') => Scope::Commands,
        Some('@') => Scope::Windows,
        Some('/' | '~') => Scope::Files,
        Some('?') => Scope::Ask,
        _ => return (Scope::All, query.trim()),
    };
    // `/` and `~` stay part of a file query ("~/notes" searches for "notes").
    let rest = trimmed[1..].trim_start_matches('/').trim();
    (scope, rest)
}

fn in_scope(scope: Scope, category: Category) -> bool {
    match scope {
        Scope::All => true,
        Scope::Commands => category.is_command(),
        Scope::Windows => category == Category::Window,
        Scope::Files => category == Category::File,
        Scope::Ask => false,
    }
}

/// Indices into `entries` matching `query`, best first.
///
/// Files and app actions only show once something is typed (or once
/// picked), so an empty query lists apps, windows and commands, most used
/// first.
pub fn search(entries: &[Entry], query: &str, history: &History) -> Vec<usize> {
    let (scope, query) = scope(query);
    let mut hits: Vec<(u32, usize)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| in_scope(scope, e.category))
        .filter(|(_, e)| {
            // Files and app actions are many; an empty query only lists the
            // ones picked before.
            !(query.is_empty()
                && scope == Scope::All
                && matches!(e.category, Category::File | Category::AppAction)
                && history.boost(e) == 0)
        })
        .filter_map(|(i, e)| score(e, query).map(|s| (s + history.boost(e), i)))
        .collect();
    // Higher score first; ties keep catalog order (apps, windows, commands, ...).
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, i)| i).collect()
}

/// Whether the assistant should be offered first: the query reads like a
/// request (several words) and the assistant understands it, while the best
/// entry's title does not contain every word.
pub fn prefer_assistant(entries: &[Entry], hits: &[usize], query: &str) -> bool {
    let (scope, text) = scope(query);
    if scope == Scope::Ask {
        return true;
    }
    if scope != Scope::All || text.split_whitespace().count() < 2 {
        return false;
    }
    // Close: the best entry's title contains every typed word.
    let lower = text.to_lowercase();
    let close = hits.first().is_some_and(|&i| {
        let title = entries[i].title.to_lowercase();
        lower.split_whitespace().all(|w| title.contains(w))
    });
    !close && assistant::interpret(text).is_ok()
}

/// The assistant entry for a query: what it would do, or why it can't.
///
/// Session operations count as confirmed: the user typed them. Destructive
/// ones still need the palette's second Enter ([`Entry::confirm`]).
pub fn ask(query: &str) -> Entry {
    let (_, text) = scope(query);
    match assistant::interpret(text) {
        Ok(actions) => {
            let confirm = actions
                .iter()
                .any(|a| matches!(a, Action::Session { op, .. } if op.is_destructive()));
            let steps = actions.len();
            let mut entry = Entry::new(
                Category::Ask,
                "✨",
                format!("Ask derisk: {text}"),
                actions
                    .into_iter()
                    .map(|a| match a {
                        Action::Session { op, .. } => Action::Session {
                            op,
                            confirmed: true,
                        },
                        other => other,
                    })
                    .collect(),
            )
            .detail(if steps == 1 {
                "1 step".to_owned()
            } else {
                format!("{steps} steps")
            });
            entry.confirm = confirm;
            entry
        }
        Err(e) => Entry::new(
            Category::Ask,
            "✨",
            format!("Ask derisk: {text}"),
            Vec::new(),
        )
        .detail(e.to_string()),
    }
}

/// Everything the palette can offer right now.
///
/// `extra` holds host-provided entries (apps, settings pages); `files` are
/// paths from [`index_files`].
pub fn entries(shell: &Shell, extra: &[Entry], files: &[PathBuf]) -> Vec<Entry> {
    let mut out = Vec::new();
    let suggested = shell.habits.suggestions(shell.clock.hour, 5);

    // Apps: suggested ones first, so they lead an empty query.
    let mut apps: Vec<&Entry> = extra
        .iter()
        .filter(|e| e.category == Category::App)
        .collect();
    apps.sort_by_key(|e| {
        e.actions
            .iter()
            .find_map(|a| match a {
                Action::Launch { app } => suggested.iter().position(|s| s == app),
                _ => None,
            })
            .unwrap_or(usize::MAX)
    });
    out.extend(apps.into_iter().cloned());
    // Suggested commands that are not in the catalog (plain programs).
    for app in &suggested {
        let known = out
            .iter()
            .any(|e| e.actions == [Action::Launch { app: app.clone() }]);
        if !known {
            out.push(Entry::app(app, app, "Suggested", "🖥", &[]));
        }
    }

    // Windows on every workspace.
    let focused = shell.focused();
    for (i, &ws) in shell.workspaces().iter().enumerate() {
        for w in shell.windows_on(ws) {
            let (app, title) = shell.window_label(w).unwrap_or_default();
            let app = shell.apps.look(app).name;
            let minimized = shell.is_minimized(w);
            let action = if minimized {
                Action::Restore { window: w.get() }
            } else {
                Action::Focus { window: w.get() }
            };
            let name = if title.is_empty() { &app } else { title };
            let mut detail = format!("{app} · workspace {}", i + 1);
            if minimized {
                detail.push_str(" · minimized");
            }
            if Some(w) == focused {
                detail.push_str(" · focused");
            }
            out.push(
                Entry::new(Category::Window, "🗗", name, vec![action])
                    .detail(detail)
                    .keywords("window switch"),
            );
        }
    }

    // The focused app's own menus, then the shell's Window menu.
    if let Some(w) = focused {
        let window = Some(w.get());
        let (app, _) = shell.window_label(w).unwrap_or_default();
        let app = shell.apps.look(app).name;
        for m in shell.menus.app_menus(w.get()) {
            flatten(&m.entries, &m.title, &mut |id, path, shortcut| {
                let mut e = Entry::new(
                    Category::AppCommand,
                    "☰",
                    path.rsplit(" › ").next().unwrap_or(path),
                    vec![Action::ActivateMenu {
                        window,
                        item: id.to_owned(),
                    }],
                )
                .detail(format!("{app} · {path}"));
                e.shortcut = shortcut.map(str::to_owned);
                out.push(e);
            });
        }
        let window_menu = menu::window_menu();
        flatten(&window_menu.entries, "Window", &mut |id, path, shortcut| {
            let Some(action) = menu::shell_action(id, w.get()) else {
                return;
            };
            let title = path.rsplit(" › ").next().unwrap_or(path);
            let title = if path.contains("Snap") {
                format!("Snap {title}")
            } else {
                title.to_owned()
            };
            let mut e = Entry::new(Category::Command, "🗖", title, vec![action])
                .detail(format!("Window · {app}"))
                .keywords("window");
            e.shortcut = shortcut.map(str::to_owned);
            out.push(e);
        });
    }

    // Shell commands.
    let commands = [
        (
            "Overview",
            "⊞",
            Action::Overview { visible: None },
            Some("Super"),
            "expose desktop show all",
        ),
        (
            "On-Screen Keyboard",
            "⌨",
            Action::Keyboard { visible: None },
            None,
            "osk virtual touch type keys show hide",
        ),
        (
            "Next Window",
            "🔄",
            Action::FocusNext,
            Some("Alt+Tab"),
            "focus switch cycle",
        ),
        (
            "Previous Window",
            "🔃",
            Action::FocusPrevious,
            Some("Alt+Shift+Tab"),
            "focus switch cycle back",
        ),
        (
            "Tall Layout",
            "⊟",
            Action::SetLayout {
                layout: LayoutKind::Tall,
            },
            Some("Super+Shift+M"),
            "tiling main stack",
        ),
        (
            "Monocle Layout",
            "▣",
            Action::SetLayout {
                layout: LayoutKind::Monocle,
            },
            Some("Super+M"),
            "tiling fullscreen one at a time",
        ),
    ];
    for (title, icon, action, shortcut, keywords) in commands {
        let mut e = Entry::new(Category::Command, icon, title, vec![action])
            .detail("Desktop")
            .keywords(keywords);
        e.shortcut = shortcut.map(str::to_owned);
        out.push(e);
    }
    // Workspaces, by position (they are dynamic; see `Shell::workspaces`).
    let active = shell.active_workspace();
    let shortcut = |chord: &str, n: u64| (n <= 9).then(|| format!("{chord}{n}"));
    for (i, &ws) in shell.workspaces().iter().enumerate() {
        let n = i as u64 + 1;
        if n != active {
            let count = shell.windows_on(ws).len();
            let mut e = Entry::new(
                Category::Workspace,
                "🖥",
                format!("Go to Workspace {n}"),
                vec![Action::SwitchWorkspace { workspace: n }],
            )
            .detail(match count {
                0 => "Empty".to_owned(),
                1 => "1 window".to_owned(),
                c => format!("{c} windows"),
            })
            .keywords("switch desktop");
            e.shortcut = shortcut("Super+", n);
            out.push(e);
            if focused.is_some() {
                let mut e = Entry::new(
                    Category::Workspace,
                    "⎆",
                    format!("Move Window to Workspace {n}"),
                    vec![Action::MoveToWorkspace {
                        window: None,
                        workspace: n,
                    }],
                )
                .keywords("send throw desktop");
                e.shortcut = shortcut("Super+Shift+", n);
                out.push(e);
            }
        }
    }
    if focused.is_some() && shell.can_add_workspace() {
        let n = shell.workspaces().len() as u64 + 1;
        let mut e = Entry::new(
            Category::Workspace,
            "⎆",
            "Move Window to New Workspace",
            vec![Action::MoveToWorkspace {
                window: None,
                workspace: n,
            }],
        )
        .keywords("send throw desktop space add");
        e.shortcut = shortcut("Super+Shift+", n);
        out.push(e);
    }

    // Session.
    for (title, icon, op, keywords) in [
        (
            "Lock Screen",
            "🔒",
            SessionOp::Lock,
            "lock away system session",
        ),
        ("Suspend", "🌙", SessionOp::Suspend, "sleep system power"),
        (
            "Hibernate",
            "❄",
            SessionOp::Hibernate,
            "sleep disk system power",
        ),
        (
            "Log Out",
            "🚪",
            SessionOp::Logout,
            "sign out logout exit system session",
        ),
        ("Restart", "⟳", SessionOp::Reboot, "reboot system power"),
        (
            "Shut Down",
            "✖",
            SessionOp::PowerOff,
            "power off poweroff shutdown system",
        ),
    ] {
        let mut e = Entry::new(
            Category::Session,
            icon,
            title,
            vec![Action::Session {
                op,
                confirmed: true,
            }],
        )
        .keywords(keywords);
        e.confirm = op.is_destructive();
        out.push(e);
    }

    // Tray items and failed services.
    for item in shell.tray.items() {
        out.push(
            Entry::new(
                Category::System,
                "★",
                item.title.clone(),
                vec![Action::ActivateTray {
                    id: item.id.clone(),
                    item: None,
                }],
            )
            .detail("Tray")
            .keywords(item.id.clone()),
        );
    }
    for unit in &shell.failed_units {
        out.push(
            Entry::new(
                Category::System,
                "⚠",
                format!("Restart {unit}"),
                vec![Action::RestartUnit { unit: unit.clone() }],
            )
            .detail("Failed service")
            .keywords("unit service systemd"),
        );
        out.push(
            Entry::new(
                Category::System,
                "⚠",
                format!("Dismiss {unit}"),
                vec![Action::ResetFailed { unit: unit.clone() }],
            )
            .detail("Failed service")
            .keywords("unit service systemd reset"),
        );
    }

    // Host extras that are not apps (settings pages, ...).
    out.extend(
        extra
            .iter()
            .filter(|e| e.category != Category::App)
            .cloned(),
    );

    // Files.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let shown = match &home {
            Some(h) => match path.strip_prefix(h) {
                Ok(rel) => format!("~/{}", rel.display()),
                Err(_) => path.display().to_string(),
            },
            None => path.display().to_string(),
        };
        let icon = if path.is_dir() { "🗀" } else { "🗋" };
        out.push(
            Entry::new(
                Category::File,
                icon,
                name,
                vec![Action::Open {
                    path: path.display().to_string(),
                }],
            )
            .detail(shown),
        );
    }
    out
}

/// Visits every enabled menu item as `(id, "Menu › Submenu › Item", shortcut)`.
fn flatten(entries: &[MenuEntry], path: &str, visit: &mut dyn FnMut(&str, &str, Option<&str>)) {
    for entry in entries {
        match entry {
            MenuEntry::Item {
                id,
                label,
                shortcut,
                enabled: true,
            } => visit(id, &format!("{path} › {label}"), shortcut.as_deref()),
            MenuEntry::Submenu { label, entries } => {
                flatten(entries, &format!("{path} › {label}"), visit);
            }
            MenuEntry::Item { .. } | MenuEntry::Separator => {}
        }
    }
}

/// Lists files and folders under `root` for the palette: at most `depth`
/// levels deep and `limit` paths, skipping hidden entries and symlinks.
///
/// Folders come before the files inside them, shallow before deep.
pub fn index_files(root: &Path, depth: usize, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut level = vec![root.to_path_buf()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for dir in level {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<_> = read
                .filter_map(Result::ok)
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter_map(|e| Some((e.path(), e.file_type().ok()?)))
                .filter(|(_, t)| !t.is_symlink())
                .collect();
            children.sort_by(|a, b| a.0.cmp(&b.0));
            for (path, kind) in children {
                if out.len() >= limit {
                    return out;
                }
                if kind.is_dir() {
                    next.push(path.clone());
                }
                out.push(path);
            }
        }
        level = next;
    }
    out
}

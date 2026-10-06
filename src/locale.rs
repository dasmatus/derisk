//! Languages, keyboard layouts and time zones: what the setup screens offer,
//! and how the choice is saved and read back.
//!
//! Everything is systemd's: `localectl` lists the installed locales and saves
//! the language and the keyboard layout (systemd-localed writes
//! /etc/locale.conf and the X11 keyboard file), and `timedatectl` lists and
//! sets the time zone. The only data of derisk's own is the native name of
//! each language, since neither glibc nor systemd has one to hand.
//!
//! A Wayland compositor has no X server to read the keyboard file, so
//! [`xkb_environment`] turns it into the `XKB_DEFAULT_*` variables
//! xkbcommon reads, and the display manager sets them for the login screen
//! and every session.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Where systemd-localed keeps the keyboard layout it was given.
pub const X11_KEYBOARD_CONF: &str = "/etc/X11/xorg.conf.d/00-keyboard.conf";

/// A language: a locale and how it names itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Language {
    /// The locale, such as `de_DE.UTF-8`.
    pub locale: String,
    /// Its name in its own language, such as `Deutsch (Deutschland)`; the
    /// locale itself when derisk does not know one.
    pub name: String,
}

/// Native names, by locale without its encoding. These are the languages
/// LosOS installs locales for; anything else installed still shows, under
/// its code. Japanese, Korean and Chinese are named in English: egui's
/// built-in fonts have no CJK glyphs, and the setup runs before any other
/// font could be chosen.
const NAMES: &[(&str, &str)] = &[
    ("cs_CZ", "Čeština"),
    ("da_DK", "Dansk"),
    ("de_AT", "Deutsch (Österreich)"),
    ("de_CH", "Deutsch (Schweiz)"),
    ("de_DE", "Deutsch (Deutschland)"),
    ("el_GR", "Ελληνικά"),
    ("en_AU", "English (Australia)"),
    ("en_CA", "English (Canada)"),
    ("en_GB", "English (United Kingdom)"),
    ("en_IE", "English (Ireland)"),
    ("en_IN", "English (India)"),
    ("en_US", "English (United States)"),
    ("es_ES", "Español (España)"),
    ("es_MX", "Español (México)"),
    ("fi_FI", "Suomi"),
    ("fr_CA", "Français (Canada)"),
    ("fr_FR", "Français (France)"),
    ("hu_HU", "Magyar"),
    ("it_IT", "Italiano"),
    ("ja_JP", "Japanese"),
    ("ko_KR", "Korean"),
    ("nb_NO", "Norsk bokmål"),
    ("nl_NL", "Nederlands"),
    ("pl_PL", "Polski"),
    ("pt_BR", "Português (Brasil)"),
    ("pt_PT", "Português (Portugal)"),
    ("ro_RO", "Română"),
    ("ru_RU", "Русский"),
    ("sk_SK", "Slovenčina"),
    ("sv_SE", "Svenska"),
    ("tr_TR", "Türkçe"),
    ("uk_UA", "Українська"),
    ("zh_CN", "Chinese (China)"),
    ("zh_TW", "Chinese (Taiwan)"),
];

/// The native name of `locale`, if derisk knows it.
pub fn language_name(locale: &str) -> Option<&'static str> {
    let base = locale.split(['.', '@']).next().unwrap_or(locale);
    NAMES.iter().find(|(l, _)| *l == base).map(|(_, n)| *n)
}

/// The UTF-8 locales in `locale -a` or `localectl list-locales` output, as
/// languages, sorted by name. `C.UTF-8` is left out: it is no one's
/// language. glibc's `xx_YY.utf8` spelling is written `xx_YY.UTF-8`, as
/// localectl and homectl are given it.
pub fn languages(list_locales: &str) -> Vec<Language> {
    let mut out: Vec<Language> = list_locales
        .lines()
        .map(str::trim)
        .filter(|l| {
            let upper = l.to_ascii_uppercase();
            let charset = upper.split('@').next().unwrap_or_default();
            (charset.ends_with(".UTF-8") || charset.ends_with(".UTF8")) && !upper.starts_with("C.")
        })
        .map(|locale| {
            let locale = match locale.split_once('.') {
                Some((base, rest)) => match rest.split_once('@') {
                    Some((_, modifier)) => format!("{base}.UTF-8@{modifier}"),
                    None => format!("{base}.UTF-8"),
                },
                None => locale.to_owned(),
            };
            Language {
                name: language_name(&locale).unwrap_or(&locale).to_owned(),
                locale,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        sort_key(&a.name)
            .cmp(&sort_key(&b.name))
            .then_with(|| a.locale.cmp(&b.locale))
    });
    out.dedup_by(|a, b| a.locale == b.locale);
    out
}

/// The plain letter under a Latin letter with a diacritic, for the letters
/// European names use; anything else unchanged.
pub fn fold_accent(c: char) -> char {
    const FROM: &str = "àáâãäåāăąçćčďđèéêëēėęěìíîïīįłľĺñńňòóôõöøōőŕřśšşťţùúûüūůűųýÿźżž";
    const TO: &str = "aaaaaaaaacccddeeeeeeeeiiiiiilllnnnoooooooorrsssttuuuuuuuuyyzzz";
    FROM.chars()
        .position(|f| f == c)
        .and_then(|i| TO.chars().nth(i))
        .unwrap_or(c)
}

/// How a name sorts: lowercase, accents dropped, so Čeština sits with the
/// Cs rather than after Z.
fn sort_key(name: &str) -> String {
    name.chars()
        .flat_map(char::to_lowercase)
        .map(fold_accent)
        .collect()
}

/// The installed languages, from `locale -a` or `localectl list-locales`.
pub fn installed_languages() -> Vec<Language> {
    // glibc's own list first: it reads the archive `LOCALE_ARCHIVE` names,
    // which is where NixOS keeps it. localectl only looks under
    // /usr/lib/locale, so it is the fallback for systems that use that.
    let from_glibc = languages(&output(&["locale", "-a"]));
    if from_glibc.is_empty() {
        languages(&output(&["localectl", "list-locales", "--no-pager"]))
    } else {
        from_glibc
    }
}

/// A keyboard layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    /// The XKB name, such as `de`.
    pub name: String,
    /// What xkeyboard-config calls it, such as `German`.
    pub description: String,
}

/// The layouts in an xkeyboard-config rules list (`base.lst`): the lines of
/// its `! layout` section, each a name and a description. Sorted by
/// description.
pub fn layouts(base_lst: &str) -> Vec<Layout> {
    let mut out = Vec::new();
    let mut in_layouts = false;
    for line in base_lst.lines() {
        if let Some(section) = line.strip_prefix('!') {
            in_layouts = section.trim() == "layout";
            continue;
        }
        if !in_layouts {
            continue;
        }
        let line = line.trim();
        if let Some((name, description)) = line.split_once(char::is_whitespace) {
            out.push(Layout {
                name: name.to_owned(),
                description: description.trim().to_owned(),
            });
        }
    }
    out.sort_by(|a, b| a.description.cmp(&b.description));
    out
}

/// Where xkeyboard-config's rules list may be: `$XKB_CONFIG_ROOT` first
/// (NixOS has no fixed path for it), then the usual places.
fn base_lst_paths() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("XKB_CONFIG_ROOT")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    roots.extend(
        [
            "/run/current-system/sw/share/X11/xkb",
            "/usr/share/X11/xkb",
            "/etc/X11/xkb",
        ]
        .map(PathBuf::from),
    );
    roots
        .into_iter()
        .map(|root| root.join("rules/base.lst"))
        .collect()
}

/// The keyboard layouts xkeyboard-config describes. When its list cannot be
/// found, the names from `localectl list-x11-keymap-layouts`, described by
/// themselves.
pub fn installed_layouts() -> Vec<Layout> {
    if let Some(text) = base_lst_paths()
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())
    {
        let found = layouts(&text);
        if !found.is_empty() {
            return found;
        }
    }
    output(&["localectl", "list-x11-keymap-layouts", "--no-pager"])
        .lines()
        .map(|l| Layout {
            name: l.trim().to_owned(),
            description: l.trim().to_owned(),
        })
        .filter(|l| !l.name.is_empty())
        .collect()
}

/// The keyboard as systemd-localed saved it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Keyboard {
    /// `XkbLayout`, such as `de` or `us,ru`.
    pub layout: String,
    /// `XkbVariant`.
    pub variant: String,
    /// `XkbOptions`.
    pub options: String,
}

/// Reads the `Option "Xkb..." "..."` lines of localed's keyboard file.
pub fn parse_x11_keyboard(conf: &str) -> Keyboard {
    let mut keyboard = Keyboard::default();
    for line in conf.lines() {
        let mut quoted = line.split('"').skip(1).step_by(2);
        if !line.trim_start().starts_with("Option") {
            continue;
        }
        let (Some(key), Some(value)) = (quoted.next(), quoted.next()) else {
            continue;
        };
        match key {
            "XkbLayout" => keyboard.layout = value.to_owned(),
            "XkbVariant" => keyboard.variant = value.to_owned(),
            "XkbOptions" => keyboard.options = value.to_owned(),
            _ => {}
        }
    }
    keyboard
}

/// The saved keyboard, or the default (empty) when none was saved.
pub fn saved_keyboard(path: &Path) -> Keyboard {
    fs::read_to_string(path)
        .map(|c| parse_x11_keyboard(&c))
        .unwrap_or_default()
}

/// `XKB_DEFAULT_*` pairs for the saved keyboard: what xkbcommon reads when a
/// compositor asks for the default keymap. Empty fields are left out, so
/// xkbcommon's own defaults stand.
pub fn xkb_environment(keyboard: &Keyboard) -> Vec<String> {
    [
        ("XKB_DEFAULT_LAYOUT", &keyboard.layout),
        ("XKB_DEFAULT_VARIANT", &keyboard.variant),
        ("XKB_DEFAULT_OPTIONS", &keyboard.options),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| format!("{k}={v}"))
    .collect()
}

/// The time zones `timedatectl` knows, such as `Europe/Bratislava`.
pub fn installed_time_zones() -> Vec<String> {
    let mut zones: Vec<String> = output(&["timedatectl", "list-timezones", "--no-pager"])
        .lines()
        .map(str::trim)
        .filter(|z| !z.is_empty())
        .map(str::to_owned)
        .collect();
    if zones.is_empty() {
        zones.push("UTC".into());
    }
    zones
}

/// The time zone the machine is in now: where /etc/localtime points under
/// a `zoneinfo` directory, or UTC.
pub fn current_time_zone() -> String {
    fs::read_link("/etc/localtime")
        .ok()
        .and_then(|target| time_zone_of(&target))
        .unwrap_or_else(|| "UTC".into())
}

/// The zone name in a path into a zoneinfo tree.
pub fn time_zone_of(target: &Path) -> Option<String> {
    let text = target.to_string_lossy();
    let (_, zone) = text.split_once("zoneinfo/")?;
    (!zone.is_empty()).then(|| zone.to_owned())
}

/// How a time zone reads in a list: `Europe/Bratislava` becomes
/// `Bratislava, Europe`, and underscores become spaces, so typing a city
/// finds it.
pub fn time_zone_label(zone: &str) -> String {
    let pretty = zone.replace('_', " ");
    match pretty.rsplit_once('/') {
        Some((region, city)) => format!("{city}, {}", region.replace('/', ", ")),
        None => pretty,
    }
}

/// Whether every word of `query` appears in `text`, ignoring case: how the
/// setup screens' search fields match.
pub fn matches(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// The command that saves `locale` as the system's language.
pub fn set_locale_argv(locale: &str) -> Vec<String> {
    vec![
        "localectl".into(),
        "set-locale".into(),
        format!("LANG={locale}"),
    ]
}

/// The command that saves `layout` as the keyboard, for Wayland and X11
/// sessions and, converted by localed, the text console.
pub fn set_keyboard_argv(layout: &str) -> Vec<String> {
    vec!["localectl".into(), "set-x11-keymap".into(), layout.into()]
}

/// The command that sets the time zone.
pub fn set_time_zone_argv(zone: &str) -> Vec<String> {
    vec!["timedatectl".into(), "set-timezone".into(), zone.into()]
}

fn output(argv: &[&str]) -> String {
    Command::new(argv[0])
        .args(&argv[1..])
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

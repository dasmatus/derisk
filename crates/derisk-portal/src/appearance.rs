//! `org.freedesktop.appearance`, from the theme derisk publishes.
//!
//! derisk writes `$XDG_RUNTIME_DIR/derisk/theme.json` whenever the theme
//! changes (see `derisk::theme`), and mcsapi-theme already puts the portal's
//! three values in it, under `"portal"`:
//!
//! ```text
//! "portal":{"color_scheme":1,"accent_color":[0.64,0.9,0.21],"contrast":0}
//! ```
//!
//! So this module only reads that block back; it never works out a color
//! scheme of its own, and an app asking the portal sees exactly what the
//! shell drew.

use std::collections::HashMap;

use ashpd::zvariant::{OwnedValue, Value};
use serde_json::Value as Json;

/// The namespace this backend answers for.
pub const NAMESPACE: &str = "org.freedesktop.appearance";

/// The appearance keys, as the portal spec spells them.
pub const COLOR_SCHEME: &str = "color-scheme";
/// See [`COLOR_SCHEME`].
pub const ACCENT_COLOR: &str = "accent-color";
/// See [`COLOR_SCHEME`].
pub const CONTRAST: &str = "contrast";

/// The values the portal serves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    /// 0 no preference, 1 dark, 2 light.
    pub color_scheme: u32,
    /// sRGB, each channel 0 to 1.
    pub accent_color: (f64, f64, f64),
    /// 0 no preference, 1 high.
    pub contrast: u32,
}

impl Appearance {
    /// Reads the `"portal"` block of a `theme.json`. `None` when the file
    /// has none or it is malformed, so the GTK backend's answer stands.
    pub fn parse(json: &str) -> Option<Self> {
        let theme: Json = serde_json::from_str(json).ok()?;
        let portal = theme.get("portal")?;
        let accent = portal.get("accent_color")?.as_array()?;
        let channel = |i: usize| {
            accent
                .get(i)
                .and_then(Json::as_f64)
                .filter(|c| (0.0..=1.0).contains(c))
        };
        let small = |key: &str, max: u64| {
            portal
                .get(key)?
                .as_u64()
                .filter(|v| *v <= max)
                .map(|v| v as u32)
        };
        Some(Self {
            color_scheme: small("color_scheme", 2)?,
            accent_color: (channel(0)?, channel(1)?, channel(2)?),
            contrast: small("contrast", 1)?,
        })
    }

    /// One key's value, typed as the spec says: `u` for the scheme and
    /// contrast, `(ddd)` for the accent.
    pub fn get(&self, key: &str) -> Option<OwnedValue> {
        let value = match key {
            COLOR_SCHEME => Value::from(self.color_scheme),
            ACCENT_COLOR => Value::from(self.accent_color),
            CONTRAST => Value::from(self.contrast),
            _ => return None,
        };
        // Only values holding file descriptors fail to become owned.
        OwnedValue::try_from(value).ok()
    }

    /// Every key and its value.
    pub fn namespace(&self) -> HashMap<String, OwnedValue> {
        [COLOR_SCHEME, ACCENT_COLOR, CONTRAST]
            .into_iter()
            .filter_map(|key| Some((key.to_owned(), self.get(key)?)))
            .collect()
    }

    /// The keys whose value differs from `before`, for `SettingChanged`.
    /// Without a `before`, every key has changed.
    pub fn changed_since(&self, before: Option<&Self>) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if before.is_none_or(|b| b.color_scheme != self.color_scheme) {
            keys.push(COLOR_SCHEME);
        }
        if before.is_none_or(|b| b.accent_color != self.accent_color) {
            keys.push(ACCENT_COLOR);
        }
        if before.is_none_or(|b| b.contrast != self.contrast) {
            keys.push(CONTRAST);
        }
        keys
    }
}

/// Whether `ReadAll`'s `namespaces` ask for `namespace`. An empty list asks
/// for everything, and a pattern ending in `*` matches by prefix, as the
/// portal spec allows (`org.freedesktop.*`).
pub fn requested(patterns: &[String], namespace: &str) -> bool {
    patterns.is_empty()
        || patterns
            .iter()
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => namespace.starts_with(prefix),
                None => pattern == namespace,
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: &str = r#"{"id":"derisk-dark","scheme":"dark",
        "portal":{"color_scheme":1,"accent_color":[0.64,0.9,0.21],"contrast":0}}"#;

    #[test]
    fn reads_the_portal_block() {
        let a = Appearance::parse(DARK).unwrap();
        assert_eq!(a.color_scheme, 1);
        assert_eq!(a.accent_color, (0.64, 0.9, 0.21));
        assert_eq!(a.contrast, 0);
        assert_eq!(a.namespace().len(), 3);
        assert_eq!(u32::try_from(a.get(COLOR_SCHEME).unwrap()).unwrap(), 1);
        let accent = a.get(ACCENT_COLOR).unwrap();
        assert_eq!(accent.value_signature().to_string(), "(ddd)");
    }

    #[test]
    fn refuses_what_the_spec_does_not_allow() {
        assert_eq!(Appearance::parse(r#"{"scheme":"dark"}"#), None);
        assert_eq!(Appearance::parse("not json"), None);
        let bad_scheme = DARK.replace("\"color_scheme\":1", "\"color_scheme\":7");
        assert_eq!(Appearance::parse(&bad_scheme), None);
        let bad_accent = DARK.replace("0.64", "64");
        assert_eq!(Appearance::parse(&bad_accent), None);
    }

    #[test]
    fn reports_only_changed_keys() {
        let dark = Appearance::parse(DARK).unwrap();
        let light = Appearance {
            color_scheme: 2,
            ..dark
        };
        assert_eq!(light.changed_since(Some(&dark)), vec![COLOR_SCHEME]);
        assert_eq!(dark.changed_since(Some(&dark)), Vec::<&str>::new());
        assert_eq!(dark.changed_since(None).len(), 3);
    }

    #[test]
    fn matches_namespace_patterns() {
        assert!(requested(&[], NAMESPACE));
        assert!(requested(&["org.freedesktop.*".into()], NAMESPACE));
        assert!(requested(&[NAMESPACE.into()], NAMESPACE));
        assert!(!requested(&["org.gnome.*".into()], NAMESPACE));
        assert!(!requested(
            &["org.freedesktop.appearance.x".into()],
            NAMESPACE
        ));
    }
}

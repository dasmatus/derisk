//! The on-screen keyboard for phones: its keys, shift and symbol pages, and
//! word prediction.
//!
//! A phone has no hardware keyboard, so the shell draws one above the
//! navigation bar (see [`crate::ui`]). This module decides what each key
//! types and which words to suggest, without a toolkit, so it can be tested
//! headless and drawn by egui today or gpui later.
//!
//! Prediction learns from what is typed: words already used, and the word
//! that usually follows the previous one, rank above the built-in list.
//! Password fields must call [`Keyboard::set_learning`]`(false)` first.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use mcsapi::Geometry;

use crate::geom::rect;

/// Height of one row of keys, in logical pixels.
pub const ROW_HEIGHT: i32 = 48;
/// Height of the suggestion strip above the keys.
pub const SUGGESTION_HEIGHT: i32 = 40;
/// Rows of keys on every page.
pub const ROWS: i32 = 4;
/// The keyboard's total height.
pub const HEIGHT: i32 = SUGGESTION_HEIGHT + ROWS * ROW_HEIGHT + 8;
/// Suggestions shown at once.
pub const SUGGESTIONS: usize = 3;

/// A key on the keyboard.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Key {
    /// Types a character (shifted on the letter page when shift is on).
    Char(char),
    /// Shift for one letter; twice in a row locks it.
    Shift,
    /// Deletes the character before the cursor.
    Backspace,
    /// Switches between the letter and symbol pages.
    Page,
    /// Types a space and finishes the word.
    Space,
    /// Return.
    Enter,
}

impl Key {
    /// The symbolic icon the key shows instead of a label, if any (see
    /// [`crate::icons`]). Caps Lock shows as a lit Shift.
    pub fn icon(self) -> Option<&'static str> {
        match self {
            Self::Shift => Some("keyboard-shift-filled"),
            Self::Backspace => Some("edit-clear"),
            _ => None,
        }
    }

    /// What the key shows, given the keyboard's state: its text, or for a
    /// key with an [`icon`](Self::icon) the glyph standing in for it.
    pub fn label(self, keyboard: &Keyboard) -> String {
        match self {
            Self::Char(c) => keyboard.shifted(c).to_string(),
            Self::Shift | Self::Backspace => {
                crate::icons::glyph(self.icon().unwrap_or_default()).into()
            }
            Self::Page => match keyboard.page {
                Page::Letters => "?123".into(),
                Page::Symbols => "ABC".into(),
            },
            Self::Space => "space".into(),
            Self::Enter => "enter".into(),
        }
    }

    /// The name assistive tools read out.
    pub fn name(self) -> String {
        match self {
            Self::Char(c) => c.to_string(),
            Self::Shift => "Shift".into(),
            Self::Backspace => "Backspace".into(),
            Self::Page => "Switch letters and symbols".into(),
            Self::Space => "Space".into(),
            Self::Enter => "Enter".into(),
        }
    }
}

/// Which keys are showing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Page {
    /// QWERTY letters.
    #[default]
    Letters,
    /// Digits and punctuation.
    Symbols,
}

/// The shift state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Shift {
    /// Lower case.
    #[default]
    Off,
    /// The next letter is upper case.
    Once,
    /// Every letter is upper case.
    Lock,
}

/// What a key press does to the focused text field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Output {
    /// Insert text at the cursor.
    Text(String),
    /// Delete the character before the cursor.
    Backspace,
    /// Return.
    Enter,
}

/// Each row's keys and their widths, in tenths of the keyboard's width.
fn rows(page: Page) -> [Vec<(Key, i32)>; 4] {
    let chars = |s: &str| s.chars().map(|c| (Key::Char(c), 10)).collect::<Vec<_>>();
    let bottom = vec![
        (Key::Page, 15),
        (Key::Char(','), 10),
        (Key::Space, 50),
        (Key::Char('.'), 10),
        (Key::Enter, 15),
    ];
    match page {
        Page::Letters => {
            let mut third = vec![(Key::Shift, 15)];
            third.extend(chars("zxcvbnm"));
            third.push((Key::Backspace, 15));
            [chars("qwertyuiop"), chars("asdfghjkl"), third, bottom]
        }
        Page::Symbols => {
            let mut third = chars("*\"':;!?");
            third.insert(0, (Key::Char('_'), 15));
            third.push((Key::Backspace, 15));
            [chars("1234567890"), chars("@#$%&-+()/"), third, bottom]
        }
    }
}

/// The keyboard's state and its word predictor.
#[derive(Debug, Default)]
pub struct Keyboard {
    page: Page,
    shift: Shift,
    /// The word being typed, as far as this keyboard has seen it.
    word: String,
    /// The word before it, for next-word prediction.
    previous: Option<String>,
    learning: bool,
    /// Words and word pairs, learned and built in.
    pub predictor: Predictor,
}

impl Keyboard {
    /// A keyboard with the built-in word list, starting a sentence.
    pub fn new() -> Self {
        Self {
            shift: Shift::Once,
            learning: true,
            predictor: Predictor::with_builtin_words(),
            ..Self::default()
        }
    }

    /// The page showing.
    pub fn page(&self) -> Page {
        self.page
    }

    /// The shift state.
    pub fn shift(&self) -> Shift {
        self.shift
    }

    /// The word being typed.
    pub fn word(&self) -> &str {
        &self.word
    }

    /// Whether typed words are learned. Turn it off for password fields, so
    /// a password never ends up in the word list on disk.
    pub fn set_learning(&mut self, learning: bool) {
        self.learning = learning;
    }

    /// Forgets the word in progress, for when focus moves to another field:
    /// the keyboard can't see what that field already holds.
    pub fn reset(&mut self) {
        self.word.clear();
        self.previous = None;
        self.page = Page::Letters;
        self.shift = Shift::Once;
    }

    fn shifted(&self, c: char) -> char {
        if self.page == Page::Letters && self.shift != Shift::Off {
            c.to_ascii_uppercase()
        } else {
            c
        }
    }

    /// Each key's tap area inside `area`, top row first, below the
    /// suggestion strip. Every row spans the full width.
    pub fn keys(&self, area: Geometry) -> impl Iterator<Item = (Key, Geometry)> + use<> {
        let top = area.loc.y + SUGGESTION_HEIGHT;
        rows(self.page)
            .into_iter()
            .enumerate()
            .flat_map(move |(r, row)| {
                let units: i32 = row.iter().map(|(_, w)| w).sum();
                // Short rows (the home row) are centered, as on a real keyboard.
                let full = units.max(100);
                let start = area.loc.x * full + (full - units) * area.size.w / 2;
                let y = top + r as i32 * ROW_HEIGHT;
                row.into_iter().scan(start, move |x, (key, w)| {
                    let left = *x / full;
                    *x += w * area.size.w;
                    Some((key, rect(left, y, *x / full - left, ROW_HEIGHT)))
                })
            })
    }

    /// The suggestion strip's cells inside `area`.
    pub fn suggestion_cells(area: Geometry) -> [Geometry; SUGGESTIONS] {
        let n = SUGGESTIONS as i32;
        let w = area.size.w / n;
        std::array::from_fn(|i| {
            let i = i as i32;
            let width = if i == n - 1 { area.size.w - w * i } else { w };
            rect(area.loc.x + i * w, area.loc.y, width, SUGGESTION_HEIGHT)
        })
    }

    /// Up to [`SUGGESTIONS`] words for the strip: completions of the word
    /// being typed, or likely next words after a space.
    pub fn suggestions(&self) -> Vec<String> {
        self.predictor
            .complete(&self.word, self.previous.as_deref(), SUGGESTIONS)
    }

    /// Presses `key` and returns what it types.
    pub fn press(&mut self, key: Key) -> Vec<Output> {
        match key {
            Key::Shift => {
                self.shift = match self.shift {
                    Shift::Off => Shift::Once,
                    Shift::Once => Shift::Lock,
                    Shift::Lock => Shift::Off,
                };
                Vec::new()
            }
            Key::Page => {
                self.page = match self.page {
                    Page::Letters => Page::Symbols,
                    Page::Symbols => Page::Letters,
                };
                Vec::new()
            }
            Key::Backspace => {
                if self.word.pop().is_none() {
                    // Deleting into text the keyboard didn't type: it can't
                    // tell what word that was.
                    self.previous = None;
                }
                vec![Output::Backspace]
            }
            Key::Space => {
                self.finish_word();
                vec![Output::Text(" ".into())]
            }
            Key::Enter => {
                self.finish_word();
                self.previous = None;
                self.shift = Shift::Once;
                vec![Output::Enter]
            }
            Key::Char(c) => {
                let c = self.shifted(c);
                if self.shift == Shift::Once && c.is_alphabetic() {
                    self.shift = Shift::Off;
                }
                if c.is_alphanumeric() || c == '\'' || c == '-' {
                    self.word.push(c);
                } else {
                    self.finish_word();
                    self.previous = None;
                    if matches!(c, '.' | '!' | '?') {
                        self.shift = Shift::Once;
                    }
                }
                vec![Output::Text(c.to_string())]
            }
        }
    }

    /// Replaces the word being typed with `word` and a space, as when a
    /// suggestion is tapped.
    pub fn choose(&mut self, word: &str) -> Vec<Output> {
        let mut out = vec![Output::Backspace; self.word.chars().count()];
        out.push(Output::Text(format!("{word} ")));
        self.word = word.to_owned();
        self.finish_word();
        if self.page == Page::Letters && self.shift == Shift::Once {
            self.shift = Shift::Off;
        }
        out
    }

    fn finish_word(&mut self) {
        let word = std::mem::take(&mut self.word);
        if word.is_empty() {
            return;
        }
        if self.learning {
            self.predictor.learn(&word, self.previous.as_deref());
        }
        self.previous = Some(word);
    }
}

/// Built-in English words, most common first. Learned words outrank them.
const WORDS: &str = "the be to of and a in that have i it for not on with he as you do at \
this but his by from they we say her she or an will my one all would there their what so up \
out if about who get which go me when make can like time no just him know take people into \
year your good some could them see other than then now look only come its over think also \
back after use two how our work first well way even new want because any these give day most \
us is are was were has had did does been being am i'm don't it's can't won't didn't that's \
you're i'll i've let's thanks thank please hello hi yes okay ok sure sorry maybe today \
tomorrow tonight morning evening night week month later soon here where why again still \
never always very really much many more less little big long great small old same right \
left next last find tell ask call try need feel leave put mean keep begin seem help show \
hear play run move live believe hold bring happen write provide sit stand lose pay meet \
include continue set learn change lead understand watch follow stop create speak read allow \
add spend grow open walk win offer remember love consider appear buy wait serve die send \
expect build stay fall cut reach kill remain suggest raise pass sell require report decide \
pull home house car phone message email file files folder settings search open close save \
copy paste delete edit text note notes photo picture music video game app apps window screen \
battery wifi network update download upload password account name address number place \
point thing things life world school state family student group country problem hand part \
case company system program question government night water room mother area money story fact \
lot study book eye job word business issue side kind head service friend father power hour \
end member law line city community president team minute idea kid body information face \
others level office door health person art war history party result morning reason research \
girl guy moment air teacher force education food lunch dinner breakfast coffee meeting work \
free busy happy sad nice cool fine late early best better bad worse sorry ready done able \
should must might shall may around through before between during without under within along \
following across behind beyond plus except up down off above near every each another such \
both few those own enough quite rather almost already yet ever together often sometimes \
usually probably actually maybe perhaps exactly instead anyway though although while until \
since unless whether either neither";

/// Scores words and word pairs for completion.
#[derive(Clone, Debug, Default)]
pub struct Predictor {
    /// Built-in rank weight plus what was learned.
    words: HashMap<String, u32>,
    /// How often each word followed another.
    pairs: HashMap<(String, String), u32>,
    learned: HashMap<String, u32>,
    dirty: bool,
}

/// How much one use of a word counts against the built-in list's weights.
const LEARNED_WEIGHT: u32 = 400;

impl Predictor {
    /// A predictor that knows the built-in word list.
    pub fn with_builtin_words() -> Self {
        let mut p = Self::default();
        let words: Vec<&str> = WORDS.split_whitespace().collect();
        let n = words.len() as u32;
        for (rank, word) in words.into_iter().enumerate() {
            // Earlier words are more common: n for the first, 1 for the last.
            p.words.entry(word.to_owned()).or_insert(n - rank as u32);
        }
        p
    }

    /// Adds words the person is likely to type (app names, contacts), at
    /// `weight`, without counting them as learned.
    pub fn add_vocabulary<'a>(&mut self, words: impl IntoIterator<Item = &'a str>, weight: u32) {
        for word in words {
            let word = word.trim().to_lowercase();
            if word.chars().count() > 1 && word.chars().all(|c| c.is_alphanumeric() || c == '\'') {
                let w = self.words.entry(word).or_insert(0);
                *w = (*w).max(weight);
            }
        }
    }

    /// Records a typed word, and the word before it.
    pub fn learn(&mut self, word: &str, previous: Option<&str>) {
        let word = word.to_lowercase();
        if word.chars().count() < 2 || word.chars().count() > 32 {
            return;
        }
        *self.learned.entry(word.clone()).or_default() += 1;
        if let Some(previous) = previous {
            *self
                .pairs
                .entry((previous.to_lowercase(), word.clone()))
                .or_default() += 1;
        }
        self.words.entry(word).or_insert(0);
        self.dirty = true;
    }

    fn score(&self, word: &str, previous: Option<&str>) -> u64 {
        let base = u64::from(self.words.get(word).copied().unwrap_or(0));
        let learned = u64::from(self.learned.get(word).copied().unwrap_or(0));
        let pair = previous
            .and_then(|p| self.pairs.get(&(p.to_lowercase(), word.to_owned())))
            .copied()
            .map_or(0, u64::from);
        base + learned * u64::from(LEARNED_WEIGHT) + pair * u64::from(LEARNED_WEIGHT) * 4
    }

    /// Up to `n` suggestions for `prefix` after `previous`. With an empty
    /// prefix, the words that most often followed `previous`. Otherwise
    /// completions first, then corrections of one typo, keeping the case of
    /// the prefix's first letter.
    pub fn complete(&self, prefix: &str, previous: Option<&str>, n: usize) -> Vec<String> {
        let lower = prefix.to_lowercase();
        if lower.is_empty() {
            let Some(previous) = previous.map(str::to_lowercase) else {
                return Vec::new();
            };
            let mut next: Vec<(&String, u32)> = self
                .pairs
                .iter()
                .filter(|((p, _), _)| *p == previous)
                .map(|((_, w), c)| (w, *c))
                .collect();
            next.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            return next.into_iter().take(n).map(|(w, _)| w.clone()).collect();
        }
        let rank = |words: Vec<&String>| {
            let mut scored: Vec<(u64, &String)> = words
                .into_iter()
                .map(|w| (self.score(w, previous), w))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            scored
                .into_iter()
                .map(|(_, w)| w.clone())
                .collect::<Vec<_>>()
        };
        let mut out = rank(
            self.words
                .keys()
                .filter(|w| w.starts_with(&lower) && **w != lower)
                .collect(),
        );
        out.truncate(n);
        if out.len() < n {
            let fixes = rank(
                self.words
                    .keys()
                    .filter(|w| **w != lower && !w.starts_with(&lower) && one_edit(w, &lower))
                    .collect(),
            );
            out.extend(fixes.into_iter().take(n - out.len()));
        }
        let capital = prefix.chars().next().is_some_and(char::is_uppercase);
        out.into_iter()
            .map(|w| if capital { capitalize(&w) } else { w })
            .collect()
    }

    /// Whether something was learned since the last [`Predictor::save`].
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// `$XDG_DATA_HOME/derisk/keyboard-words` (or `~/.local/share/...`).
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("derisk").join("keyboard-words"))
    }

    /// Writes what was learned: `word count` and `previous word count` lines.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        let mut text = String::new();
        let mut words: Vec<_> = self.learned.iter().collect();
        words.sort();
        for (w, c) in words {
            text.push_str(&format!("{w} {c}\n"));
        }
        let mut pairs: Vec<_> = self.pairs.iter().collect();
        pairs.sort();
        for ((p, w), c) in pairs {
            text.push_str(&format!("{p} {w} {c}\n"));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Written aside and renamed, so a crash never leaves half a file.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(())
    }

    /// Reads what [`Predictor::save`] wrote, ignoring lines it can't parse.
    pub fn load(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.as_slice() {
                [w, c] => {
                    if let Ok(c) = c.parse() {
                        self.learned.insert((*w).to_owned(), c);
                        self.words.entry((*w).to_owned()).or_insert(0);
                    }
                }
                [p, w, c] => {
                    if let Ok(c) = c.parse() {
                        self.pairs.insert(((*p).to_owned(), (*w).to_owned()), c);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Whether `a` and `b` differ by one inserted, deleted, replaced or swapped
/// character.
fn one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 || b.len() < 2 {
        return false;
    }
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let (ra, rb) = (&a[start..], &b[start..]);
    match ra.len().cmp(&rb.len()) {
        std::cmp::Ordering::Equal => {
            ra[1..] == rb[1..]
                || (ra.len() >= 2 && ra[0] == rb[1] && ra[1] == rb[0] && ra[2..] == rb[2..])
        }
        std::cmp::Ordering::Greater => ra[1..] == *rb,
        std::cmp::Ordering::Less => *ra == rb[1..],
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(keyboard: &mut Keyboard, text: &str) -> Vec<Output> {
        text.chars()
            .flat_map(|c| {
                let key = match c {
                    ' ' => Key::Space,
                    '\n' => Key::Enter,
                    c => Key::Char(c.to_ascii_lowercase()),
                };
                keyboard.press(key)
            })
            .collect()
    }

    #[test]
    fn starts_capitalized_and_shift_applies_once() {
        let mut k = Keyboard::new();
        let out = typed(&mut k, "hi");
        assert_eq!(
            out,
            vec![Output::Text("H".into()), Output::Text("i".into())]
        );
        k.press(Key::Shift);
        k.press(Key::Shift);
        assert_eq!(k.shift(), Shift::Lock);
        typed(&mut k, "ok");
        assert_eq!(k.word(), "HiOK");
    }

    #[test]
    fn a_full_stop_capitalizes_the_next_word() {
        let mut k = Keyboard::new();
        typed(&mut k, "done. ");
        assert_eq!(k.shift(), Shift::Once);
    }

    #[test]
    fn completes_from_the_word_list() {
        let mut k = Keyboard::new();
        k.reset();
        k.shift = Shift::Off;
        typed(&mut k, "tom");
        assert!(
            k.suggestions().contains(&"tomorrow".to_owned()),
            "{:?}",
            k.suggestions()
        );
    }

    #[test]
    fn suggests_corrections_for_one_typo() {
        let p = Predictor::with_builtin_words();
        assert!(p.complete("teh", None, 3).contains(&"the".to_owned()));
        assert!(p.complete("helo", None, 3).contains(&"hello".to_owned()));
    }

    #[test]
    fn keeps_the_case_of_the_first_letter() {
        let p = Predictor::with_builtin_words();
        assert!(p.complete("Tom", None, 3).contains(&"Tomorrow".to_owned()));
    }

    #[test]
    fn learned_words_and_pairs_win() {
        let mut k = Keyboard::new();
        k.shift = Shift::Off;
        for _ in 0..3 {
            typed(&mut k, "derisk rocks ");
        }
        typed(&mut k, "der");
        assert_eq!(k.suggestions()[0], "derisk");
        // After "derisk " with nothing typed, the usual next word.
        k.choose("derisk");
        assert_eq!(k.suggestions()[0], "rocks");
    }

    #[test]
    fn choosing_replaces_the_word_in_progress() {
        let mut k = Keyboard::new();
        k.shift = Shift::Off;
        typed(&mut k, "tom");
        let out = k.choose("tomorrow");
        assert_eq!(
            out,
            vec![
                Output::Backspace,
                Output::Backspace,
                Output::Backspace,
                Output::Text("tomorrow ".into())
            ]
        );
        assert_eq!(k.word(), "");
    }

    #[test]
    fn passwords_are_not_learned() {
        let mut k = Keyboard::new();
        k.set_learning(false);
        typed(&mut k, "hunter2 ");
        assert!(!k.predictor.is_dirty());
        assert!(
            k.predictor
                .complete("hunt", None, 3)
                .iter()
                .all(|w| w != "hunter2")
        );
    }

    #[test]
    fn learned_words_survive_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("derisk-kbd-{}", std::process::id()));
        let path = dir.join("keyboard-words");
        let mut p = Predictor::with_builtin_words();
        p.learn("smithay", Some("built"));
        p.save(&path).unwrap();
        let mut q = Predictor::with_builtin_words();
        q.load(&path);
        assert_eq!(q.complete("smi", None, 1), vec!["smithay".to_owned()]);
        assert_eq!(q.complete("", Some("built"), 1), vec!["smithay".to_owned()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keys_fill_each_row_and_reach_both_edges() {
        let k = Keyboard::new();
        let area = rect(0, 500, 392, HEIGHT);
        let keys: Vec<_> = k.keys(area).collect();
        let top_row: Vec<_> = keys
            .iter()
            .filter(|(_, g)| g.loc.y == 500 + SUGGESTION_HEIGHT)
            .collect();
        assert_eq!(top_row.len(), 10);
        assert_eq!(top_row[0].1.loc.x, 0);
        let last = top_row[9].1;
        assert_eq!(last.loc.x + last.size.w, 392);
        let space = keys.iter().find(|(k, _)| *k == Key::Space).unwrap().1;
        assert!(space.size.w > 150);
        assert!(keys.iter().all(|(_, g)| g.size.h == ROW_HEIGHT));
    }

    #[test]
    fn one_edit_distance() {
        assert!(one_edit("the", "teh"));
        assert!(one_edit("hello", "helo"));
        assert!(one_edit("helo", "hello"));
        assert!(one_edit("cat", "cut"));
        assert!(!one_edit("cat", "dog"));
    }
}

//! The session's side of Settings → Privacy: forgetting recently used
//! files and emptying old trash.
//!
//! GTK and Qt apps share `$XDG_DATA_HOME/recently-used.xbel`; with history
//! off the session deletes it whenever an app writes it again. Trashed
//! files follow the freedesktop.org trash spec: `Trash/info/<name>.trashinfo`
//! records a `DeletionDate` for `Trash/files/<name>`.

use std::{
    io,
    path::{Path, PathBuf},
};

use derisk_settings::Privacy;

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
}

/// What a sweep removed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Swept {
    /// The recently used list was deleted.
    pub recent: bool,
    /// Trashed items deleted for good.
    pub trashed: usize,
}

/// Applies `privacy` under `data` (see [`data_home`]) at `now` (Unix
/// seconds).
pub fn sweep(privacy: &Privacy, data: &Path, now: i64) -> io::Result<Swept> {
    let mut swept = Swept::default();
    if !privacy.remember_recent {
        match std::fs::remove_file(data.join("recently-used.xbel")) {
            Ok(()) => swept.recent = true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    if privacy.empty_trash_days > 0 {
        let cutoff = now - i64::from(privacy.empty_trash_days) * 86_400;
        swept.trashed = empty_trash_before(&data.join("Trash"), cutoff)?;
    }
    Ok(swept)
}

/// Deletes trashed items whose `DeletionDate` is before `cutoff` (Unix
/// seconds). Items without a readable date stay.
pub fn empty_trash_before(trash: &Path, cutoff: i64) -> io::Result<usize> {
    let entries = match std::fs::read_dir(trash.join("info")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0;
    for info in entries.flatten().map(|e| e.path()) {
        let Some(name) = info
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".trashinfo"))
        else {
            continue;
        };
        let Some(deleted) = std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| deletion_date(&t))
        else {
            continue;
        };
        if deleted >= cutoff {
            continue;
        }
        let file = trash.join("files").join(name);
        let gone = match std::fs::symlink_metadata(&file) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&file),
            Ok(_) => std::fs::remove_file(&file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        if gone.is_ok() && std::fs::remove_file(&info).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The `DeletionDate=YYYY-MM-DDThh:mm:ss` of a `.trashinfo`, as Unix
/// seconds. The spec stores local time without a zone; reading it as UTC
/// is off by at most a day's fraction, which a days-long setting absorbs.
pub fn deletion_date(trashinfo: &str) -> Option<i64> {
    let value = trashinfo
        .lines()
        .find_map(|l| l.trim().strip_prefix("DeletionDate="))?;
    let (date, time) = value.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next().flatten().unwrap_or(0));
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + s)
}

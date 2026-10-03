//! Filesystem operations with no UI state: listing, naming, copying, and the
//! freedesktop.org trash.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// What a directory entry is.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    /// A directory, or a symbolic link to one.
    Directory,
    /// A regular file, or a symbolic link to one.
    File,
    /// A broken link or a special file such as a socket.
    Other,
}

/// One entry of a directory listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// File name.
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// What the entry is, following symbolic links.
    pub kind: Kind,
    /// Whether the entry itself is a symbolic link.
    pub symlink: bool,
    /// Size in bytes; zero for directories.
    pub size: u64,
    /// Last modification time, when known.
    pub modified: Option<SystemTime>,
}

impl Entry {
    /// Whether the name starts with a dot.
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    fn read(path: PathBuf) -> io::Result<Self> {
        let link = fs::symlink_metadata(&path)?;
        let symlink = link.file_type().is_symlink();
        let target = if symlink {
            fs::metadata(&path).ok()
        } else {
            Some(link)
        };
        let kind = match &target {
            Some(m) if m.is_dir() => Kind::Directory,
            Some(m) if m.is_file() => Kind::File,
            _ => Kind::Other,
        };
        Ok(Self {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            size: match (&target, kind) {
                (Some(m), Kind::File) => m.len(),
                _ => 0,
            },
            modified: target.and_then(|m| m.modified().ok()),
            path,
            kind,
            symlink,
        })
    }
}

/// Lists a directory. Entries that vanish while listing are skipped.
pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        match Entry::read(item?.path()) {
            Ok(entry) => entries.push(entry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(entries)
}

/// Checks a single file name typed by the user.
pub fn validate_name(name: &str) -> Result<&str, &'static str> {
    if name.is_empty() {
        Err("The name is empty")
    } else if name == "." || name == ".." {
        Err("`.` and `..` are reserved")
    } else if name.contains('/') || name.contains('\0') {
        Err("Names cannot contain `/`")
    } else if name.len() > 255 {
        Err("The name is too long")
    } else {
        Ok(name)
    }
}

/// Returns `dir/name`, or `dir/name (copy)`, `dir/name (copy 2)`, ... with
/// the suffix before the extension, whichever does not exist yet.
pub fn unused_path(dir: &Path, name: &OsStr) -> PathBuf {
    let candidate = dir.join(name);
    if fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }
    let path = Path::new(name);
    let (stem, extension) = match (path.file_stem(), path.extension()) {
        // `.bashrc` has no extension; keep dotfiles whole.
        (Some(stem), Some(ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    (1u32..)
        .map(|n| {
            let mut file = stem.to_owned();
            file.push(if n == 1 {
                " (copy)".into()
            } else {
                format!(" (copy {n})")
            });
            if let Some(ext) = extension {
                file.push(".");
                file.push(ext);
            }
            dir.join(file)
        })
        .find(|path| fs::symlink_metadata(path).is_err())
        .expect("u32 suffixes exhausted")
}

/// Copies a file, symbolic link, or directory tree to `to`, which must not
/// exist. Symbolic links are copied as links, not followed.
pub fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if is_inside(to, from)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot copy a folder into itself",
        ));
    }
    copy_tree(from, to)
}

/// Whether `to` would land inside the folder `from`. Comparing the spelled
/// paths misses an alias such as `/link-to-a/sub` for `/a/sub`, which would
/// make the copy recurse into its own output, so a folder is also compared
/// by where both paths really are.
fn is_inside(to: &Path, from: &Path) -> io::Result<bool> {
    if to.starts_with(from) {
        return Ok(true);
    }
    if !fs::symlink_metadata(from)?.is_dir() {
        return Ok(false);
    }
    let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Ok(false);
    };
    Ok(fs::canonicalize(parent)?.starts_with(fs::canonicalize(from)?))
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)
    } else if meta.is_dir() {
        fs::create_dir(to)?;
        for item in fs::read_dir(from)? {
            let item = item?;
            copy_tree(&item.path(), &to.join(item.file_name()))?;
        }
        Ok(())
    } else {
        if fs::symlink_metadata(to).is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        fs::copy(from, to).map(drop)
    }
}

/// Moves `from` to `to`, copying and removing when they are on different
/// filesystems. `to` must not exist.
pub fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    if to.starts_with(from) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot move a folder into itself",
        ));
    }
    match fs::rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_recursive(from, to)?;
            remove(from)
        }
        result => result,
    }
}

fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// The user's trash, following the freedesktop.org Trash specification
/// (home trash only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trash {
    root: PathBuf,
}

impl Trash {
    /// `$XDG_DATA_HOME/Trash`, falling back to `~/.local/share/Trash`.
    pub fn home() -> Option<Self> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|dir| Path::new(dir).is_absolute())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share"))
            })?;
        Some(Self::at(data.join("Trash")))
    }

    /// A trash rooted at `root`, which holds `files` and `info`.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Where trashed files live; browse this to see the trash.
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    fn info_dir(&self) -> PathBuf {
        self.root.join("info")
    }

    /// Moves `path` to the trash and records where it came from. Returns its
    /// new location.
    pub fn put(&self, path: &Path) -> io::Result<PathBuf> {
        let original = std::path::absolute(path)?;
        let name = original
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cannot trash `/`"))?;
        if original.starts_with(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "already in the trash",
            ));
        }
        let (files, info) = (self.files_dir(), self.info_dir());
        fs::create_dir_all(&files)?;
        fs::create_dir_all(&info)?;
        // Reserve the info file first with create_new, as the spec requires,
        // so two trashers cannot pick the same name.
        let mut n = 0u32;
        let (target, info_path) = loop {
            let mut candidate = name.to_owned();
            if n > 0 {
                candidate.push(format!(".{n}"));
            }
            let mut info_name = candidate.clone();
            info_name.push(".trashinfo");
            let info_path = info.join(info_name);
            let target = files.join(&candidate);
            if fs::symlink_metadata(&target).is_err() {
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&info_path)
                {
                    Ok(_) => break (target, info_path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
            n = n
                .checked_add(1)
                .ok_or_else(|| io::Error::other("trash names exhausted"))?;
        };
        let record = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            percent_encode(&original),
            timestamp(SystemTime::now()),
        );
        let result = fs::write(&info_path, record).and_then(|()| move_path(&original, &target));
        if result.is_err() {
            let _ = fs::remove_file(&info_path);
        }
        result.map(|()| target)
    }
}

fn percent_encode(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Formats a time as `YYYY-MM-DDThh:mm:ss` in UTC.
pub fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Formats a byte count, for example `1.5 KiB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

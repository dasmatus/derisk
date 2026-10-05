use std::path::PathBuf;

use derisk::privacy::{Swept, deletion_date, empty_trash_before, sweep};
use derisk_settings::{Privacy, Settings};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("derisk-sweep-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn trash(dir: &std::path::Path, name: &str, date: &str) {
    std::fs::create_dir_all(dir.join("Trash/info")).unwrap();
    std::fs::create_dir_all(dir.join("Trash/files")).unwrap();
    std::fs::write(dir.join("Trash/files").join(name), b"x").unwrap();
    std::fs::write(
        dir.join("Trash/info").join(format!("{name}.trashinfo")),
        format!("[Trash Info]\nPath=/home/me/{name}\nDeletionDate={date}\n"),
    )
    .unwrap();
}

#[test]
fn deletion_dates_read_as_utc() {
    assert_eq!(
        deletion_date("[Trash Info]\nDeletionDate=1970-01-01T00:00:00\n"),
        Some(0)
    );
    assert_eq!(
        deletion_date("DeletionDate=2026-10-04T12:30:15"),
        Some(1_791_117_015)
    );
    assert_eq!(
        deletion_date("DeletionDate=2024-02-29T00:00:00"),
        Some(1_709_164_800)
    );
    assert_eq!(deletion_date("DeletionDate=2026-13-01T00:00:00"), None);
    assert_eq!(deletion_date("Path=/x"), None);
}

#[test]
fn old_trash_goes_and_new_trash_stays() {
    let dir = temp_dir("trash");
    trash(&dir, "old.txt", "2026-09-01T10:00:00");
    trash(&dir, "new.txt", "2026-10-03T10:00:00");
    std::fs::create_dir_all(dir.join("Trash/files/folder/inner")).unwrap();
    std::fs::write(
        dir.join("Trash/info/folder.trashinfo"),
        "DeletionDate=2026-08-01T00:00:00\n",
    )
    .unwrap();
    std::fs::write(dir.join("Trash/info/undated.trashinfo"), "Path=/x\n").unwrap();
    let now = deletion_date("DeletionDate=2026-10-04T00:00:00").unwrap();
    let cutoff = now - 30 * 86_400;
    assert_eq!(empty_trash_before(&dir.join("Trash"), cutoff).unwrap(), 2);
    assert!(!dir.join("Trash/files/old.txt").exists());
    assert!(!dir.join("Trash/files/folder").exists());
    assert!(dir.join("Trash/files/new.txt").exists());
    assert!(dir.join("Trash/info/undated.trashinfo").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sweep_follows_the_settings() {
    let dir = temp_dir("sweep");
    std::fs::write(dir.join("recently-used.xbel"), "<xbel/>").unwrap();
    trash(&dir, "old.txt", "2026-09-01T10:00:00");
    let now = deletion_date("DeletionDate=2026-10-04T00:00:00").unwrap();
    let mut privacy = Privacy {
        empty_trash_days: 0,
        ..Settings::default().privacy
    };
    assert_eq!(sweep(&privacy, &dir, now).unwrap(), Swept::default());
    assert!(dir.join("recently-used.xbel").exists());
    privacy.remember_recent = false;
    privacy.empty_trash_days = 30;
    assert_eq!(
        sweep(&privacy, &dir, now).unwrap(),
        Swept {
            recent: true,
            trashed: 1
        }
    );
    assert!(!dir.join("recently-used.xbel").exists());
    // Nothing left to do, and a missing trash is no error.
    let _ = std::fs::remove_dir_all(dir.join("Trash"));
    assert_eq!(sweep(&privacy, &dir, now).unwrap(), Swept::default());
    let _ = std::fs::remove_dir_all(&dir);
}

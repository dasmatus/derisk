use std::{
    cell::RefCell,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, UNIX_EPOCH},
};

use derisk_files::{
    Browser, ClipboardOp, FilesApp, Kind, SortKey, Trash,
    fs_ops::{copy_recursive, human_size, move_path, timestamp, unused_path, validate_name},
};
use mcsapi_ui::{Theme, egui, run_frame};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("derisk-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str, bytes: usize) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, vec![b'x'; bytes]).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn browser(dir: &TempDir) -> Browser {
    Browser::with_trash(&dir.0, Some(Trash::at(dir.0.join(".Trash")))).unwrap()
}

fn names(browser: &Browser) -> Vec<&str> {
    browser.visible().map(|e| e.name.as_str()).collect()
}

#[test]
fn names_are_validated() {
    assert!(validate_name("report.pdf").is_ok());
    for bad in ["", ".", "..", "a/b", "nul\0"] {
        assert!(validate_name(bad).is_err(), "{bad:?}");
    }
    assert!(validate_name(&"a".repeat(256)).is_err());
}

#[test]
fn unused_paths_add_copy_before_the_extension() {
    let dir = TempDir::new("unused");
    assert_eq!(
        unused_path(&dir.0, OsStr::new("a.txt")),
        dir.0.join("a.txt")
    );
    dir.file("a.txt", 1);
    assert_eq!(
        unused_path(&dir.0, OsStr::new("a.txt")),
        dir.0.join("a (copy).txt")
    );
    dir.file("a (copy).txt", 1);
    assert_eq!(
        unused_path(&dir.0, OsStr::new("a.txt")),
        dir.0.join("a (copy 2).txt")
    );
    dir.file(".bashrc", 1);
    assert_eq!(
        unused_path(&dir.0, OsStr::new(".bashrc")),
        dir.0.join(".bashrc (copy)")
    );
}

#[test]
fn copies_trees_and_keeps_links() {
    let dir = TempDir::new("copy");
    dir.file("src/a.txt", 3);
    dir.file("src/sub/b.txt", 5);
    std::os::unix::fs::symlink("a.txt", dir.0.join("src/link")).unwrap();
    copy_recursive(&dir.0.join("src"), &dir.0.join("dst")).unwrap();
    assert_eq!(fs::read(dir.0.join("dst/sub/b.txt")).unwrap().len(), 5);
    assert_eq!(
        fs::read_link(dir.0.join("dst/link")).unwrap(),
        Path::new("a.txt")
    );
    assert!(copy_recursive(&dir.0.join("src"), &dir.0.join("src/sub/inner")).is_err());
    std::os::unix::fs::symlink(dir.0.join("src"), dir.0.join("alias")).unwrap();
    assert!(
        copy_recursive(&dir.0.join("src"), &dir.0.join("alias/sub/inner")).is_err(),
        "an alias of the source is still inside it"
    );
    assert!(!dir.0.join("src/sub/inner").exists());
    fs::remove_file(dir.0.join("alias")).unwrap();
    assert!(move_path(&dir.0.join("src"), &dir.0.join("src/sub/inner")).is_err());
    assert!(move_path(&dir.0.join("src/a.txt"), &dir.0.join("dst/sub/b.txt")).is_err());
    move_path(&dir.0.join("src"), &dir.0.join("moved")).unwrap();
    assert!(!dir.0.join("src").exists());
    assert!(dir.0.join("moved/sub/b.txt").exists());
}

#[test]
fn trash_records_the_original_path() {
    let dir = TempDir::new("trash");
    let trash = Trash::at(dir.0.join("Trash"));
    let first = dir.file("my file%.txt", 1);
    let stored = trash.put(&first).unwrap();
    assert_eq!(stored, dir.0.join("Trash/files/my file%.txt"));
    let info = fs::read_to_string(dir.0.join("Trash/info/my file%.txt.trashinfo")).unwrap();
    assert!(info.starts_with("[Trash Info]\nPath="));
    assert!(info.contains("my%20file%25.txt\n"), "{info}");
    assert!(info.contains("DeletionDate="));
    // A second file with the same name gets a numbered slot.
    let second = dir.file("my file%.txt", 2);
    assert_eq!(
        trash.put(&second).unwrap(),
        dir.0.join("Trash/files/my file%.txt.1")
    );
    assert!(trash.put(&stored).is_err(), "already in the trash");
    assert!(trash.put(Path::new("/")).is_err());
}

#[test]
fn formats_sizes_and_times() {
    assert_eq!(human_size(0), "0 B");
    assert_eq!(human_size(1023), "1023 B");
    assert_eq!(human_size(1536), "1.5 KiB");
    assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
    assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00");
    let leap = UNIX_EPOCH + Duration::from_secs(951_782_400 + 3661);
    assert_eq!(timestamp(leap), "2000-02-29T01:01:01");
}

#[test]
fn lists_folders_first_and_hides_dot_files() {
    let dir = TempDir::new("list");
    dir.file("b.txt", 10);
    dir.file("A.txt", 30);
    dir.file(".hidden", 1);
    fs::create_dir(dir.0.join("zeta")).unwrap();
    let mut files = browser(&dir);
    assert_eq!(names(&files), ["zeta", "A.txt", "b.txt"]);
    files.show_hidden = true;
    assert_eq!(names(&files), ["zeta", ".hidden", "A.txt", "b.txt"]);
    files.show_hidden = false;
    files.sort_by(SortKey::Size);
    assert_eq!(names(&files), ["zeta", "b.txt", "A.txt"]);
    files.sort_by(SortKey::Size);
    assert_eq!(files.sort(), (SortKey::Size, true));
    assert_eq!(names(&files), ["zeta", "A.txt", "b.txt"]);
    files.filter = "B.T".into();
    assert_eq!(names(&files), ["b.txt"]);
    let entry = files.visible().next().unwrap();
    assert_eq!((entry.kind, entry.size), (Kind::File, 10));
}

#[test]
fn history_moves_back_forward_and_up() {
    let dir = TempDir::new("history");
    fs::create_dir_all(dir.0.join("a/b")).unwrap();
    let mut files = browser(&dir);
    files.navigate(dir.0.join("a")).unwrap();
    files.navigate(dir.0.join("a/b")).unwrap();
    files.go_back().unwrap();
    assert_eq!(files.cwd(), dir.0.join("a"));
    assert!(files.can_go_forward());
    files.go_forward().unwrap();
    assert_eq!(files.cwd(), dir.0.join("a/b"));
    files.go_up().unwrap();
    assert_eq!(files.cwd(), dir.0.join("a"));
    assert!(!files.can_go_forward(), "navigating clears forward history");
    assert!(files.navigate(dir.0.join("missing")).is_err());
    assert_eq!(
        files.cwd(),
        dir.0.join("a"),
        "failed navigation changes nothing"
    );
    files.go_back().unwrap();
    files.go_back().unwrap();
    files.go_back().unwrap();
    assert_eq!(files.cwd(), dir.0);
    assert!(!files.can_go_back());
}

#[test]
fn creates_renames_and_trashes() {
    let dir = TempDir::new("edit");
    let mut files = browser(&dir);
    let folder = files.create_folder("Projects").unwrap();
    assert!(folder.is_dir());
    assert!(files.create_folder("Projects").is_err());
    assert!(files.create_file("../escape").is_err());
    let note = files.create_file("note.txt").unwrap();
    dir.file("taken.txt", 1);
    assert!(
        files.rename(&note, "taken.txt").is_err(),
        "never overwrites"
    );
    let renamed = files.rename(&note, "renamed.txt").unwrap();
    assert!(files.is_selected(&renamed));
    files.select(&folder, true);
    assert_eq!(files.selection().len(), 2);
    assert_eq!(files.trash_selection().unwrap(), 2);
    assert_eq!(names(&files), ["taken.txt"]);
    assert!(dir.0.join(".Trash/files/Projects").is_dir());
    assert!(files.selection().is_empty());
}

#[test]
fn copy_and_cut_paste() {
    let dir = TempDir::new("paste");
    fs::create_dir(dir.0.join("dest")).unwrap();
    let a = dir.file("a.txt", 4);
    let mut files = browser(&dir);
    files.select(&a, false);
    files.set_clipboard(ClipboardOp::Copy);
    assert_eq!(files.paste().unwrap(), [dir.0.join("a (copy).txt")]);
    assert_eq!(
        files.clipboard().unwrap().0,
        ClipboardOp::Copy,
        "copies can repeat"
    );

    files.select(&a, false);
    files.set_clipboard(ClipboardOp::Cut);
    assert_eq!(
        files.paste().unwrap(),
        std::slice::from_ref(&a),
        "cut into the same folder stays"
    );
    files.navigate(dir.0.join("dest")).unwrap();
    assert_eq!(files.paste().unwrap(), [dir.0.join("dest/a.txt")]);
    assert!(!a.exists());
    assert!(files.clipboard().is_none(), "cut clipboard empties");
    files.select_all();
    assert_eq!(files.selection().len(), 1);
    files.clear_selection();

    // A cut that fails part way keeps only what is still to move.
    let b = dir.file("dest/b.txt", 1);
    let c = dir.file("dest/c.txt", 1);
    files.refresh().unwrap();
    files.select(&b, false);
    files.select(&c, true);
    files.set_clipboard(ClipboardOp::Cut);
    fs::create_dir(dir.0.join("back")).unwrap();
    files.navigate(dir.0.join("back")).unwrap();
    let held = files.clipboard().unwrap().1.to_vec();
    fs::remove_file(&held[1]).unwrap();
    assert!(files.paste().is_err());
    assert_eq!(files.clipboard().unwrap().1, &held[1..]);
    files.clear_selection();
    assert!(files.selection().is_empty());
}

#[test]
fn app_opens_files_and_folders() {
    let dir = TempDir::new("app");
    let doc = dir.file("doc.txt", 1);
    fs::create_dir(dir.0.join("sub")).unwrap();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let log = opened.clone();
    let mut app = FilesApp::new(
        Ok(browser(&dir)),
        Box::new(move |path| {
            log.borrow_mut().push(path.to_owned());
            Ok(())
        }),
    );
    app.activate(&doc);
    assert_eq!(*opened.borrow(), [doc]);
    app.activate(&dir.0.join("sub"));
    assert_eq!(app.browser.as_ref().unwrap().cwd(), dir.0.join("sub"));

    let context = egui::Context::default();
    for _ in 0..2 {
        let mut output = run_frame(
            &mut app,
            &context,
            egui::RawInput::default(),
            &Theme::default(),
        );
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }

    let mut missing = FilesApp::new(Browser::new(dir.0.join("nope")), Box::new(|_| Ok(())));
    assert!(
        missing
            .status()
            .unwrap()
            .starts_with("Could not open folder")
    );
    run_frame(
        &mut missing,
        &context,
        egui::RawInput::default(),
        &Theme::default(),
    )
    .textures_delta
    .clear();
}

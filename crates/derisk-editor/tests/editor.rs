use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use derisk_editor::{Document, EditorApp, MAX_FILE_SIZE};
use mcsapi_ui::{Theme, egui, run_frame};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("derisk-editor-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn refuses_binary_and_huge_files() {
    let dir = temp_dir("refuse");
    fs::write(dir.join("binary"), [0xff, 0xfe, 0x00]).unwrap();
    assert!(Document::open(&dir.join("binary")).is_err());
    let big = fs::File::create(dir.join("big")).unwrap();
    big.set_len(MAX_FILE_SIZE + 1).unwrap();
    assert!(Document::open(&dir.join("big")).is_err());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saving_keeps_permissions_and_clears_dirty() {
    let dir = temp_dir("save");
    let path = dir.join("script.sh");
    fs::write(&path, "echo hi\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let mut doc = Document::open(&path).unwrap();
    assert_eq!(doc.name(), "script.sh");
    assert!(!doc.is_dirty());
    doc.text.push_str("echo bye\n");
    assert!(doc.is_dirty());
    doc.save().unwrap();
    assert!(!doc.is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "echo hi\necho bye\n");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(Document::default().save().is_err(), "untitled needs a path");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saving_never_writes_through_a_planted_name() {
    // Another user who can write to the directory used to be able to plant
    // `<file>.derisk-save` as a symlink, and the save wrote the document
    // through it into whatever it pointed at.
    let dir = temp_dir("planted");
    let victim = dir.join("bashrc");
    fs::write(&victim, "untouched\n").unwrap();
    let path = dir.join("notes.txt");
    fs::write(&path, "shared\n").unwrap();
    std::os::unix::fs::symlink(&victim, dir.join("notes.txt.derisk-save")).unwrap();
    let mut doc = Document::open(&path).unwrap();
    doc.text.push_str("more\n");
    doc.save().unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), "shared\nmore\n");
    assert!(
        !fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Nothing left behind but the planted link.
    let left: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 3, "{left:?}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn finds_replaces_and_reports_positions() {
    let mut doc = Document::default();
    doc.text = "Alpha beta\nALPHA gamma\nalpha".into();
    assert_eq!(doc.find("alpha"), [0, 11, 23]);
    assert!(doc.find("").is_empty());
    assert_eq!(doc.replace_all("alpha", "omega"), 1);
    assert_eq!(doc.replace_all("", "x"), 0);
    assert!(doc.text.ends_with("omega"));
    assert_eq!(doc.line_column(0), (1, 1));
    assert_eq!(doc.line_column(13), (2, 3));
    // Lowercasing İ changes its byte length; fall back to exact case.
    let mut turkish = Document::default();
    turkish.text = "İstanbul istanbul".into();
    assert_eq!(turkish.find("istanbul"), [10]);
}

#[test]
fn app_saves_to_the_location_field() {
    let dir = temp_dir("app");
    let path = dir.join("new.txt");
    let mut app = EditorApp::open(&path);
    assert!(app.status().unwrap().starts_with("New file"));
    app.document.text = "draft".into();
    app.save();
    assert_eq!(fs::read_to_string(&path).unwrap(), "draft");
    assert_eq!(app.document.path(), Some(path.as_path()));
    let context = egui::Context::default();
    let mut output = run_frame(
        &mut app,
        &context,
        egui::RawInput::default(),
        &Theme::default(),
    );
    assert!(!output.shapes.is_empty());
    output.textures_delta.clear();
    let mut untitled = EditorApp::new();
    untitled.save();
    assert!(untitled.status().unwrap().starts_with("Could not save"));
    fs::remove_dir_all(dir).unwrap();
}

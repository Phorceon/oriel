//! Tests for the persisted state in `store.rs`. Tests only write under the system temp
//! directory, in a sub-folder unique to the process and each test.

use super::*;
use crate::types::{AppInfo, Rect, Saved};
use std::path::PathBuf;

/// A fresh, empty directory under `temp_dir()` that is removed at the end of the test.
fn temp_subdir(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "oriel-store-test-{}-{}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn sample_app() -> AppInfo {
    AppInfo {
        bundle_id: "com.example.Foo".to_string(),
        name: "Foo".to_string(),
        path: PathBuf::from("/Applications/Foo.app"),
    }
}

fn sample_saved() -> Saved {
    Saved {
        apps: vec![sample_app()],
        frame: Some(Rect::new(10.0, 20.0, 800.0, 600.0)),
        active_app: Some("com.example.Foo".to_string()),
    }
}

#[test]
fn save_then_load_round_trip() {
    let dir = temp_subdir("round_trip");
    let path = dir.join("state.json");

    let saved = sample_saved();
    save(&path, &saved).unwrap();
    let loaded = load(&path);

    assert_eq!(loaded, saved);
    assert_eq!(loaded.apps, vec![sample_app()]);
    assert_eq!(loaded.frame, Some(Rect::new(10.0, 20.0, 800.0, 600.0)));
    assert_eq!(loaded.active_app.as_deref(), Some("com.example.Foo"));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_missing_file_is_default() {
    let dir = temp_subdir("missing");
    let path = dir.join("does-not-exist.json");

    assert_eq!(load(&path), Saved::default());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_empty_file_is_default() {
    let dir = temp_subdir("empty");
    let path = dir.join("state.json");
    fs::write(&path, "").unwrap();

    assert_eq!(load(&path), Saved::default());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_garbage_is_default() {
    let dir = temp_subdir("garbage");
    let path = dir.join("state.json");
    fs::write(&path, "this is not json {").unwrap();

    assert_eq!(load(&path), Saved::default());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_json_with_wrong_types_is_default() {
    let dir = temp_subdir("wrong_types");
    let path = dir.join("state.json");
    // `apps` must be an array, not an object.
    fs::write(&path, r#"{"apps": {"bundle_id": "x"}}"#).unwrap();

    assert_eq!(load(&path), Saved::default());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_valid_json_with_missing_fields_uses_serde_defaults() {
    let dir = temp_subdir("missing_fields");
    let path = dir.join("state.json");
    // Only `apps` is provided; `frame` and `active_app` fall back to their serde defaults.
    fs::write(&path, r#"{"apps": [{"bundle_id":"com.example.Foo","name":"Foo","path":"/Applications/Foo.app"}]}"#)
        .unwrap();

    let loaded = load(&path);
    assert_eq!(loaded.apps, vec![sample_app()]);
    assert_eq!(loaded.frame, None);
    assert_eq!(loaded.active_app, None);

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_creates_missing_parent_folders() {
    let dir = temp_subdir("nested");
    let path = dir.join("a").join("b").join("state.json");

    save(&path, &sample_saved()).unwrap();

    assert!(path.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_replaces_existing_file() {
    let dir = temp_subdir("replace");
    let path = dir.join("state.json");
    fs::write(&path, r#"{"apps":[]}"#).unwrap();

    save(&path, &sample_saved()).unwrap();

    assert_eq!(load(&path), sample_saved());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_leaves_no_tmp_file() {
    let dir = temp_subdir("no_tmp");
    let path = dir.join("state.json");

    save(&path, &sample_saved()).unwrap();

    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "leftover temp file(s) after save");

    fs::remove_dir_all(&dir).unwrap();
}

/// Every `default_path` environment case in one test, because `set_var`/`remove_var` are
/// unsafe in edition 2024. The previous env values are restored before the test ends.
#[test]
fn default_path_env_cases() {
    unsafe {
        let old_oriel = std::env::var("ORIEL_STATE").ok();
        let old_home = std::env::var("HOME").ok();

        // SAFETY: this test runs single-threaded and restores the original values of both
        // variables before returning, so no other test observes a changed environment.
        std::env::set_var("ORIEL_STATE", "/tmp/custom/state.json");
        assert_eq!(default_path(), PathBuf::from("/tmp/custom/state.json"));

        // Empty ORIEL_STATE is treated as unset: fall back to HOME.
        std::env::set_var("ORIEL_STATE", "");
        std::env::set_var("HOME", "/Users/tester");
        assert_eq!(
            default_path(),
            PathBuf::from("/Users/tester/Library/Application Support/Oriel/state.json")
        );

        // Neither variable set: relative fallback.
        std::env::remove_var("ORIEL_STATE");
        std::env::remove_var("HOME");
        assert_eq!(default_path(), PathBuf::from("./oriel-state.json"));

        match old_oriel {
            Some(v) => std::env::set_var("ORIEL_STATE", v),
            None => std::env::remove_var("ORIEL_STATE"),
        }
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

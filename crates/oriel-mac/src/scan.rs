//! ScanApps: find installed `.app` bundles.

use oriel_core::AppInfo;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The folders a scan looks into, most preferred first.
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

/// Read the id and name of one bundle. `None` when it has no bundle id.
pub fn read_app_info(app: &Path) -> Option<AppInfo> {
    let value = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    let dict = value.as_dictionary()?;
    let text = |key: &str| {
        dict.get(key)
            .and_then(|v| v.as_string())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let bundle_id = text("CFBundleIdentifier")?;
    let name = text("CFBundleDisplayName")
        .or_else(|| text("CFBundleName"))
        .or_else(|| app.file_stem().map(|s| s.to_string_lossy().into_owned()))?;
    Some(AppInfo { bundle_id, name, path: app.to_path_buf() })
}

fn is_app(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "app")
}

/// Scan `roots` and one level of sub-folders in each. Sorted by name (case-insensitive), no
/// duplicate bundle ids; the first root wins.
pub fn scan(roots: &[PathBuf]) -> Vec<AppInfo> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    let mut add = |path: &Path| {
        if let Some(info) = read_app_info(path)
            && seen.insert(info.bundle_id.clone())
        {
            apps.push(info);
        }
    };
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in &paths {
            if is_app(path) {
                add(path);
            }
        }
        for dir in paths.iter().filter(|p| p.is_dir() && !is_app(p)) {
            let Ok(sub) = std::fs::read_dir(dir) else { continue };
            let mut subs: Vec<PathBuf> = sub.filter_map(|e| e.ok().map(|e| e.path())).collect();
            subs.sort();
            for path in subs.iter().filter(|p| is_app(p)) {
                add(path);
            }
        }
    }
    apps.sort_by_key(|a| (a.name.to_lowercase(), a.bundle_id.clone()));
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app(root: &Path, dir: &str, file: &str, plist_body: &str) {
        let contents = root.join(dir).join(file).join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>{plist_body}</dict></plist>"#
        );
        std::fs::write(contents.join("Info.plist"), xml).unwrap();
    }

    #[test]
    fn scans_names_and_dedupes() {
        let tmp = std::env::temp_dir().join(format!("oriel-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let a = tmp.join("a");
        let b = tmp.join("b");
        make_app(&a, "", "Zed.app", "<key>CFBundleIdentifier</key><string>dev.zed</string><key>CFBundleName</key><string>Zed</string>");
        make_app(&a, "", "alpha.app", "<key>CFBundleIdentifier</key><string>x.alpha</string><key>CFBundleDisplayName</key><string>Alpha Pro</string><key>CFBundleName</key><string>alpha</string>");
        make_app(&a, "Utilities", "Term.app", "<key>CFBundleIdentifier</key><string>x.term</string>");
        make_app(&a, "", "NoId.app", "<key>CFBundleName</key><string>NoId</string>");
        // Duplicate id in the second root: the first root wins.
        make_app(&b, "", "Zed2.app", "<key>CFBundleIdentifier</key><string>dev.zed</string><key>CFBundleName</key><string>Zed Copy</string>");
        let apps = scan(&[a.clone(), b]);
        let names: Vec<_> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha Pro", "Term", "Zed"]);
        assert_eq!(apps[2].path, a.join("Zed.app"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

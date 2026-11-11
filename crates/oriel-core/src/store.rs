//! Saved state on disk: one JSON file. Owner of the bodies: the oriel-store agent.
//! The signatures are the frozen contract.

use crate::types::Saved;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The default file: `~/Library/Application Support/Oriel/state.json` on macOS. The
/// `ORIEL_STATE` environment variable, when set and not empty, replaces the whole path.
pub fn default_path() -> PathBuf {
    if let Ok(p) = std::env::var("ORIEL_STATE")
        && !p.trim().is_empty()
    {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Oriel")
            .join("state.json");
    }
    PathBuf::from("./oriel-state.json")
}

/// Read the saved state. A missing, empty or damaged file gives `Saved::default()`: a bad
/// file must never stop the app.
pub fn load(path: &Path) -> Saved {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Saved::default(),
    };
    serde_json::from_str::<Saved>(&text).unwrap_or_default()
}

/// Write the saved state atomically (write a temporary file beside it, then rename), creating
/// the parent folders.
pub fn save(path: &Path, saved: &Saved) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp = PathBuf::from(tmp_os);

    let file = fs::File::create(&tmp)?;
    serde_json::to_writer_pretty(file, saved).map_err(io::Error::other)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

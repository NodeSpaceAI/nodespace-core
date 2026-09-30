//! Small shared helper: atomic write-to-temp-then-rename for a JSON file.
//! `preferences.rs`, `window_state.rs` and `commands/onboarding.rs` each
//! persist a different JSON shape; this is the one place their shared write
//! mechanics live instead of being copied between them.

use std::path::Path;

use serde::Serialize;
use tokio::fs;

/// Serialize `value` as pretty JSON and write it to `dir/<filename>`,
/// atomically: `dir` is created if missing, the write lands at
/// `dir/<filename>.tmp` first, then an OS-level rename replaces the real
/// file — so a crash or power loss mid-write can never leave a corrupt or
/// partially-written file behind.
///
/// Not safe against two *concurrent* callers writing the same
/// `dir`/`filename` — both would race on the same temp path. Callers whose
/// writes can genuinely overlap (e.g. rapid `WindowEvent::Resized` deliveries
/// spawning one save task each) must serialize their own calls; see
/// `window_state.rs`'s `SAVE_LOCK` for that pattern.
pub(crate) async fn write_json<T: Serialize>(
    dir: &Path,
    filename: &str,
    value: &T,
) -> Result<(), String> {
    fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("Failed to create {} directory: {e}", dir.display()))?;

    let serialized = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize {filename}: {e}"))?;

    let path = dir.join(filename);
    let temp_path = dir.join(format!("{filename}.tmp"));
    fs::write(&temp_path, serialized)
        .await
        .map_err(|e| format!("Failed to write {filename}: {e}"))?;
    fs::rename(&temp_path, &path)
        .await
        .map_err(|e| format!("Failed to save {filename}: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_readable_json_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_json(dir.path(), "thing.json", &vec![1, 2, 3])
            .await
            .expect("write failed");

        let contents = tokio::fs::read_to_string(dir.path().join("thing.json"))
            .await
            .expect("read failed");
        let parsed: Vec<i32> = serde_json::from_str(&contents).expect("parse failed");
        assert_eq!(parsed, vec![1, 2, 3]);
        assert!(!dir.path().join("thing.json.tmp").exists());
    }

    #[tokio::test]
    async fn creates_the_directory_if_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("nested").join("deeper");
        write_json(&nested, "thing.json", &42)
            .await
            .expect("write failed");
        assert!(nested.join("thing.json").exists());
    }
}

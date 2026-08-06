use crate::paths::AppPaths;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: usize = 256 * 1024;

pub fn record(paths: &AppPaths, message: &str) {
    let path = paths.diagnostics_log();
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let entry = format!("{timestamp} {}\n", message.replace(['\n', '\r'], " "));
    let mut contents = fs::read(&path).unwrap_or_default();
    if contents.len() + entry.len() > MAX_LOG_BYTES {
        let retained = MAX_LOG_BYTES.saturating_sub(entry.len());
        contents = contents
            .split_off(contents.len().saturating_sub(retained))
            .into_iter()
            .collect();
    }
    contents.extend_from_slice(entry.as_bytes());
    let _ = fs::write(path, contents);
}

#[cfg(test)]
mod tests {
    use super::record;
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    #[test]
    fn diagnostics_log_is_capped() {
        let root = tempdir().expect("temporary root");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = AppPaths {
            root: root.path().to_path_buf(),
            config: root.path().join("config.toml"),
            state: root.path().join("state"),
            logs: root.path().join("logs"),
            backups: root.path().join("backups"),
            codex_home: codex_home.path().to_path_buf(),
        };
        for _ in 0..2_000 {
            record(&paths, &"x".repeat(200));
        }
        assert!(
            std::fs::metadata(paths.diagnostics_log())
                .expect("log metadata")
                .len()
                <= 256 * 1024
        );
    }
}

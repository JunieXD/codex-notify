use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

const SESSION_INDEX_BLOCK_SIZE: usize = 8_192;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnState {
    pub prompt: String,
    pub cwd: String,
    pub thread_id: String,
    pub conversation_title_at_start: Option<String>,
    pub started_at_unix_seconds: u64,
}

impl TurnState {
    pub fn new(
        prompt: impl Into<String>,
        cwd: impl Into<String>,
        thread_id: impl Into<String>,
        conversation_title_at_start: Option<String>,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            cwd: cwd.into(),
            thread_id: thread_id.into(),
            conversation_title_at_start,
            started_at_unix_seconds: now_unix_seconds(),
        }
    }
}

pub fn turn_state_path(state_directory: &Path, turn_id: &str) -> Option<PathBuf> {
    if turn_id.trim().is_empty() {
        return None;
    }

    let mut digest = Sha256::new();
    digest.update(turn_id.as_bytes());
    let filename = format!("{:x}.json", digest.finalize());
    Some(state_directory.join(filename))
}

pub fn write_turn_state(state_directory: &Path, turn_id: &str, state: &TurnState) -> Result<()> {
    let destination =
        turn_state_path(state_directory, turn_id).context("保存任务上下文时缺少 Codex turn ID")?;
    fs::create_dir_all(state_directory)
        .with_context(|| format!("无法创建目录 {}", state_directory.display()))?;

    let contents = serde_json::to_vec(state).context("无法生成任务状态数据")?;
    let mut temporary = NamedTempFile::new_in(state_directory)
        .with_context(|| format!("无法在 {} 中创建状态文件", state_directory.display()))?;
    temporary.write_all(&contents).context("无法写入任务状态")?;
    temporary.flush().context("无法保存任务状态")?;

    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(&destination)
            .with_context(|| format!("无法替换 {}", destination.display()))?;
    }
    temporary
        .persist(&destination)
        .map_err(|error| error.error)
        .with_context(|| format!("无法保存 {}", destination.display()))?;
    Ok(())
}

pub fn load_turn_state(state_directory: &Path, turn_id: &str) -> Result<Option<TurnState>> {
    let Some(path) = turn_state_path(state_directory, turn_id) else {
        return Ok(None);
    };
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取 {}", path.display()));
        }
    };

    let state = serde_json::from_slice(&contents)
        .with_context(|| format!("无法解析状态文件 {}", path.display()))?;
    Ok(Some(state))
}

pub fn remove_turn_state(state_directory: &Path, turn_id: &str) -> Result<()> {
    let Some(path) = turn_state_path(state_directory, turn_id) else {
        return Ok(());
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("无法删除 {}", path.display())),
    }
}

pub fn elapsed_since(state: &TurnState, now: SystemTime) -> Option<Duration> {
    let started = UNIX_EPOCH.checked_add(Duration::from_secs(state.started_at_unix_seconds))?;
    now.duration_since(started).ok()
}

pub fn prune_turn_states(state_directory: &Path, maximum_age: Duration) -> Result<usize> {
    let mut removed = 0;
    let now = SystemTime::now();
    let entries = match fs::read_dir(state_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取 {}", state_directory.display()));
        }
    };

    for entry in entries {
        let entry = entry.context("无法读取任务状态项目")?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata().context("无法检查任务状态项目")?;
        let modified = metadata.modified().context("无法读取状态文件修改时间")?;
        if now.duration_since(modified).unwrap_or_default() > maximum_age {
            fs::remove_file(&path)
                .with_context(|| format!("无法删除过期状态 {}", path.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn find_thread_title(index_path: &Path, thread_id: &str) -> Option<String> {
    if thread_id.trim().is_empty() {
        return None;
    }

    let mut file = File::open(index_path).ok()?;
    let mut position = file.seek(SeekFrom::End(0)).ok()?;
    let mut remainder = Vec::new();

    while position > 0 {
        let block_size = usize::try_from(position)
            .ok()
            .map_or(SESSION_INDEX_BLOCK_SIZE, |size| {
                size.min(SESSION_INDEX_BLOCK_SIZE)
            });
        position -= u64::try_from(block_size).ok()?;
        file.seek(SeekFrom::Start(position)).ok()?;

        let mut block = vec![0; block_size];
        file.read_exact(&mut block).ok()?;
        block.extend_from_slice(&remainder);
        let mut lines = block.split(|byte| *byte == b'\n').collect::<Vec<_>>();

        if position > 0 {
            remainder = lines.remove(0).to_vec();
        } else {
            remainder.clear();
        }

        for line in lines.into_iter().rev() {
            let record: SessionIndexRecord = match serde_json::from_slice(line) {
                Ok(record) => record,
                Err(_) => continue,
            };
            if record.id == thread_id {
                let title = record.thread_name.trim();
                if !title.is_empty() {
                    return Some(title.to_owned());
                }
            }
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct SessionIndexRecord {
    id: String,
    thread_name: String,
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        TurnState, elapsed_since, find_thread_title, load_turn_state, prune_turn_states,
        remove_turn_state, turn_state_path, write_turn_state,
    };
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;

    #[test]
    fn state_paths_do_not_expose_turn_ids() {
        let state_directory = tempdir().expect("temporary directory");
        let path = turn_state_path(state_directory.path(), "turn-secret").expect("state path");
        assert!(!path.to_string_lossy().contains("turn-secret"));
    }

    #[test]
    fn stores_loads_and_removes_turn_state() {
        let state_directory = tempdir().expect("temporary directory");
        let state = TurnState::new("task", "/workspace", "thread-1", Some("Title".to_owned()));
        write_turn_state(state_directory.path(), "turn-1", &state).expect("write state");

        assert_eq!(
            load_turn_state(state_directory.path(), "turn-1").expect("load state"),
            Some(state)
        );
        remove_turn_state(state_directory.path(), "turn-1").expect("remove state");
        assert_eq!(
            load_turn_state(state_directory.path(), "turn-1").expect("load missing state"),
            None
        );
    }

    #[test]
    fn title_lookup_reads_the_newest_matching_index_record() {
        let directory = tempdir().expect("temporary directory");
        let index_path = directory.path().join("session_index.jsonl");
        fs::write(
            &index_path,
            "{\"id\":\"thread-1\",\"thread_name\":\"Old title\"}\n\
             {\"id\":\"thread-2\",\"thread_name\":\"Other\"}\n\
             {\"id\":\"thread-1\",\"thread_name\":\"Newest title\"}\n",
        )
        .expect("write index");

        assert_eq!(
            find_thread_title(&index_path, "thread-1"),
            Some("Newest title".to_owned())
        );
    }

    #[test]
    fn state_elapsed_time_uses_the_recorded_start() {
        let state = TurnState {
            prompt: "task".to_owned(),
            cwd: String::new(),
            thread_id: String::new(),
            conversation_title_at_start: None,
            started_at_unix_seconds: 100,
        };
        let now = UNIX_EPOCH + Duration::from_secs(125);
        assert_eq!(elapsed_since(&state, now), Some(Duration::from_secs(25)));
    }

    #[test]
    fn pruning_removes_old_state_files() {
        let state_directory = tempdir().expect("temporary directory");
        let state = TurnState::new("task", "", "", None);
        write_turn_state(state_directory.path(), "turn-1", &state).expect("write state");
        let removed = prune_turn_states(state_directory.path(), Duration::from_secs(u64::MAX))
            .expect("prune state");
        assert_eq!(removed, 0);
        assert!(SystemTime::now() >= UNIX_EPOCH);
    }
}

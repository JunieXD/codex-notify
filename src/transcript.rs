//! Best-effort parsing for Codex session transcripts.
//!
//! Transcript JSONL is intentionally isolated in this module because Codex does
//! not promise it as a stable public integration surface. Callers must treat
//! malformed records and missing fields as normal, non-fatal conditions.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
const CONTEXT_ONLY_PREFIXES: &[&str] = &["<environment_context>", "# agents.md instructions"];
const KNOWN_USAGE_LIMIT_MESSAGES: &[&str] = &[
    "\u{4f60}\u{5df2}\u{8fbe}\u{5230}\u{4f7f}\u{7528}\u{4e0a}\u{9650}\u{3002}\u{8bf7}\u{7a0d}\u{540e}\u{518d}\u{8bd5}\u{3002}",
    "you have reached your usage limit. please try again later.",
    "you've reached your usage limit. please try again later.",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEvent {
    pub end_offset: u64,
    pub kind: TranscriptEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEventKind {
    SessionMeta(SessionMeta),
    UserPrompt(UserPrompt),
    TaskStarted,
    TaskCompleted(TaskCompletion),
    GoalStatus(String),
    TerminalError(TerminalError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub session_id: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPrompt {
    pub turn_id: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCompletion {
    pub turn_id: String,
    pub last_agent_message: String,
    pub completed_at_seconds: Option<u64>,
    pub duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalError {
    pub turn_id: String,
    pub message: String,
    pub completed_at_seconds: Option<u64>,
    pub duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    pub next_offset: u64,
    pub events: Vec<TranscriptEvent>,
}

/// Boundary for the unstable local transcript format. A future Codex format
/// can add another implementation without changing monitor policy or delivery.
pub trait TranscriptSource {
    fn read_events(&self, path: &Path, start_offset: u64) -> Result<ReadResult>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct JsonlTranscriptSource;

impl TranscriptSource for JsonlTranscriptSource {
    fn read_events(&self, path: &Path, start_offset: u64) -> Result<ReadResult> {
        read_events_from(path, start_offset)
    }
}

/// Read complete JSONL records from `start_offset` without consuming a partial
/// final line. This makes a later watcher pass safely retry a record while
/// Codex is still appending it.
pub fn read_events_from(path: &Path, start_offset: u64) -> Result<ReadResult> {
    let mut file =
        File::open(path).with_context(|| format!("无法打开 Codex 任务记录 {}", path.display()))?;
    let length = file
        .metadata()
        .with_context(|| format!("无法检查 Codex 任务记录 {}", path.display()))?
        .len();
    let start_offset = start_offset.min(length);
    file.seek(SeekFrom::Start(start_offset))
        .with_context(|| format!("无法定位 Codex 任务记录 {}", path.display()))?;

    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut offset = start_offset;
    let mut events = Vec::new();

    loop {
        let line_start = offset;
        line.clear();
        let line_read = read_bounded_line(&mut reader, &mut line)
            .with_context(|| format!("无法读取 Codex 任务记录 {}", path.display()))?;
        let BoundedLine::Complete { bytes, too_large } = line_read else {
            // The JSON record has not been fully written yet. Leave the
            // cursor at its beginning so the next pass can parse it intact.
            break;
        };
        offset = offset.saturating_add(bytes);
        if too_large {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        for kind in record_events(&record) {
            events.push(TranscriptEvent {
                end_offset: offset,
                kind,
            });
        }

        debug_assert!(offset >= line_start);
    }

    Ok(ReadResult {
        next_offset: offset,
        events,
    })
}

enum BoundedLine {
    Complete { bytes: u64, too_large: bool },
    PartialOrEnd,
}

/// Read one newline-terminated record without allocating more than the parser
/// cap. The reader may consume a partial line internally, but the caller keeps
/// its external offset unchanged until a newline is observed.
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> std::io::Result<BoundedLine> {
    let mut bytes = 0_u64;
    let mut too_large = false;

    loop {
        let (consumed, complete) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                return Ok(BoundedLine::PartialOrEnd);
            }
            let consumed = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            let next_length = bytes.saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
            if !too_large && next_length <= MAX_RECORD_BYTES as u64 {
                line.extend_from_slice(&available[..consumed]);
            } else {
                too_large = true;
                line.clear();
            }
            (
                consumed,
                consumed < available.len() || available[consumed - 1] == b'\n',
            )
        };
        reader.consume(consumed);
        bytes = bytes.saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
        if complete {
            return Ok(BoundedLine::Complete { bytes, too_large });
        }
    }
}

fn record_events(record: &Value) -> Vec<TranscriptEventKind> {
    let Some(record_type) = record.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };
    let payload = record.get("payload").and_then(Value::as_object);

    match record_type {
        "session_meta" => payload
            .map(session_meta)
            .filter(|meta| !meta.session_id.is_empty() || !meta.cwd.is_empty())
            .map(TranscriptEventKind::SessionMeta)
            .into_iter()
            .collect(),
        "response_item" => payload
            .and_then(user_prompt)
            .map(TranscriptEventKind::UserPrompt)
            .into_iter()
            .collect(),
        "event_msg" => payload.map(event_message).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn session_meta(payload: &serde_json::Map<String, Value>) -> SessionMeta {
    SessionMeta {
        session_id: string_field(payload, "id")
            .or_else(|| string_field(payload, "session_id"))
            .unwrap_or_default(),
        cwd: string_field(payload, "cwd").unwrap_or_default(),
    }
}

fn user_prompt(payload: &serde_json::Map<String, Value>) -> Option<UserPrompt> {
    if payload.get("type").and_then(Value::as_str) != Some("message")
        || payload.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }

    let turn_id = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(Value::as_object)
        .and_then(|metadata| string_field(metadata, "turn_id"))
        .unwrap_or_default();
    let mut texts = Vec::new();
    for content in payload
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if content.get("type").and_then(Value::as_str) != Some("input_text") {
            continue;
        }
        let Some(text) = content.get("text").and_then(Value::as_str) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() || is_context_only(text) {
            continue;
        }
        texts.push(text);
    }
    let prompt = texts.join("\n\n");
    (!prompt.is_empty()).then_some(UserPrompt { turn_id, prompt })
}

fn event_message(payload: &serde_json::Map<String, Value>) -> Vec<TranscriptEventKind> {
    match payload.get("type").and_then(Value::as_str) {
        Some("task_started") => vec![TranscriptEventKind::TaskStarted],
        Some("thread_goal_updated") => payload
            .get("goal")
            .and_then(Value::as_object)
            .and_then(|goal| string_field(goal, "status"))
            .map(normalized)
            .filter(|status| !status.is_empty())
            .map(TranscriptEventKind::GoalStatus)
            .into_iter()
            .collect(),
        Some("task_complete") => match terminal_error(payload) {
            Some(error) => vec![TranscriptEventKind::TerminalError(error)],
            None => vec![TranscriptEventKind::TaskCompleted(task_completion(payload))],
        },
        _ => Vec::new(),
    }
}

fn task_completion(payload: &serde_json::Map<String, Value>) -> TaskCompletion {
    TaskCompletion {
        turn_id: string_field(payload, "turn_id").unwrap_or_default(),
        last_agent_message: string_field(payload, "last_agent_message").unwrap_or_default(),
        completed_at_seconds: number_seconds(payload.get("completed_at")),
        duration_seconds: payload
            .get("duration_ms")
            .and_then(Value::as_u64)
            .map(|milliseconds| milliseconds / 1_000),
    }
}

fn terminal_error(payload: &serde_json::Map<String, Value>) -> Option<TerminalError> {
    let message = match payload.get("error") {
        Some(Value::Object(error)) => string_field(error, "message"),
        Some(Value::String(error)) => Some(error.trim().to_owned()),
        _ => None,
    }
    .filter(|message| !message.is_empty())
    .or_else(|| {
        let message = string_field(payload, "last_agent_message")?;
        known_usage_limit(&message).then_some(message)
    })?;

    Some(TerminalError {
        turn_id: string_field(payload, "turn_id").unwrap_or_default(),
        message,
        completed_at_seconds: number_seconds(payload.get("completed_at")),
        duration_seconds: payload
            .get("duration_ms")
            .and_then(Value::as_u64)
            .map(|milliseconds| milliseconds / 1_000),
    })
}

fn string_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn number_seconds(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.floor() as u64)
}

fn normalized(value: String) -> String {
    value.trim().to_ascii_lowercase()
}

fn known_usage_limit(message: &str) -> bool {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    KNOWN_USAGE_LIMIT_MESSAGES
        .iter()
        .any(|known| normalized.eq_ignore_ascii_case(known))
}

fn is_context_only(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    CONTEXT_ONLY_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECORD_BYTES, TranscriptEventKind, read_events_from};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn reads_terminal_errors_and_goal_events_without_consuming_partial_lines() {
        let directory = tempdir().expect("temporary directory");
        let transcript = directory.path().join("session.jsonl");
        let complete = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\",\"cwd\":\"/workspace\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"status\":\"active\"}}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"turn-1\",\"completed_at\":100,\"duration_ms\":4200,\"error\":{\"message\":\"stream disconnected\"}}}\n"
        );
        fs::write(&transcript, format!("{complete}{{\"type\":\"event_msg\""))
            .expect("write transcript");

        let first = read_events_from(&transcript, 0).expect("read transcript");
        assert_eq!(first.next_offset as usize, complete.len());
        assert!(first.events.iter().any(|event| matches!(
            &event.kind,
            TranscriptEventKind::GoalStatus(status) if status == "active"
        )));
        assert!(first.events.iter().any(|event| matches!(
            &event.kind,
            TranscriptEventKind::TerminalError(error)
                if error.turn_id == "turn-1" && error.duration_seconds == Some(4)
        )));

        fs::write(
            &transcript,
            format!("{complete}{{\"type\":\"event_msg\"}}\n"),
        )
        .expect("finish transcript line");
        let second = read_events_from(&transcript, first.next_offset).expect("read appended");
        assert!(second.next_offset > first.next_offset);
    }

    #[test]
    fn recognizes_the_exact_usage_limit_terminal_message() {
        let directory = tempdir().expect("temporary directory");
        let transcript = directory.path().join("session.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"turn-1\",\"last_agent_message\":\"\u{4f60}\u{5df2}\u{8fbe}\u{5230}\u{4f7f}\u{7528}\u{4e0a}\u{9650}\u{3002}\u{8bf7}\u{7a0d}\u{540e}\u{518d}\u{8bd5}\u{3002}\"}}\n",
        )
        .expect("write transcript");

        let result = read_events_from(&transcript, 0).expect("read transcript");
        assert!(result.events.iter().any(|event| matches!(
            &event.kind,
            TranscriptEventKind::TerminalError(error)
                if error.message.contains("\u{4f7f}\u{7528}\u{4e0a}\u{9650}")
        )));
    }

    #[test]
    fn reads_normal_task_completions_for_background_delivery() {
        let directory = tempdir().expect("temporary directory");
        let transcript = directory.path().join("session.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"turn-2\",\"last_agent_message\":\"Done\",\"completed_at\":101,\"duration_ms\":5200}}\n",
        )
        .expect("write transcript");

        let result = read_events_from(&transcript, 0).expect("read transcript");
        assert!(result.events.iter().any(|event| matches!(
            &event.kind,
            TranscriptEventKind::TaskCompleted(completion)
                if completion.turn_id == "turn-2"
                    && completion.last_agent_message == "Done"
                    && completion.completed_at_seconds == Some(101)
                    && completion.duration_seconds == Some(5)
        )));
    }

    #[test]
    fn skips_an_oversized_record_without_losing_the_next_complete_event() {
        let directory = tempdir().expect("temporary directory");
        let transcript = directory.path().join("session.jsonl");
        let oversized = "x".repeat(MAX_RECORD_BYTES + 1);
        fs::write(
            &transcript,
            format!(
                "{oversized}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n"
            ),
        )
        .expect("write transcript");

        let result = read_events_from(&transcript, 0).expect("read transcript");
        assert!(
            result
                .events
                .iter()
                .any(|event| { matches!(event.kind, TranscriptEventKind::TaskStarted) })
        );
    }
}

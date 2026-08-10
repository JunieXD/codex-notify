//! Durable, best-effort detection for terminal Codex transcript errors.
//!
//! A transcript error is not necessarily terminal: Codex can reconnect or a
//! Goal can continue automatically. Candidates therefore remain in a local
//! confirmation state until they either recover or meet the notification
//! criteria below.

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model::Notification;
use crate::paths::AppPaths;
use crate::settings::atomic_write;
use crate::state::{elapsed_since, find_thread_title, load_turn_state};
use crate::transcript::{
    JsonlTranscriptSource, TerminalError, TranscriptEventKind, TranscriptSource,
};

pub const WATCH_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_LOOKBACK: Duration = Duration::from_secs(10 * 60);
const TERMINAL_CONFIRMATION: Duration = Duration::from_secs(30);
const ACTIVE_GOAL_STALL: Duration = Duration::from_secs(10 * 60);
const MAX_TRANSCRIPT_AGE: Duration = Duration::from_secs(2 * 24 * 60 * 60);
const MAX_SEEN_EVENTS: usize = 4_096;
const MAX_COMPLETED_TURNS: usize = 4_096;
const MAX_CONFIRMING: usize = 128;
const MAX_PROMPTS_PER_FILE: usize = 128;
const DELIVERY_LEASE: Duration = Duration::from_secs(60);
const GOAL_FAILURE_STATUSES: &[&str] = &["blocked", "usage_limited", "budget_limited"];
const GOAL_STOP_STATUSES: &[&str] = &["paused", "complete"];

#[derive(Debug, Clone)]
pub struct PendingDelivery {
    pub key: String,
    pub notification: Notification,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchSummary {
    pub scanned_files: usize,
    pub new_candidates: usize,
    pub canceled_candidates: usize,
    pub pending_deliveries: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MonitorState {
    #[serde(default)]
    files: BTreeMap<String, FileCursor>,
    #[serde(default)]
    seen: Vec<String>,
    #[serde(default)]
    completed_turns: Vec<String>,
    #[serde(default)]
    confirming: Vec<Candidate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileCursor {
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    goal_status: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    prompts: Vec<PromptSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromptSnapshot {
    #[serde(default)]
    turn_id: String,
    #[serde(default)]
    prompt: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CandidateSource {
    Transcript,
    #[default]
    StopHook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Candidate {
    key: String,
    #[serde(default)]
    turn_id: String,
    #[serde(default)]
    error_message: String,
    #[serde(default)]
    task: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    thread_id: String,
    #[serde(default)]
    conversation_title: String,
    #[serde(default)]
    duration_seconds: Option<u64>,
    detected_at_seconds: u64,
    #[serde(default)]
    active_goal: bool,
    #[serde(default)]
    latest_goal_status: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    after_offset: u64,
    #[serde(default)]
    checked_offset: u64,
    #[serde(default)]
    source: CandidateSource,
    #[serde(default)]
    delivery_started_at_seconds: Option<u64>,
}

/// Scan recent transcript additions, update durable confirmation state, and
/// reserve any notifications that are ready to send. Call `settle_delivery`
/// after each provider request so failures remain retryable.
pub fn prepare_notifications(
    paths: &AppPaths,
    now: SystemTime,
) -> Result<(WatchSummary, Vec<PendingDelivery>)> {
    let now_seconds = unix_seconds(now);
    with_state(paths, |state, first_run| {
        let transcript_source = JsonlTranscriptSource;
        let mut summary = WatchSummary::default();
        let seen = state.seen.iter().cloned().collect::<HashSet<_>>();
        let completed = state
            .completed_turns
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let session_files = recent_session_files(paths, now_seconds);
        let active_paths = session_files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<HashSet<_>>();

        let scan_context = ScanContext {
            paths,
            transcript_source: &transcript_source,
            seen: &seen,
            completed: &completed,
            first_run,
            now_seconds,
        };
        for path in &session_files {
            scan_file(&scan_context, state, path, &mut summary)?;
        }
        state.files.retain(|path, _| active_paths.contains(path));
        apply_stop_hook_goal_context(state);

        let deliveries =
            confirm_candidates(paths, state, now_seconds, &mut summary, &transcript_source)?;
        prune_state(state);
        summary.pending_deliveries = deliveries.len();
        Ok((summary, deliveries))
    })
}

/// Mark a planned Feishu delivery as successful or retryable. A short delivery
/// lease prevents two concurrently started watcher processes from sending the
/// same card while the first one is awaiting the network.
pub fn settle_delivery(paths: &AppPaths, key: &str, delivered: bool) -> Result<()> {
    with_state(paths, |state, _| {
        let Some(index) = state
            .confirming
            .iter()
            .position(|candidate| candidate.key == key)
        else {
            return Ok(());
        };
        if delivered {
            let candidate = state.confirming.remove(index);
            add_bounded(&mut state.seen, candidate.key, MAX_SEEN_EVENTS);
            if !candidate.turn_id.is_empty() {
                state
                    .confirming
                    .retain(|other| other.turn_id != candidate.turn_id);
            }
        } else if let Some(candidate) = state.confirming.get_mut(index) {
            candidate.delivery_started_at_seconds = None;
        }
        prune_state(state);
        Ok(())
    })
}

/// A documented normal-completion notify event outranks a speculative Stop
/// fallback or a transcript candidate for the same turn.
pub fn mark_turn_completed(paths: &AppPaths, turn_id: &str) -> Result<()> {
    let turn_id = turn_id.trim();
    if turn_id.is_empty() {
        return Ok(());
    }
    with_state(paths, |state, _| {
        add_bounded(
            &mut state.completed_turns,
            turn_id.to_owned(),
            MAX_COMPLETED_TURNS,
        );
        state
            .confirming
            .retain(|candidate| candidate.turn_id != turn_id);
        Ok(())
    })
}

/// Record a Stop Hook that ended without a final assistant message. It does
/// not send immediately; the normal confirmation rules decide later whether
/// the missing result was an actual interruption.
pub fn record_stop_fallback(
    paths: &AppPaths,
    turn_id: &str,
    session_id: &str,
    cwd: &str,
    transcript_path: Option<&Path>,
    now: SystemTime,
) -> Result<bool> {
    let turn_id = turn_id.trim();
    if turn_id.is_empty() {
        return Ok(false);
    }
    let now_seconds = unix_seconds(now);
    with_state(paths, |state, _| {
        if state.completed_turns.iter().any(|known| known == turn_id)
            || state
                .confirming
                .iter()
                .any(|candidate| candidate.turn_id == turn_id)
        {
            return Ok(false);
        }

        // Stop has no original prompt. Requiring Prompt Hook state prevents a
        // background/internal Codex turn with no final text from producing a
        // generic user-facing alert. Transcript errors remain independently
        // detectable by the watcher.
        let Some(turn_state) = load_turn_state(&paths.state, turn_id).ok().flatten() else {
            return Ok(false);
        };
        let candidate_path = transcript_path
            .filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned());
        let offset = transcript_path
            .and_then(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let thread_id = nonempty(&turn_state.thread_id, session_id);
        let conversation_title = turn_state.conversation_title_at_start.unwrap_or_default();
        let task = if turn_state.prompt.trim().is_empty() {
            "\u{672a}\u{547d}\u{540d}\u{4efb}\u{52a1}".to_owned()
        } else {
            turn_state.prompt
        };
        let candidate_cwd = nonempty(&turn_state.cwd, cwd);
        state.confirming.push(Candidate {
            key: digest_key(&["stop", turn_id]),
            turn_id: turn_id.to_owned(),
            error_message: "\u{672a}\u{6536}\u{5230} Codex \u{7684}\u{6700}\u{7ec8}\u{5b8c}\u{6210}\u{6d88}\u{606f}\u{3002}\u{8bf7}\u{6253}\u{5f00} Codex \u{67e5}\u{770b}\u{8be6}\u{60c5}\u{3002}".to_owned(),
            task,
            cwd: candidate_cwd,
            thread_id,
            conversation_title,
            duration_seconds: None,
            detected_at_seconds: now_seconds,
            active_goal: false,
            latest_goal_status: String::new(),
            path: candidate_path,
            after_offset: offset,
            checked_offset: offset,
            source: CandidateSource::StopHook,
            delivery_started_at_seconds: None,
        });
        prune_state(state);
        Ok(true)
    })
}

pub fn monitor_state_path(paths: &AppPaths) -> PathBuf {
    paths.state.join("monitor.json")
}

struct ScanContext<'a> {
    paths: &'a AppPaths,
    transcript_source: &'a dyn TranscriptSource,
    seen: &'a HashSet<String>,
    completed: &'a HashSet<String>,
    first_run: bool,
    now_seconds: u64,
}

fn scan_file(
    context: &ScanContext<'_>,
    state: &mut MonitorState,
    path: &Path,
    summary: &mut WatchSummary,
) -> Result<()> {
    let path_key = path.to_string_lossy().into_owned();
    let length = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(_) => return Ok(()),
    };
    let is_new_file = !state.files.contains_key(&path_key);
    let mut cursor = state.files.remove(&path_key).unwrap_or_default();
    if cursor.offset > length {
        cursor = FileCursor::default();
    }
    let records = match context.transcript_source.read_events(path, cursor.offset) {
        Ok(records) => records,
        Err(error) => {
            state.files.insert(path_key, cursor);
            return Err(error);
        }
    };
    summary.scanned_files += 1;

    let next_offset = records.next_offset;
    for record in records.events {
        match record.kind {
            TranscriptEventKind::SessionMeta(meta) => {
                if !meta.session_id.is_empty() {
                    cursor.session_id = meta.session_id;
                }
                if !meta.cwd.is_empty() {
                    cursor.cwd = meta.cwd;
                }
            }
            TranscriptEventKind::UserPrompt(prompt) => {
                remember_prompt(&mut cursor, prompt.turn_id, prompt.prompt)
            }
            TranscriptEventKind::TaskStarted => {}
            TranscriptEventKind::GoalStatus(status) => cursor.goal_status = status,
            TranscriptEventKind::TerminalError(error) => {
                if !should_consider_error(
                    &error,
                    context.first_run || is_new_file,
                    context.now_seconds,
                ) {
                    continue;
                }
                let candidate = candidate_from_error(
                    context.paths,
                    &cursor,
                    &path_key,
                    record.end_offset,
                    error,
                    context.now_seconds,
                );
                if crate::codex::is_internal_prompt(&candidate.task) {
                    continue;
                }
                if context.seen.contains(&candidate.key)
                    || (!candidate.turn_id.is_empty()
                        && context.completed.contains(&candidate.turn_id))
                {
                    continue;
                }
                if add_or_upgrade_candidate(&mut state.confirming, candidate) {
                    summary.new_candidates += 1;
                }
            }
        }
    }
    cursor.offset = next_offset;
    state.files.insert(path_key, cursor);
    Ok(())
}

fn candidate_from_error(
    paths: &AppPaths,
    cursor: &FileCursor,
    path: &str,
    after_offset: u64,
    error: TerminalError,
    now_seconds: u64,
) -> Candidate {
    let turn_state = load_turn_state(&paths.state, &error.turn_id).ok().flatten();
    let fallback_prompt = cursor
        .prompts
        .iter()
        .rev()
        .find(|prompt| !error.turn_id.is_empty() && prompt.turn_id == error.turn_id)
        .or_else(|| cursor.prompts.last())
        .map(|prompt| prompt.prompt.as_str())
        .unwrap_or("\u{672a}\u{547d}\u{540d}\u{4efb}\u{52a1}");
    let task = turn_state
        .as_ref()
        .map(|state| state.prompt.trim())
        .filter(|task| !task.is_empty())
        .unwrap_or(fallback_prompt)
        .to_owned();
    let thread_id = nonempty(
        turn_state
            .as_ref()
            .map(|state| state.thread_id.as_str())
            .unwrap_or_default(),
        &cursor.session_id,
    );
    let conversation_title = turn_state
        .as_ref()
        .and_then(|state| state.conversation_title_at_start.clone())
        .unwrap_or_default();
    let cwd = nonempty(
        turn_state
            .as_ref()
            .map(|state| state.cwd.as_str())
            .unwrap_or_default(),
        &cursor.cwd,
    );
    let key = digest_key(&[
        "transcript",
        &error.turn_id,
        &error.completed_at_seconds.unwrap_or_default().to_string(),
        &error.message,
    ]);

    Candidate {
        key,
        turn_id: error.turn_id,
        error_message: error.message,
        task,
        cwd,
        thread_id,
        conversation_title,
        duration_seconds: error.duration_seconds,
        detected_at_seconds: now_seconds,
        active_goal: cursor.goal_status == "active",
        latest_goal_status: String::new(),
        path: Some(path.to_owned()),
        after_offset,
        checked_offset: after_offset,
        source: CandidateSource::Transcript,
        delivery_started_at_seconds: None,
    }
}

fn add_or_upgrade_candidate(candidates: &mut Vec<Candidate>, candidate: Candidate) -> bool {
    if let Some(existing) = candidates.iter_mut().find(|existing| {
        existing.key == candidate.key
            || (!candidate.turn_id.is_empty() && existing.turn_id == candidate.turn_id)
    }) {
        if existing.source == CandidateSource::StopHook || existing.key != candidate.key {
            *existing = candidate;
        }
        return false;
    }
    candidates.push(candidate);
    true
}

fn apply_stop_hook_goal_context(state: &mut MonitorState) {
    for candidate in &mut state.confirming {
        if candidate.source != CandidateSource::StopHook {
            continue;
        }
        let Some(path) = candidate.path.as_ref() else {
            continue;
        };
        let Some(cursor) = state.files.get(path) else {
            continue;
        };
        if cursor.goal_status.is_empty() {
            continue;
        }
        candidate.latest_goal_status = cursor.goal_status.clone();
        if cursor.goal_status == "active" {
            candidate.active_goal = true;
        }
    }
}

fn confirm_candidates(
    paths: &AppPaths,
    state: &mut MonitorState,
    now_seconds: u64,
    summary: &mut WatchSummary,
    transcript_source: &dyn TranscriptSource,
) -> Result<Vec<PendingDelivery>> {
    let completed = state
        .completed_turns
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut retained = Vec::with_capacity(state.confirming.len());
    let mut deliveries = Vec::new();

    for mut candidate in std::mem::take(&mut state.confirming) {
        if !candidate.turn_id.is_empty() && completed.contains(&candidate.turn_id) {
            summary.canceled_candidates += 1;
            continue;
        }
        if candidate
            .delivery_started_at_seconds
            .is_some_and(|started| now_seconds.saturating_sub(started) < DELIVERY_LEASE.as_secs())
        {
            retained.push(candidate);
            continue;
        }
        if let Some(path) = candidate.path.clone() {
            let evidence = candidate_evidence(transcript_source, Path::new(&path), &mut candidate)?;
            if evidence.resumed {
                add_bounded(&mut state.seen, candidate.key, MAX_SEEN_EVENTS);
                summary.canceled_candidates += 1;
                continue;
            }
            if let Some(status) = evidence.latest_goal_status {
                candidate.latest_goal_status = status;
            }
        }

        if goal_stop_status(&candidate.latest_goal_status)
            && (candidate.active_goal || candidate.source == CandidateSource::StopHook)
        {
            add_bounded(&mut state.seen, candidate.key, MAX_SEEN_EVENTS);
            summary.canceled_candidates += 1;
            continue;
        }

        let age = now_seconds.saturating_sub(candidate.detected_at_seconds);
        let is_due = if candidate.active_goal {
            goal_failure_status(&candidate.latest_goal_status) || age >= ACTIVE_GOAL_STALL.as_secs()
        } else {
            age >= TERMINAL_CONFIRMATION.as_secs()
        };
        if !is_due {
            retained.push(candidate);
            continue;
        }

        candidate.delivery_started_at_seconds = Some(now_seconds);
        let notification = interruption_notification(paths, &candidate, now_seconds);
        deliveries.push(PendingDelivery {
            key: candidate.key.clone(),
            notification,
        });
        retained.push(candidate);
    }

    state.confirming = retained;
    Ok(deliveries)
}

#[derive(Default)]
struct CandidateEvidence {
    resumed: bool,
    latest_goal_status: Option<String>,
}

fn candidate_evidence(
    transcript_source: &dyn TranscriptSource,
    path: &Path,
    candidate: &mut Candidate,
) -> Result<CandidateEvidence> {
    if !path.is_file() {
        return Ok(CandidateEvidence::default());
    }
    candidate.checked_offset = candidate.checked_offset.max(candidate.after_offset);
    let records = transcript_source.read_events(path, candidate.checked_offset)?;
    candidate.checked_offset = records.next_offset;
    let mut evidence = CandidateEvidence::default();
    for record in records.events {
        match record.kind {
            TranscriptEventKind::TaskStarted => evidence.resumed = true,
            TranscriptEventKind::GoalStatus(status) => evidence.latest_goal_status = Some(status),
            _ => {}
        }
    }
    Ok(evidence)
}

fn interruption_notification(
    paths: &AppPaths,
    candidate: &Candidate,
    now_seconds: u64,
) -> Notification {
    let turn_state = load_turn_state(&paths.state, &candidate.turn_id)
        .ok()
        .flatten();
    let task = turn_state
        .as_ref()
        .map(|state| state.prompt.trim())
        .filter(|task| !task.is_empty())
        .unwrap_or(candidate.task.trim())
        .to_owned();
    let thread_id = nonempty(
        turn_state
            .as_ref()
            .map(|state| state.thread_id.as_str())
            .unwrap_or_default(),
        &candidate.thread_id,
    );
    let title = find_thread_title(&paths.session_index(), &thread_id)
        .or_else(|| {
            turn_state
                .as_ref()
                .and_then(|state| state.conversation_title_at_start.clone())
        })
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| {
            if candidate.conversation_title.trim().is_empty() {
                "Codex \u{4f1a}\u{8bdd}".to_owned()
            } else {
                candidate.conversation_title.clone()
            }
        });
    let cwd = nonempty(
        turn_state
            .as_ref()
            .map(|state| state.cwd.as_str())
            .unwrap_or_default(),
        &candidate.cwd,
    );
    let elapsed = candidate
        .duration_seconds
        .map(Duration::from_secs)
        .or_else(|| {
            turn_state.as_ref().and_then(|state| {
                UNIX_EPOCH
                    .checked_add(Duration::from_secs(now_seconds))
                    .and_then(|now| elapsed_since(state, now))
            })
        });
    let details = if cwd.is_empty() {
        format!("\u{9519}\u{8bef}\u{ff1a}{}", candidate.error_message)
    } else {
        format!(
            "\u{9519}\u{8bef}\u{ff1a}{}\n\u{5de5}\u{4f5c}\u{76ee}\u{5f55}\u{ff1a}{cwd}",
            candidate.error_message
        )
    };
    let mut notification = Notification::interrupted(title, task, details, elapsed, &candidate.key);
    notification.workspace = (!cwd.is_empty()).then(|| PathBuf::from(cwd));
    notification
}

fn should_consider_error(error: &TerminalError, initial_index: bool, now_seconds: u64) -> bool {
    if !initial_index {
        return true;
    }
    error.completed_at_seconds.is_some_and(|completed| {
        now_seconds.saturating_sub(completed) <= INITIAL_LOOKBACK.as_secs()
    })
}

fn recent_session_files(paths: &AppPaths, now_seconds: u64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for day in session_day_directories(&paths.codex_home, now_seconds) {
        let Ok(entries) = fs::read_dir(day) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }

    let archived = paths.codex_home.join("archived_sessions");
    if let Ok(entries) = fs::read_dir(archived) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            let is_recent = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age <= MAX_TRANSCRIPT_AGE);
            if is_recent {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn session_day_directories(codex_home: &Path, now_seconds: u64) -> Vec<PathBuf> {
    let day = i64::try_from(now_seconds / 86_400).unwrap_or(i64::MAX);
    // Include a day on either side of UTC today. This covers local session
    // directories around every timezone boundary without walking history.
    (-2..=1)
        .map(|offset| civil_date(day.saturating_add(offset)))
        .map(|(year, month, day)| {
            codex_home
                .join("sessions")
                .join(format!("{year:04}"))
                .join(format!("{month:02}"))
                .join(format!("{day:02}"))
        })
        .collect()
}

fn civil_date(days_since_epoch: i64) -> (i64, u32, u32) {
    // Howard Hinnant's public-domain civil-from-days algorithm.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (
        year,
        u32::try_from(month).unwrap_or_default(),
        u32::try_from(day).unwrap_or_default(),
    )
}

fn remember_prompt(cursor: &mut FileCursor, turn_id: String, prompt: String) {
    cursor.prompts.push(PromptSnapshot { turn_id, prompt });
    if cursor.prompts.len() > MAX_PROMPTS_PER_FILE {
        let excess = cursor.prompts.len() - MAX_PROMPTS_PER_FILE;
        cursor.prompts.drain(..excess);
    }
}

fn goal_failure_status(status: &str) -> bool {
    GOAL_FAILURE_STATUSES.contains(&status)
}

fn goal_stop_status(status: &str) -> bool {
    GOAL_STOP_STATUSES.contains(&status)
}

fn nonempty(primary: &str, fallback: &str) -> String {
    let primary = primary.trim();
    if primary.is_empty() {
        fallback.trim().to_owned()
    } else {
        primary.to_owned()
    }
}

fn digest_key(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn unix_seconds(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn add_bounded(values: &mut Vec<String>, value: String, maximum: usize) {
    values.retain(|existing| existing != &value);
    values.push(value);
    if values.len() > maximum {
        values.drain(..values.len() - maximum);
    }
}

fn prune_state(state: &mut MonitorState) {
    if state.seen.len() > MAX_SEEN_EVENTS {
        state.seen.drain(..state.seen.len() - MAX_SEEN_EVENTS);
    }
    if state.completed_turns.len() > MAX_COMPLETED_TURNS {
        state
            .completed_turns
            .drain(..state.completed_turns.len() - MAX_COMPLETED_TURNS);
    }
    if state.confirming.len() > MAX_CONFIRMING {
        state
            .confirming
            .drain(..state.confirming.len() - MAX_CONFIRMING);
    }
}

fn with_state<T>(
    paths: &AppPaths,
    operation: impl FnOnce(&mut MonitorState, bool) -> Result<T>,
) -> Result<T> {
    fs::create_dir_all(&paths.state)
        .with_context(|| format!("无法创建目录 {}", paths.state.display()))?;
    let lock_path = paths.state.join("monitor.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("无法打开 {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("无法锁定 {}", lock_path.display()))?;

    let result = (|| {
        let state_path = monitor_state_path(paths);
        let first_run = !state_path.exists();
        let mut state = load_state(&state_path)?;
        let result = operation(&mut state, first_run)?;
        let contents = serde_json::to_vec(&state).context("无法生成后台监听状态数据")?;
        atomic_write(&state_path, &contents)?;
        Ok(result)
    })();
    let _ = FileExt::unlock(&lock);
    result
}

fn load_state(path: &Path) -> Result<MonitorState> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MonitorState::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取 {}", path.display()));
        }
    };
    serde_json::from_slice(&contents)
        .with_context(|| format!("无法解析状态文件 {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{
        WATCH_INTERVAL, mark_turn_completed, prepare_notifications, record_stop_fallback,
        session_day_directories, settle_delivery,
    };
    use crate::model::Outcome;
    use crate::paths::AppPaths;
    use crate::state::{TurnState, write_turn_state};
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;

    fn paths() -> (tempfile::TempDir, tempfile::TempDir, AppPaths) {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = AppPaths {
            root: app_home.path().to_path_buf(),
            config: app_home.path().join("config.toml"),
            state: app_home.path().join("state"),
            logs: app_home.path().join("logs"),
            backups: app_home.path().join("backups"),
            codex_home: codex_home.path().to_path_buf(),
        };
        (app_home, codex_home, paths)
    }

    fn timestamp() -> (SystemTime, u64) {
        let now = SystemTime::now();
        let seconds = now
            .duration_since(UNIX_EPOCH)
            .expect("current timestamp")
            .as_secs();
        (now, seconds)
    }

    fn transcript_path(paths: &AppPaths, now_seconds: u64) -> std::path::PathBuf {
        let directory = session_day_directories(&paths.codex_home, now_seconds)
            .into_iter()
            .nth(2)
            .expect("today directory");
        fs::create_dir_all(&directory).expect("create session directory");
        directory.join("rollout-session.jsonl")
    }

    fn transcript_with_error(now_seconds: u64, goal: Option<&str>) -> String {
        let goal = goal.map(|status| format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"thread_goal_updated\",\"goal\":{{\"status\":\"{status}\"}}}}}}\n"
        )).unwrap_or_default();
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-1\",\"cwd\":\"/workspace\"}}}}\n\
             {{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"internal_chat_message_metadata_passthrough\":{{\"turn_id\":\"turn-1\"}},\"content\":[{{\"type\":\"input_text\",\"text\":\"Implement watcher\"}}]}}}}\n\
             {goal}\
             {{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\",\"turn_id\":\"turn-1\",\"completed_at\":{now_seconds},\"duration_ms\":65000,\"error\":{{\"message\":\"stream disconnected before completion\"}}}}}}\n"
        )
    }

    #[test]
    fn waits_before_sending_a_transcript_error_then_deduplicates_delivery() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        fs::write(&transcript, transcript_with_error(seconds, None)).expect("write transcript");

        let (first, deliveries) = prepare_notifications(&paths, now).expect("first scan");
        assert_eq!(first.new_candidates, 1);
        assert!(deliveries.is_empty());

        let (_, deliveries) = prepare_notifications(&paths, now + Duration::from_secs(31))
            .expect("confirmation scan");
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].notification.outcome, Outcome::Interrupted);
        assert!(
            deliveries[0]
                .notification
                .details_markdown
                .contains("stream disconnected")
        );
        assert_eq!(
            deliveries[0].notification.elapsed,
            Some(Duration::from_secs(65))
        );

        settle_delivery(&paths, &deliveries[0].key, true).expect("settle delivery");
        let (_, after_delivery) = prepare_notifications(&paths, now + Duration::from_secs(62))
            .expect("deduplicated scan");
        assert!(after_delivery.is_empty());
    }

    #[test]
    fn subsequent_task_start_cancels_an_apparent_error_in_the_same_scan() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        let contents = format!(
            "{}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n",
            transcript_with_error(seconds, None)
        );
        fs::write(transcript, contents).expect("write transcript");

        let (summary, deliveries) = prepare_notifications(&paths, now).expect("scan");
        assert!(deliveries.is_empty());
        assert_eq!(summary.canceled_candidates, 1);
        let (_, after_wait) =
            prepare_notifications(&paths, now + WATCH_INTERVAL + Duration::from_secs(1))
                .expect("follow-up scan");
        assert!(after_wait.is_empty());
    }

    #[test]
    fn active_goal_waits_until_blocked_instead_of_alerting_on_a_single_failure() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        fs::write(&transcript, transcript_with_error(seconds, Some("active")))
            .expect("write transcript");

        let (_, first) = prepare_notifications(&paths, now).expect("first scan");
        assert!(first.is_empty());
        let (_, waiting) =
            prepare_notifications(&paths, now + Duration::from_secs(31)).expect("active goal scan");
        assert!(waiting.is_empty());

        fs::write(
            &transcript,
            format!(
                "{}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"thread_goal_updated\",\"goal\":{{\"status\":\"blocked\"}}}}}}\n",
                transcript_with_error(seconds, Some("active"))
            ),
        )
        .expect("append blocked goal");
        let (_, blocked) = prepare_notifications(&paths, now + Duration::from_secs(32))
            .expect("blocked goal scan");
        assert_eq!(blocked.len(), 1);
    }

    #[test]
    fn active_goal_notifies_after_ten_minutes_of_silence() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        fs::write(&transcript, transcript_with_error(seconds, Some("active")))
            .expect("write transcript");

        let (_, first) = prepare_notifications(&paths, now).expect("first scan");
        assert!(first.is_empty());
        let (_, stalled) = prepare_notifications(&paths, now + Duration::from_secs(600))
            .expect("stalled goal scan");
        assert_eq!(stalled.len(), 1);
    }

    #[test]
    fn active_goal_completion_cancels_an_error_candidate() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        fs::write(&transcript, transcript_with_error(seconds, Some("active")))
            .expect("write transcript");
        let (_, first) = prepare_notifications(&paths, now).expect("first scan");
        assert!(first.is_empty());

        fs::write(
            &transcript,
            format!(
                "{}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"thread_goal_updated\",\"goal\":{{\"status\":\"complete\"}}}}}}\n",
                transcript_with_error(seconds, Some("active"))
            ),
        )
        .expect("append complete goal");
        let (_, completed) = prepare_notifications(&paths, now + Duration::from_secs(601))
            .expect("complete goal scan");
        assert!(completed.is_empty());
    }

    #[test]
    fn known_usage_limit_message_reaches_the_interruption_flow() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        let contents = transcript_with_error(seconds, None)
            .replace("\"error\":{\"message\":\"stream disconnected before completion\"}", "\"last_agent_message\":\"\u{4f60}\u{5df2}\u{8fbe}\u{5230}\u{4f7f}\u{7528}\u{4e0a}\u{9650}\u{3002}\u{8bf7}\u{7a0d}\u{540e}\u{518d}\u{8bd5}\u{3002}\"");
        fs::write(transcript, contents).expect("write transcript");

        let (_, first) = prepare_notifications(&paths, now).expect("first scan");
        assert!(first.is_empty());
        let (_, deliveries) = prepare_notifications(&paths, now + Duration::from_secs(31))
            .expect("confirmed usage-limit scan");
        assert_eq!(deliveries.len(), 1);
        assert!(
            deliveries[0]
                .notification
                .details_markdown
                .contains("\u{4f7f}\u{7528}\u{4e0a}\u{9650}")
        );
    }

    #[test]
    fn normal_completion_cancels_a_stop_fallback_before_confirmation() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        fs::write(&transcript, "").expect("create transcript");
        write_turn_state(
            &paths.state,
            "turn-1",
            &TurnState::new("Task", "/workspace", "thread-1", None),
        )
        .expect("write turn state");

        assert!(
            record_stop_fallback(
                &paths,
                "turn-1",
                "thread-1",
                "/workspace",
                Some(&transcript),
                now,
            )
            .expect("record fallback")
        );
        mark_turn_completed(&paths, "turn-1").expect("mark completion");
        let (_, deliveries) =
            prepare_notifications(&paths, now + Duration::from_secs(31)).expect("scan");
        assert!(deliveries.is_empty());
    }

    #[test]
    fn internal_transcript_turns_do_not_create_interruption_candidates() {
        let (_app_home, _codex_home, paths) = paths();
        let (now, seconds) = timestamp();
        let transcript = transcript_path(&paths, seconds);
        let contents = transcript_with_error(seconds, None).replace(
            "Implement watcher",
            "Generate 0 to 3 hyperpersonalized suggestions for what this user can do with Codex",
        );
        fs::write(transcript, contents).expect("write transcript");

        let (summary, deliveries) = prepare_notifications(&paths, now).expect("scan");
        assert_eq!(summary.new_candidates, 0);
        assert!(deliveries.is_empty());
    }
}

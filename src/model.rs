use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Interrupted,
}

impl Outcome {
    pub fn emoji(self) -> &'static str {
        match self {
            Self::Completed => "\u{2705}",
            Self::Interrupted => "\u{26a0}\u{fe0f}",
        }
    }

    pub fn card_template(self) -> &'static str {
        match self {
            Self::Completed => "green",
            Self::Interrupted => "red",
        }
    }

    pub fn heading(self) -> &'static str {
        match self {
            Self::Completed => "Codex \u{4efb}\u{52a1}\u{5b8c}\u{6210}",
            Self::Interrupted => "Codex \u{4efb}\u{52a1}\u{5f02}\u{5e38}\u{4e2d}\u{65ad}",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub outcome: Outcome,
    pub conversation_title: String,
    pub task: String,
    pub details_markdown: String,
    pub elapsed: Option<Duration>,
    pub workspace: Option<PathBuf>,
    pub event_id: String,
    pub occurred_at: SystemTime,
}

impl Notification {
    pub fn completed(
        conversation_title: impl Into<String>,
        task: impl Into<String>,
        details_markdown: impl Into<String>,
        elapsed: Option<Duration>,
        event_id: impl Into<String>,
    ) -> Self {
        Self {
            outcome: Outcome::Completed,
            conversation_title: conversation_title.into(),
            task: task.into(),
            details_markdown: details_markdown.into(),
            elapsed,
            workspace: None,
            event_id: event_id.into(),
            occurred_at: SystemTime::now(),
        }
    }

    pub fn interrupted(
        conversation_title: impl Into<String>,
        task: impl Into<String>,
        details_markdown: impl Into<String>,
        elapsed: Option<Duration>,
        event_id: impl Into<String>,
    ) -> Self {
        Self {
            outcome: Outcome::Interrupted,
            conversation_title: conversation_title.into(),
            task: task.into(),
            details_markdown: details_markdown.into(),
            elapsed,
            workspace: None,
            event_id: event_id.into(),
            occurred_at: SystemTime::now(),
        }
    }
}

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;
    use std::time::Duration;

    #[test]
    fn formats_duration_for_seconds_minutes_and_hours() {
        assert_eq!(format_duration(Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1m 5s");
        assert_eq!(format_duration(Duration::from_secs(3_661)), "1h 1m 1s");
    }
}

use crate::model::{Notification, format_duration};
use serde_json::{Value, json};

pub const MAX_CARD_CONTENT_BYTES: usize = 28_000;
const MAX_TASK_BYTES: usize = 6_000;
const MAX_DETAILS_BYTES: usize = 20_000;
const TRUNCATION_SUFFIX: &str = "\n\n(\u{5185}\u{5bb9}\u{8fc7}\u{957f}\u{ff0c}\u{5df2}\u{622a}\u{65ad}\u{ff1b}\u{5b8c}\u{6574}\u{5185}\u{5bb9}\u{8bf7}\u{5728} Codex \u{4e2d}\u{67e5}\u{770b}\u{3002})";

#[derive(Debug, Clone)]
pub struct RenderedCard {
    pub outer_title: String,
    pub serialized_content: String,
    pub value: Value,
}

pub fn render(notification: &Notification) -> RenderedCard {
    let title = outer_title(notification);
    let heading = heading(notification);
    let task_text = normalize_text(&notification.task);
    let task = shorten_utf8(&task_text, MAX_TASK_BYTES);
    let details_text = normalize_text(&notification.details_markdown);
    let details = shorten_utf8(&details_text, MAX_DETAILS_BYTES);

    let (task, details, value, serialized_content) =
        fit_card(notification, &title, &heading, task, details);

    debug_assert!(!task.is_empty() || !details.is_empty());
    debug_assert!(serialized_content.len() <= MAX_CARD_CONTENT_BYTES);

    RenderedCard {
        outer_title: title,
        serialized_content,
        value,
    }
}

fn normalize_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "\u{672a}\u{547d}\u{540d}".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn outer_title(notification: &Notification) -> String {
    let title = single_line(&notification.conversation_title, 77);
    let title = if title.is_empty() {
        "Codex \u{4f1a}\u{8bdd}".to_owned()
    } else {
        title
    };
    format!("{} {title}", notification.outcome.emoji())
}

fn heading(notification: &Notification) -> String {
    match notification.elapsed {
        Some(elapsed) => format!(
            "{} {} \u{00b7} {}",
            notification.outcome.emoji(),
            notification.outcome.heading(),
            format_duration(elapsed)
        ),
        None => format!(
            "{} {}",
            notification.outcome.emoji(),
            notification.outcome.heading()
        ),
    }
}

fn single_line(value: &str, maximum_characters: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let character_count = compact.chars().count();
    if character_count <= maximum_characters {
        return compact;
    }

    let shortened = compact
        .chars()
        .take(maximum_characters.saturating_sub(1))
        .collect::<String>();
    format!("{shortened}\u{2026}")
}

fn card_value(
    notification: &Notification,
    title: &str,
    heading: &str,
    task: &str,
    details: &str,
) -> Value {
    let markdown_content =
        format!("**\u{4efb}\u{52a1}**\n{task}\n\n**\u{7ed3}\u{679c}**\n{details}");

    json!({
        "schema": "2.0",
        "header": {
            "template": notification.outcome.card_template(),
            "title": {
                "tag": "plain_text",
                "content": title,
            },
        },
        "body": {
            "elements": [{
                "tag": "collapsible_panel",
                "expanded": false,
                "header": {
                    "title": {
                        "tag": "plain_text",
                        "content": heading,
                    },
                    "icon": {
                        "tag": "standard_icon",
                        "token": "down-small-ccm_outlined",
                        "size": "16px 16px",
                    },
                    "icon_position": "right",
                    "icon_expanded_angle": -180,
                },
                "elements": [{
                    "tag": "markdown",
                    "content": markdown_content,
                }],
            }],
        },
    })
}

fn serialized(value: &Value) -> String {
    serde_json::to_string(value).expect("Feishu card values must be serializable")
}

fn fit_card(
    notification: &Notification,
    title: &str,
    heading: &str,
    mut task: String,
    mut details: String,
) -> (String, String, Value, String) {
    let mut value = card_value(notification, title, heading, &task, &details);
    let mut serialized_content = serialized(&value);
    if serialized_content.len() <= MAX_CARD_CONTENT_BYTES {
        return (task, details, value, serialized_content);
    }

    details = fit_text_to_card(&details, |candidate| {
        card_value(notification, title, heading, &task, candidate)
    });
    value = card_value(notification, title, heading, &task, &details);
    serialized_content = serialized(&value);
    if serialized_content.len() <= MAX_CARD_CONTENT_BYTES {
        return (task, details, value, serialized_content);
    }

    task = fit_text_to_card(&task, |candidate| {
        card_value(notification, title, heading, candidate, "")
    });
    details.clear();
    value = card_value(notification, title, heading, &task, &details);
    serialized_content = serialized(&value);

    (task, details, value, serialized_content)
}

fn fit_text_to_card<F>(source: &str, render: F) -> String
where
    F: Fn(&str) -> Value,
{
    let mut lower = 0;
    let mut upper = source.len();
    let mut best = String::new();

    while lower <= upper {
        let middle = lower + (upper - lower) / 2;
        let candidate = shorten_utf8(source, middle);
        if serialized(&render(&candidate)).len() <= MAX_CARD_CONTENT_BYTES {
            best = candidate;
            lower = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            upper = middle - 1;
        }
    }

    best
}

pub fn shorten_utf8(value: &str, maximum_bytes: usize) -> String {
    if maximum_bytes == 0 {
        return String::new();
    }
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    if maximum_bytes <= TRUNCATION_SUFFIX.len() {
        return utf8_prefix(value, maximum_bytes);
    }

    let prefix_bytes = maximum_bytes - TRUNCATION_SUFFIX.len();
    format!("{}{}", utf8_prefix(value, prefix_bytes), TRUNCATION_SUFFIX)
}

fn utf8_prefix(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }

    let mut ending = 0;
    for (index, character) in value.char_indices() {
        if index + character.len_utf8() > maximum_bytes {
            break;
        }
        ending = index + character.len_utf8();
    }
    value[..ending].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{MAX_CARD_CONTENT_BYTES, render, shorten_utf8};
    use crate::model::Notification;
    use std::time::Duration;

    fn completion() -> Notification {
        Notification::completed(
            "\u{786e}\u{8ba4}\u{4efb}\u{52a1}\u{5b8c}\u{6210}\u{901a}\u{77e5}\u{80fd}\u{529b}",
            "\u{8bf7}\u{5b9e}\u{73b0}\u{98de}\u{4e66}\u{901a}\u{77e5}",
            "## \u{5df2}\u{5b8c}\u{6210}\n\n- \u{5361}\u{7247}\n- Markdown",
            Some(Duration::from_secs(3_661)),
            "turn-1",
        )
    }

    #[test]
    fn outer_title_uses_status_and_conversation_title() {
        let card = render(&completion());
        assert_eq!(
            card.outer_title,
            "\u{2705} \u{786e}\u{8ba4}\u{4efb}\u{52a1}\u{5b8c}\u{6210}\u{901a}\u{77e5}\u{80fd}\u{529b}"
        );
        assert!(
            !card
                .outer_title
                .contains("\u{8bf7}\u{5b9e}\u{73b0}\u{98de}\u{4e66}\u{901a}\u{77e5}")
        );
    }

    #[test]
    fn card_keeps_details_in_a_collapsed_markdown_panel() {
        let card = render(&completion());
        let panel = &card.value["body"]["elements"][0];
        assert_eq!(panel["tag"], "collapsible_panel");
        assert_eq!(panel["expanded"], false);
        assert!(card.serialized_content.contains("**\u{4efb}\u{52a1}**"));
        assert!(card.serialized_content.contains("1h 1m 1s"));
    }

    #[test]
    fn card_size_is_capped_after_json_escaping() {
        let mut notification = completion();
        notification.details_markdown = "\"\\\\\n".repeat(20_000);
        let card = render(&notification);
        assert!(card.serialized_content.len() <= MAX_CARD_CONTENT_BYTES);
        assert!(serde_json::from_str::<serde_json::Value>(&card.serialized_content).is_ok());
    }

    #[test]
    fn utf8_truncation_never_breaks_a_character() {
        assert_eq!(shorten_utf8("\u{4f60}\u{597d}", 4), "\u{4f60}");
    }
}

use crate::model::{Notification, format_duration};
use chrono::{DateTime, Datelike, Local, Timelike};
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
    let moment = DateTime::<Local>::from(notification.occurred_at);
    let title = outer_title(notification, &moment);
    let sent_at = format_sent_at(&moment);
    let heading = heading(notification);
    let task_text = normalize_text(&notification.task);
    let task = shorten_utf8(&task_text, MAX_TASK_BYTES);
    let details_text = normalize_text(&notification.details_markdown);
    let details = shorten_utf8(&details_text, MAX_DETAILS_BYTES);

    let (task, details, value, serialized_content) =
        fit_card(notification, &title, &heading, &sent_at, task, details);

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
        replace_markdown_media(trimmed)
    }
}

fn replace_markdown_media(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("![") {
        output.push_str(&remaining[..start]);
        let image = &remaining[start..];
        let Some(label_end) = image[2..].find("](").map(|index| index + 2) else {
            output.push_str(image);
            return output;
        };
        let destination_start = label_end + 2;
        let Some(destination_end) = markdown_destination_end(image, destination_start) else {
            output.push_str(image);
            return output;
        };
        let label = image[2..label_end].trim();
        let label = if label.is_empty() {
            "媒体文件"
        } else {
            label
        };
        output.push_str(&format!("附件：{label}（请在 Codex 中查看）"));
        remaining = &image[destination_end + 1..];
    }
    output.push_str(remaining);
    output
}

fn markdown_destination_end(value: &str, start: usize) -> Option<usize> {
    let mut depth = 0_u32;
    let mut escaped = false;
    for (offset, character) in value[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => depth = depth.saturating_add(1),
            ')' if depth == 0 => return Some(start + offset),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn outer_title(notification: &Notification, moment: &DateTime<Local>) -> String {
    let title = single_line(&notification.conversation_title, 71);
    let title = if title.is_empty() {
        "Codex \u{4f1a}\u{8bdd}".to_owned()
    } else {
        title
    };
    format!(
        "{} {:02}:{:02} {title}",
        notification.outcome.emoji(),
        moment.hour(),
        moment.minute()
    )
}

fn format_sent_at(moment: &DateTime<Local>) -> String {
    format!(
        "{}\u{6708}{}\u{65e5} {:02}:{:02}",
        moment.month(),
        moment.day(),
        moment.hour(),
        moment.minute()
    )
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
    sent_at: &str,
    task: &str,
    details: &str,
) -> Value {
    let markdown_content = format!(
        "**\u{53d1}\u{9001}\u{65f6}\u{95f4}** {sent_at}\n\n**\u{4efb}\u{52a1}**\n{task}\n\n**\u{7ed3}\u{679c}**\n{details}"
    );

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
    serde_json::to_string(value).expect("飞书卡片内容应当可以序列化")
}

fn fit_card(
    notification: &Notification,
    title: &str,
    heading: &str,
    sent_at: &str,
    mut task: String,
    mut details: String,
) -> (String, String, Value, String) {
    let mut value = card_value(notification, title, heading, sent_at, &task, &details);
    let mut serialized_content = serialized(&value);
    if serialized_content.len() <= MAX_CARD_CONTENT_BYTES {
        return (task, details, value, serialized_content);
    }

    details = fit_text_to_card(&details, |candidate| {
        card_value(notification, title, heading, sent_at, &task, candidate)
    });
    value = card_value(notification, title, heading, sent_at, &task, &details);
    serialized_content = serialized(&value);
    if serialized_content.len() <= MAX_CARD_CONTENT_BYTES {
        return (task, details, value, serialized_content);
    }

    task = fit_text_to_card(&task, |candidate| {
        card_value(notification, title, heading, sent_at, candidate, "")
    });
    details.clear();
    value = card_value(notification, title, heading, sent_at, &task, &details);
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
    use crate::model::{Notification, Outcome};
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
        assert!(card.outer_title.starts_with("\u{2705} "));
        assert!(card.outer_title.ends_with(
            " \u{786e}\u{8ba4}\u{4efb}\u{52a1}\u{5b8c}\u{6210}\u{901a}\u{77e5}\u{80fd}\u{529b}"
        ));
        let time = card
            .outer_title
            .strip_prefix("\u{2705} ")
            .and_then(|value| value.split_once(' '))
            .map(|(time, _)| time)
            .expect("outer title time");
        assert_eq!(time.len(), 5);
        assert_eq!(time.as_bytes()[2], b':');
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
        assert!(
            card.serialized_content
                .contains("**\u{53d1}\u{9001}\u{65f6}\u{95f4}**")
        );
        assert!(card.serialized_content.contains("**\u{4efb}\u{52a1}**"));
        assert!(card.serialized_content.contains("1 小时 1 分 1 秒"));
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
    fn local_markdown_media_is_rendered_as_a_safe_attachment_note() {
        let mut notification = completion();
        notification.details_markdown = concat!(
            "成片：\n\n",
            "![宣传片](/Users/example/video/demo.mp4)\n\n",
            "![预览](</Users/example/video/preview (final).png>)"
        )
        .to_owned();
        let card = render(&notification);
        let markdown = card.value["body"]["elements"][0]["elements"][0]["content"]
            .as_str()
            .expect("markdown content");

        assert!(markdown.contains("附件：宣传片（请在 Codex 中查看）"));
        assert!(markdown.contains("附件：预览（请在 Codex 中查看）"));
        assert!(!markdown.contains("!["));
        assert!(!markdown.contains("/Users/example"));
    }

    #[test]
    fn utf8_truncation_never_breaks_a_character() {
        assert_eq!(shorten_utf8("\u{4f60}\u{597d}", 4), "\u{4f60}");
    }

    #[test]
    fn interruption_cards_use_the_same_collapsed_layout_with_warning_title() {
        let notification = Notification::interrupted(
            "\u{5bfc}\u{5165}\u{901a}\u{77e5}",
            "\u{5b9e}\u{73b0}\u{76d1}\u{63a7}",
            "**\u{9519}\u{8bef}**\nstream disconnected",
            Some(Duration::from_secs(31)),
            "error-1",
        );
        let card = render(&notification);
        assert_eq!(notification.outcome, Outcome::Interrupted);
        assert!(card.outer_title.starts_with("\u{26a0}\u{fe0f}"));
        assert!(card.serialized_content.contains("**\u{7ed3}\u{679c}**"));
        assert_eq!(card.value["body"]["elements"][0]["expanded"], false);
    }
}

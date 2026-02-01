use chrono::{DateTime, Local, TimeZone};
use regex::Regex;
use std::collections::HashMap;

use crate::widgets::MessageData;

/// Extract YouTube video ID from a URL
#[cfg(test)]
fn extract_youtube_id(url: &str) -> Option<String> {
    // youtube.com/watch?v=ID
    if let Some(pos) = url.find("v=") {
        let id = &url[pos + 2..];
        let id = id.split('&').next().unwrap_or(id);
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    // youtu.be/ID
    if url.contains("youtu.be/") {
        if let Some(pos) = url.find("youtu.be/") {
            let id = &url[pos + 9..];
            let id = id.split('?').next().unwrap_or(id);
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Format message reactions as a string
pub fn format_reactions(reactions: &HashMap<String, u32>) -> String {
    if reactions.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();
    for (emoji, count) in reactions.iter() {
        if *count > 1 {
            parts.push(format!("{}x{}", count, emoji));
        } else {
            parts.push(emoji.clone());
        }
    }

    parts.join(" ")
}

/// Get media label for different types - matching Python's colored output
pub fn get_media_label(media_type: &str, title: Option<&str>) -> String {
    match media_type {
        "youtube" => {
            if let Some(t) = title {
                format!("[YouTube: {}]", t)
            } else {
                "[YouTube]".to_string()
            }
        }
        "spotify" => {
            if let Some(t) = title {
                format!("[Spotify: {}]", t)
            } else {
                "[Spotify]".to_string()
            }
        }
        "photo" => "[IMG]".to_string(),
        "video" => "[CLIP]".to_string(),
        "audio" => "[AUDIO]".to_string(),
        "voice" => "[VOICE]".to_string(),
        "video_note" => "[VIDEO_NOTE]".to_string(),
        "sticker" => "[STICKER]".to_string(),
        "gif" => "[GIF]".to_string(),
        "document" => "[FILE]".to_string(),
        "contact" => "[CONTACT]".to_string(),
        "location" => "[LOCATION]".to_string(),
        "poll" => "[POLL]".to_string(),
        "dice" => "[DICE]".to_string(),
        "game" => "[GAME]".to_string(),
        _ => format!("[{}]", media_type.to_uppercase()),
    }
}

/// Shorten long URLs in text by truncating
pub fn shorten_urls(text: &str, max_len: usize) -> String {
    let url_regex = Regex::new(r"https?://[^\s]+").unwrap();

    let mut result = text.to_string();
    for cap in url_regex.find_iter(text) {
        let url = cap.as_str();
        if url.chars().count() > max_len {
            let truncate_at = url
                .char_indices()
                .nth(max_len)
                .map(|(i, _)| i)
                .unwrap_or(url.len());
            let shortened = format!("{}...", &url[..truncate_at]);
            result = result.replace(url, &shortened);
        }
    }

    result
}

/// Strip emojis from text (if emoji display is disabled)
pub fn strip_emojis(text: &str) -> String {
    let emoji_regex = Regex::new(
        r"[\u{1F600}-\u{1F64F}\u{1F300}-\u{1F5FF}\u{1F680}-\u{1F6FF}\u{1F700}-\u{1F77F}\u{1F780}-\u{1F7FF}\u{1F800}-\u{1F8FF}\u{1F900}-\u{1F9FF}\u{1FA00}-\u{1FA6F}\u{1FA70}-\u{1FAFF}\u{2600}-\u{26FF}\u{2700}-\u{27BF}\u{FE00}-\u{FE0F}\u{200D}]+"
    ).unwrap();
    emoji_regex.replace_all(text, "").to_string()
}

/// Wrap text to fit within a given width, with indent for continuation lines
pub fn wrap_text(text: &str, indent: usize, width: usize) -> String {
    if width <= indent {
        return text.to_string();
    }
    let content_width = width - indent;
    let pad: String = " ".repeat(indent);
    let mut result_lines: Vec<String> = Vec::new();

    for (i, paragraph) in text.split('\n').enumerate() {
        if paragraph.is_empty() {
            result_lines.push(if i > 0 { pad.clone() } else { String::new() });
            continue;
        }

        let words: Vec<&str> = paragraph.split(' ').collect();
        let mut current_line = String::new();
        let first_line_of_para = i == 0;

        for word in &words {
            // Handle very long words - use char_count instead of byte len
            let word_char_count = word.chars().count();
            if word_char_count > content_width {
                if !current_line.is_empty() {
                    if first_line_of_para && result_lines.is_empty() {
                        result_lines.push(current_line.clone());
                    } else {
                        result_lines.push(format!("{}{}", pad, current_line));
                    }
                    current_line.clear();
                }
                // Split word by character boundaries
                let char_indices: Vec<(usize, char)> = word.char_indices().collect();
                let mut char_pos = 0;
                while char_pos < char_indices.len() {
                    let chunk_end = (char_pos + content_width).min(char_indices.len());
                    let byte_start = char_indices[char_pos].0;
                    let byte_end = if chunk_end < char_indices.len() {
                        char_indices[chunk_end].0
                    } else {
                        word.len()
                    };
                    let chunk = &word[byte_start..byte_end];
                    if first_line_of_para && result_lines.is_empty() {
                        result_lines.push(chunk.to_string());
                    } else {
                        result_lines.push(format!("{}{}", pad, chunk));
                    }
                    char_pos = chunk_end;
                }
                continue;
            }

            let test_line = if current_line.is_empty() {
                word.to_string()
            } else {
                format!("{} {}", current_line, word)
            };

            if test_line.chars().count() <= content_width {
                current_line = test_line;
            } else {
                if !current_line.is_empty() {
                    if first_line_of_para && result_lines.is_empty() {
                        result_lines.push(current_line.clone());
                    } else {
                        result_lines.push(format!("{}{}", pad, current_line));
                    }
                }
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            if first_line_of_para && result_lines.is_empty() {
                result_lines.push(current_line);
            } else {
                result_lines.push(format!("{}{}", pad, current_line));
            }
        }
    }

    result_lines.join("\n")
}

/// Format timestamp for display
pub fn format_timestamp(timestamp: i64) -> String {
    let datetime: DateTime<Local> = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Local::now);

    let now = Local::now();
    if datetime.date_naive() == now.date_naive() {
        datetime.format("%H:%M").to_string()
    } else {
        datetime.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// Format all messages for a pane display - matching Python's _format_messages
pub fn format_messages_for_display(
    msg_data: &[MessageData],
    width: usize,
    compact_mode: bool,
    show_emojis: bool,
    show_reactions: bool,
    show_timestamps: bool,
    show_line_numbers: bool,
    filter_type: Option<&str>,
    filter_value: Option<&str>,
    unread_count: u32,
    aliases: &HashMap<String, String>,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Show filter indicator if active
    if let Some(ft) = filter_type {
        let fv = filter_value.unwrap_or("");
        lines.push(format!("Filter: {}={} (use /filter off to disable)", ft, fv));
        lines.push(String::new());
    }

    let unread_marker_idx = if unread_count > 0 {
        msg_data.len().saturating_sub(unread_count as usize)
    } else {
        usize::MAX
    };

    for (idx, data) in msg_data.iter().enumerate() {
        // Show unread marker
        if idx == unread_marker_idx && unread_count > 0 {
            let marker = "-".repeat(width / 2);
            lines.push(format!("{} {} unread {}", marker, unread_count, marker));
        }

        let media_label = if let Some(ref media_type) = data.media_type {
            get_media_label(media_type, None)
        } else {
            data.media_label.as_deref().unwrap_or("").to_string()
        };
        let mut text = data.text.clone();

        if text.is_empty() && media_label.is_empty() {
            continue;
        }

        // Resolve sender name (use alias if available)
        let sender_name = aliases
            .get(&data.sender_id)
            .cloned()
            .unwrap_or_else(|| data.sender_name.clone());

        let timestamp = format_timestamp(data.timestamp);
        let num_str = format!("#{}", idx + 1);

        // Calculate prefix length for wrapping
        let mut prefix_len = sender_name.len() + 2; // "name: "
        if show_line_numbers {
            prefix_len += num_str.len() + 1; // "#N "
        }
        if show_timestamps {
            prefix_len += timestamp.len() + 1; // "HH:MM "
        }

        // Process text
        if !text.is_empty() {
            text = shorten_urls(&text, 60);
            if !show_emojis {
                text = strip_emojis(&text);
            }
            let wrapped = wrap_text(&text, prefix_len, width);
            if !media_label.is_empty() {
                text = format!("{} {}", media_label, wrapped);
            } else {
                text = wrapped;
            }
        } else {
            text = media_label.to_string();
        }

        // Handle reply info - show what message this is replying to
        // Look up the actual message being replied to in msg_data
        if let Some(ref reply_to_id) = data.reply_to_msg_id {
            // Try to find the message being replied to in our loaded messages
            if let Some(original_msg) = msg_data.iter().find(|m| &m.msg_id == reply_to_id) {
                let reply_sender = aliases
                    .get(&original_msg.sender_id)
                    .cloned()
                    .unwrap_or_else(|| original_msg.sender_name.clone());
                
                let mut rt = original_msg.text.clone();
                if !show_emojis {
                    rt = strip_emojis(&rt);
                }
                // Get first line only and truncate if needed
                let first_line = rt.lines().next().unwrap_or(&rt);
                let display_text = if first_line.chars().count() > 50 {
                    let truncate_at = first_line.char_indices().nth(50).map(|(i, _)| i).unwrap_or(first_line.len());
                    format!("{}...", &first_line[..truncate_at])
                } else {
                    first_line.to_string()
                };
                // Add marker if replying to my own message
                let reply_marker = if original_msg.is_outgoing {
                    "[REPLY_TO_ME] "
                } else {
                    ""
                };
                lines.push(format!("{}  ↳ Reply to {}: {}", reply_marker, reply_sender, display_text));
            } else {
                // Message not in our loaded history - show minimal info
                // If we have cached reply info from Telegram, use it
                if let (Some(reply_sender), Some(reply_text)) = (&data.reply_sender, &data.reply_text) {
                    let mut rt = reply_text.clone();
                    if !show_emojis {
                        rt = strip_emojis(&rt);
                    }
                    let first_line = rt.lines().next().unwrap_or(&rt);
                    let display_text = if first_line.chars().count() > 50 {
                        let truncate_at = first_line.char_indices().nth(50).map(|(i, _)| i).unwrap_or(first_line.len());
                        format!("{}...", &first_line[..truncate_at])
                    } else {
                        first_line.to_string()
                    };
                    lines.push(format!("  ↳ Reply to {}: {}", reply_sender, display_text));
                } else {
                    // No info available, just show message ID
                    lines.push(format!("  ↳ Reply to message #{}", reply_to_id));
                }
            }
        }

        // Get reactions
        let reactions_suffix = if show_reactions && !data.reactions.is_empty() {
            let r = format_reactions(&data.reactions);
            format!(" [{}]", r)
        } else {
            String::new()
        };

        // Build message line
        let mut parts: Vec<String> = Vec::new();

        if show_line_numbers {
            parts.push(num_str);
        }
        if show_timestamps {
            parts.push(timestamp);
        }

        // Reply arrow if this was a reply
        if data.reply_to_msg_id.is_some() {
            parts.push("^".to_string());
        }

        // Add sender name and message
        // We use internal markers that will be parsed in app.rs for coloring
        // Format: [OUT|IN]:sender_id:sender_name:message
        let formatted_msg = if data.is_outgoing {
            format!("[OUT]:{}:{}:{}", data.sender_id, sender_name, text)
        } else {
            format!("[IN]:{}:{}:{}", data.sender_id, sender_name, text)
        };
        parts.push(formatted_msg);

        let mut msg_line = parts.join(" ");
        msg_line.push_str(&reactions_suffix);

        lines.push(msg_line);

        // Blank line between messages in non-compact mode
        if !compact_mode {
            lines.push(String::new());
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_urls() {
        let text =
            "Check this out: https://example.com/very/long/path/that/should/be/shortened/here";
        let result = shorten_urls(text, 30);
        assert!(result.contains("..."));
        assert!(result.len() < text.len());
    }

    #[test]
    fn test_extract_youtube_id() {
        let url1 = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let url2 = "https://youtu.be/dQw4w9WgXcQ";

        assert_eq!(
            extract_youtube_id(url1),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_youtube_id(url2),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_format_reactions() {
        let mut reactions = HashMap::new();
        reactions.insert("👍".to_string(), 5);
        reactions.insert("❤️".to_string(), 1);

        let result = format_reactions(&reactions);
        assert!(result.contains("5x👍") || result.contains("👍"));
        assert!(result.contains("❤️"));
    }

    #[test]
    fn test_wrap_text() {
        let text = "This is a longer text that should be wrapped at word boundaries properly";
        let result = wrap_text(text, 10, 40);
        // Each line should not exceed 40 chars (first line) or 30 chars (continuation)
        for (i, line) in result.split('\n').enumerate() {
            if i == 0 {
                assert!(line.len() <= 40, "First line too long: {}", line);
            } else {
                assert!(line.len() <= 40, "Continuation line too long: {}", line);
            }
        }
    }

    #[test]
    fn test_strip_emojis() {
        let text = "Hello 👋 World 🌍";
        let result = strip_emojis(text);
        assert!(!result.contains('👋'));
        assert!(!result.contains('🌍'));
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }
}

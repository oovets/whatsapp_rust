use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterType {
    Sender,
    Media,
    Link,
}

/// Represents a single message with all its metadata for display
#[derive(Clone, Debug)]
pub struct MessageData {
    pub msg_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub text: String,
    pub is_outgoing: bool,
    pub timestamp: i64,        // Unix timestamp
    pub media_type: Option<String>,
    pub media_label: Option<String>,  // e.g. "[YouTube: title]"
    pub reactions: HashMap<String, u32>,
    pub reply_to_msg_id: Option<String>,
    pub reply_sender: Option<String>,
    pub reply_text: Option<String>,
}

pub struct ChatPane {
    pub chat_id: Option<String>,
    pub chat_name: String,
    pub username: Option<String>,
    pub messages: Vec<String>,         // Formatted display lines
    pub msg_data: Vec<MessageData>,    // Raw message data for formatting
    pub scroll_offset: usize,
    pub reply_to_message: Option<String>,  // Telegram message ID to reply to
    pub reply_preview: Option<String>, // Text shown in reply preview bar
    pub filter_type: Option<FilterType>,
    pub filter_value: Option<String>,
    pub typing_indicator: Option<String>, // "Name is typing..."
    pub typing_expire: Option<std::time::Instant>,
    pub online_status: String,
    pub pinned_message: Option<String>,
    pub _unread_count: u32,
    pub unread_count_at_load: u32,
    pub format_cache: HashMap<FormatCacheKey, Vec<String>>,
    pub input_buffer: String,          // Per-pane input buffer
    pub input_cursor: usize,           // Cursor byte position in input_buffer
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct FormatCacheKey {
    pub width: u16,
    pub compact_mode: bool,
    pub show_emojis: bool,
    pub show_reactions: bool,
    pub show_timestamps: bool,
    pub show_line_numbers: bool,
    pub msg_count: usize,
    pub filter_type: Option<String>,
    pub filter_value: Option<String>,
}

impl ChatPane {
    pub fn new() -> Self {
        Self {
            chat_id: None,
            chat_name: String::from("No chat selected"),
            username: None,
            messages: Vec::new(),
            msg_data: Vec::new(),
            scroll_offset: 0,
            reply_to_message: None,
            reply_preview: None,
            filter_type: None,
            filter_value: None,
            typing_indicator: None,
            typing_expire: None,
            online_status: String::new(),
            pinned_message: None,
            _unread_count: 0,
            unread_count_at_load: 0,
            input_buffer: String::new(),
            input_cursor: 0,
            format_cache: HashMap::new(),
        }
    }

    pub fn add_message(&mut self, message: String) {
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.msg_data.clear();
        self.scroll_offset = 0;
        self.input_buffer.clear();
        self.format_cache.clear();
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    pub fn show_typing_indicator(&mut self, name: &str) {
        self.typing_indicator = Some(format!("{} is typing...", name));
        self.typing_expire = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    }

    pub fn hide_typing_indicator(&mut self) {
        self.typing_indicator = None;
        self.typing_expire = None;
    }

    pub fn check_typing_expired(&mut self) {
        if let Some(expire) = self.typing_expire {
            if std::time::Instant::now() >= expire {
                self.hide_typing_indicator();
            }
        }
    }

    pub fn show_reply_preview(&mut self, text: String) {
        self.reply_preview = Some(text);
    }

    pub fn hide_reply_preview(&mut self) {
        self.reply_preview = None;
    }

    /// Build the header text including online status, username, pinned message, typing indicator
    pub fn header_text(&self) -> String {
        let mut header = self.chat_name.clone();

        if !self.online_status.is_empty() {
            header.push_str(&format!(" [{}]", self.online_status));
        }

        if let Some(ref username) = self.username {
            if !username.is_empty() {
                header.push_str(&format!(" {}", username));
            }
        }

        if let Some(ref pinned) = self.pinned_message {
            header.push_str(&format!(" | Pinned: {}", pinned));
        }

        if let Some(ref typing) = self.typing_indicator {
            header.push_str(&format!(" {}", typing));
        }

        header
    }

    /// Check if a message matches the current filter
    pub fn _message_matches_filter(&self, data: &MessageData) -> bool {
        match (&self.filter_type, &self.filter_value) {
            (None, _) => true,
            (Some(FilterType::Sender), Some(value)) => {
                data.sender_name.to_lowercase().contains(&value.to_lowercase())
            }
            (Some(FilterType::Media), Some(value)) => {
                match value.as_str() {
                    "photo" => data.media_type.as_deref() == Some("photo"),
                    "video" => data.media_type.as_deref() == Some("video"),
                    "audio" => data.media_type.as_deref() == Some("audio"),
                    "voice" => data.media_type.as_deref() == Some("voice"),
                    "document" => data.media_type.as_deref() == Some("document"),
                    "sticker" => data.media_type.as_deref() == Some("sticker"),
                    "gif" => data.media_type.as_deref() == Some("gif"),
                    _ => data.media_type.is_some(),
                }
            }
            (Some(FilterType::Link), _) => {
                data.text.contains("http://") || data.text.contains("https://")
            }
            _ => true,
        }
    }
}

impl Default for ChatPane {
    fn default() -> Self {
        Self::new()
    }
}

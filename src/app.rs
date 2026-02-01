use anyhow::Result;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};

use crate::commands::CommandHandler;
use crate::config::Config;
use crate::formatting::format_messages_for_display;
use crate::persistence::{Aliases, AppState, LayoutData, PaneState};
use crate::split_view::{PaneNode, SplitDirection};
use crate::whatsapp::WhatsAppClient;
use crate::utils::{send_desktop_notification, try_autocomplete};
use crate::widgets::ChatPane;

pub struct App {
    pub config: Config,
    pub whatsapp: WhatsAppClient,
    pub my_user_jid: String,  // Current user's ID for determining outgoing messages
    pub chats: Vec<ChatInfo>,
    pub selected_chat_idx: usize,
    pub panes: Vec<ChatPane>,
    pub focused_pane_idx: usize,
    pub pane_tree: PaneNode,
    pub input_history: Vec<String>,
    pub history_idx: Option<usize>,
    pub history_temp: String, // Save current input when browsing history
    pub aliases: Aliases,
    pub focus_on_chat_list: bool,
    pub status_message: Option<String>, // Notification bar at bottom
    pub status_expire: Option<std::time::Instant>,
    pub pane_areas: std::collections::HashMap<usize, Rect>, // Track pane screen positions
    pub chat_list_area: Option<Rect>, // Track chat list area for mouse clicks
    pub needs_redraw: bool,

    // Settings
    pub show_reactions: bool,
    pub show_notifications: bool,
    pub compact_mode: bool,
    pub show_emojis: bool,
    pub show_line_numbers: bool,
    pub show_timestamps: bool,
    pub show_chat_list: bool,
    pub show_user_colors: bool,
    pub show_borders: bool,
    pub user_colors: std::collections::HashMap<String, Color>, // Map sender_id to color for group chats
}

#[derive(Clone)]
pub struct ChatInfo {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub unread: u32,
    pub _is_channel: bool,
    pub is_group: bool,
}

impl App {
    pub async fn new() -> Result<Self> {
        let config = Config::load()?;
        let whatsapp = WhatsAppClient::new(&config).await?;
        let my_user_jid = whatsapp.get_me().await?;
        let app_state = AppState::load(&config).unwrap_or_else(|_| AppState {
            settings: crate::persistence::AppSettings::default(),
            aliases: Aliases::default(),
            layout: LayoutData::default(),
        });

        // Load initial chats
        let chats = whatsapp.get_dialogs().await.unwrap_or_else(|_| Vec::new());

        // Load pane tree first to know which panes we need
        let (pane_tree, required_indices) = if let Some(saved_tree) = app_state.layout.pane_tree {
            let indices = saved_tree.get_pane_indices();
            (saved_tree, indices)
        } else {
            // No saved tree, create default based on number of saved panes
            let tree = if !app_state.layout.panes.is_empty() && app_state.layout.panes.len() > 1 {
                let mut t = PaneNode::new_single(0);
                for i in 1..app_state.layout.panes.len() {
                    t.split(SplitDirection::Vertical, i);
                }
                t
            } else {
                PaneNode::new_single(0)
            };
            let indices = tree.get_pane_indices();
            (tree, indices)
        };
        
        // Determine how many panes we need (max of what tree references and what's saved)
        let max_required_idx = required_indices.iter().max().copied().unwrap_or(0);
        let total_panes_needed = (max_required_idx + 1).max(app_state.layout.panes.len()).max(1);
        
        // Load panes - create panes for all indices up to total_panes_needed
        let mut panes: Vec<ChatPane> = Vec::new();
        for i in 0..total_panes_needed {
            if let Some(ps) = app_state.layout.panes.get(i) {
                // Load saved pane state
                let mut pane = ChatPane::new();
                pane.chat_id = ps.chat_id.clone();
                pane.chat_name = ps.chat_name.clone();
                pane.scroll_offset = ps.scroll_offset;
                // Load filter settings
                if let Some(ref filter_type_str) = ps.filter_type {
                    pane.filter_type = Some(match filter_type_str.as_str() {
                        "sender" => crate::widgets::FilterType::Sender,
                        "media" => crate::widgets::FilterType::Media,
                        "link" => crate::widgets::FilterType::Link,
                        _ => {
                            panes.push(pane);
                            continue;
                        }
                    });
                }
                pane.filter_value = ps.filter_value.clone();
                panes.push(pane);
            } else {
                // Create empty pane for missing index
                panes.push(ChatPane::new());
            }
        }
        
        let focused_pane_idx = if app_state.layout.focused_pane < panes.len() {
            app_state.layout.focused_pane
        } else {
            0
        };

        let mut app = Self {
            config,
            whatsapp,
            my_user_jid,
            chats,
            selected_chat_idx: 0,
            panes,
            focused_pane_idx,
            pane_tree,
            input_history: Vec::new(),
            history_idx: None,
            history_temp: String::new(),
            aliases: app_state.aliases,
            focus_on_chat_list: true,
            status_message: None,
            status_expire: None,
            chat_list_area: None,
            pane_areas: std::collections::HashMap::new(),
            needs_redraw: true,
            show_reactions: app_state.settings.show_reactions,
            show_notifications: app_state.settings.show_notifications,
            compact_mode: app_state.settings.compact_mode,
            show_emojis: app_state.settings.show_emojis,
            show_line_numbers: app_state.settings.show_line_numbers,
            show_timestamps: app_state.settings.show_timestamps,
            show_chat_list: app_state.settings.show_chat_list,
            show_user_colors: app_state.settings.show_user_colors,
            show_borders: app_state.settings.show_borders,
            user_colors: std::collections::HashMap::new(),
        };

        // Load messages for all panes that have a saved chat_id
        // This is what we had before - it works better
        app.load_saved_chat_messages().await?;

        Ok(app)
    }

    /// Refresh messages for a specific pane
    async fn refresh_pane_messages(&mut self, pane_idx: usize) -> Result<()> {
        if let Some(pane) = self.panes.get(pane_idx) {
            if let Some(ref chat_id) = pane.chat_id {
                match self.whatsapp.get_messages(&chat_id, 50).await {
                    Ok(raw_messages) => {
                        if !raw_messages.is_empty() {
                            let msg_data: Vec<crate::widgets::MessageData> = raw_messages
                                .iter()
                                .map(|(msg_id, sender_id, sender_name, text, reply_to_id, media_type, reactions, timestamp)| {
                                    let reply_to_msg_id = reply_to_id.clone();
                                    
                                    crate::widgets::MessageData {
                                        msg_id: msg_id.clone(),
                                        sender_id: sender_id.clone(),
                                        sender_name: sender_name.clone(),
                                        text: text.clone(),
                                        is_outgoing: sender_id == &self.my_user_jid,
                                        timestamp: *timestamp,
                                        media_type: media_type.clone(),
                                        media_label: None,
                                        reactions: reactions.clone(),
                                        reply_to_msg_id,
                                        reply_sender: None,
                                        reply_text: None,
                                    }
                                })
                                .collect();
                            
                            if let Some(pane) = self.panes.get_mut(pane_idx) {
                                pane.msg_data = msg_data;
                                pane.format_cache.clear(); // Clear cache so messages are re-rendered
                            }
                        }
                    }
                    Err(_) => {
                        // Silently fail - messages will update via polling
                    }
                }
            }
        }
        Ok(())
    }

    /// Load messages for all panes that have a saved chat_id
    async fn load_saved_chat_messages(&mut self) -> Result<()> {
        for (_idx, pane) in self.panes.iter_mut().enumerate() {
            if let Some(ref chat_id) = pane.chat_id {
                // Try to load messages for this chat
                match self.whatsapp.get_messages(&chat_id, 50).await {
                    Ok(raw_messages) => {
                        if !raw_messages.is_empty() {
                            let msg_data: Vec<crate::widgets::MessageData> = raw_messages
                                .iter()
                                .map(|(msg_id, sender_id, sender_name, text, reply_to_id, media_type, reactions, timestamp)| {
                                    let reply_to_msg_id = reply_to_id.clone();
                                    
                                    crate::widgets::MessageData {
                                        msg_id: msg_id.clone(),
                                        sender_id: sender_id.clone(),
                                        sender_name: sender_name.clone(),
                                        text: text.clone(),
                                        is_outgoing: sender_id == &self.my_user_jid,
                                        timestamp: *timestamp,
                                        media_type: media_type.clone(),
                                        media_label: None,
                                        reactions: reactions.clone(),
                                        reply_to_msg_id,
                                        reply_sender: None,
                                        reply_text: None,
                                    }
                                })
                                .collect();
                            
                            pane.msg_data = msg_data;
                            pane.format_cache.clear(); // Clear cache so messages are re-rendered
                            
                            // Also try to find username from chats list
                            if let Some(chat_info) = self.chats.iter().find(|c| &c.id == chat_id) {
                                pane.username = chat_info.username.clone();
                            }
                        }
                    }
                    Err(_) => {
                        // Silently continue loading other panes
                    }
                }
            }
        }
        Ok(())
    }

    pub fn draw(&mut self, f: &mut Frame) {
        // Update cursor blink timer for blinking cursor
        // This will be checked in draw_chat_pane_impl
        // Check typing indicators for expiry
        for pane in &mut self.panes {
            pane.check_typing_expired();
        }
        // Check status message expiry
        if let Some(expire) = self.status_expire {
            if std::time::Instant::now() >= expire {
                self.status_message = None;
                self.status_expire = None;
            }
        }

        let has_status = self.status_message.is_some();
        let main_constraints = if has_status {
            vec![Constraint::Min(0), Constraint::Length(1)]
        } else {
            vec![Constraint::Min(0)]
        };

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints(main_constraints)
            .split(f.area());

        let (chat_area, pane_area) = if self.show_chat_list {
            let total_width = outer[0].width;
            let base_chat_width = total_width.saturating_mul(20) / 100;
            let chat_width = base_chat_width.saturating_sub(5).max(10);
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(chat_width), Constraint::Min(0)])
                .split(outer[0]);
            (Some(chunks[0]), chunks[1])
        } else {
            (None, outer[0])
        };

        // Store chat list area for mouse handling
        if let Some(area) = chat_area {
            self.chat_list_area = Some(area);
            self.draw_chat_list(f, area);
        } else {
            self.chat_list_area = None;
        }

        let colors = [
            Color::Cyan, Color::Yellow, Color::Magenta, Color::Blue,
            Color::Red, Color::Green, Color::White, Color::LightCyan,
            Color::LightYellow, Color::LightMagenta, Color::LightBlue,
            Color::LightRed, Color::LightGreen, Color::DarkGray,
            Color::Rgb(192, 192, 192),
            Color::Rgb(255, 165, 0),
            Color::Rgb(255, 192, 203),
            Color::Rgb(128, 0, 128),
            Color::Rgb(0, 255, 255),
            Color::Rgb(255, 20, 147)
        ];
        
        let mut senders_to_color: Vec<String> = Vec::new();
        for pane in &self.panes {
            if let Some(ref chat_id) = pane.chat_id {
                let is_group_chat = self.chats.iter().any(|c| &c.id == chat_id && c.is_group);
                if is_group_chat && !pane.msg_data.is_empty() {
                    for msg in &pane.msg_data {
                        if !self.user_colors.contains_key(&msg.sender_id) && !senders_to_color.contains(&msg.sender_id) {
                            senders_to_color.push(msg.sender_id.clone());
                        }
                    }
                }
            }
        }
        
        for sender_id in &senders_to_color {
            // Hash the string to get a u64
            let mut hash: u64 = 0;
            for byte in sender_id.bytes() {
                hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
            }
            hash = hash.wrapping_mul(2654435761);
            hash = hash ^ (hash >> 16);
            hash = hash.wrapping_mul(0x85ebca6b);
            hash = hash ^ (hash >> 13);
            hash = hash.wrapping_mul(0xc2b2ae35);
            hash = hash ^ (hash >> 16);
            
            let color_idx = (hash as usize) % colors.len();
            let color = colors[color_idx];
            self.user_colors.insert(sender_id.clone(), color);
        }

        let render_fn = |f: &mut Frame, area: Rect, pane: &ChatPane, is_focused: bool| {
            self.draw_chat_pane_impl(f, area, pane, is_focused);
        };

        let mut pane_areas = std::collections::HashMap::new();
        self.pane_tree
            .render(f, pane_area, &self.panes, self.focused_pane_idx, &render_fn, &mut pane_areas);
        self.pane_areas = pane_areas;

        // Draw status bar
        if has_status {
            if let Some(ref msg) = self.status_message {
                let status = Paragraph::new(msg.as_str())
                    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                f.render_widget(status, outer[1]);
            }
        }
    }

    fn draw_chat_list(&self, f: &mut Frame, area: Rect) {
        // Find which chat is open in the focused pane
        let active_chat_id = self.panes
            .get(self.focused_pane_idx)
            .and_then(|p| p.chat_id.clone());
        
        let max_width = area.width.saturating_sub(6).max(1) as usize;
        let (unread_group, active_group, other_group) = self.chat_list_groups();

        let build_item = |chat: &ChatInfo| -> ListItem {
            // Highlight if this chat is open in the focused pane
            let base_style = if Some(chat.id.clone()) == active_chat_id {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let unread_marker = if chat.unread > 0 { "▶ " } else { "" };
            let unread_count = if chat.unread > 0 {
                format!("({}) ", chat.unread)
            } else {
                String::new()
            };

            let mut name_part = chat.name.clone();
            if let Some(ref username) = chat.username {
                if !username.is_empty() {
                    name_part.push_str(&format!(" {}", username));
                }
            }

            let mut spans = Vec::new();
            if !unread_marker.is_empty() {
                spans.push(ratatui::text::Span::styled(
                    unread_marker.to_string(),
                    Style::default().fg(Color::Red),
                ));
            }
            if !unread_count.is_empty() {
                spans.push(ratatui::text::Span::styled(unread_count, base_style));
            }
            spans.push(ratatui::text::Span::styled(name_part, base_style));

            // Truncate spans to fit
            let total_chars: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let truncated = total_chars > max_width && max_width > 0;
            let mut remaining = if truncated { max_width.saturating_sub(1) } else { max_width };
            let mut out_spans: Vec<ratatui::text::Span> = Vec::new();

            for span in spans.into_iter() {
                if remaining == 0 {
                    break;
                }
                let span_len = span.content.chars().count();
                if span_len <= remaining {
                    remaining = remaining.saturating_sub(span_len);
                    out_spans.push(span);
                } else {
                    let clipped: String = span.content.chars().take(remaining).collect();
                    out_spans.push(ratatui::text::Span::styled(clipped, span.style));
                    break;
                }
            }

            if truncated {
                out_spans.push(ratatui::text::Span::styled("…".to_string(), base_style));
            }

            ListItem::new(ratatui::text::Line::from(out_spans))
        };

        let header_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD);
        let mut items: Vec<ListItem> = Vec::new();

        if !unread_group.is_empty() {
            items.push(ListItem::new("Unread").style(header_style));
            for chat_idx in unread_group.iter() {
                items.push(build_item(&self.chats[*chat_idx]));
            }
        }

        if !active_group.is_empty() {
            items.push(ListItem::new("Active").style(header_style));
            for chat_idx in active_group.iter() {
                items.push(build_item(&self.chats[*chat_idx]));
            }
        }

        if !other_group.is_empty() {
            items.push(ListItem::new("Other").style(header_style));
            for chat_idx in other_group.iter() {
                items.push(build_item(&self.chats[*chat_idx]));
            }
        }

        let border_style = if self.focus_on_chat_list {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };

        let list_block = if self.show_borders {
            Block::default()
                .borders(Borders::ALL)
                .title("Chats")
                .border_style(border_style)
        } else {
            Block::default()
        };
        let list = List::new(items)
            .block(list_block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        f.render_widget(list, area);
    }

    fn draw_chat_pane_impl(
        &self,
        f: &mut Frame,
        area: Rect,
        pane: &ChatPane,
        is_focused: bool,
    ) {
        let has_reply_preview = pane.reply_preview.is_some();

        // Calculate input height dynamically based on text width
        let border_overhead = if self.show_borders { 2 } else { 0 };
        let header_height = if self.show_borders { 3 } else { 1 };
        let inner_width = area.width.saturating_sub(if self.show_borders { 2 } else { 0 }).max(1) as usize;
        let text_lines = if is_focused && inner_width > 0 {
            let buf = &pane.input_buffer;
            let mut lines: u16 = 0;
            for line in buf.split('\n') {
                // Each logical line wraps based on its length (+ cursor on last segment)
                let len = line.len();
                lines += ((len as f64) / (inner_width as f64)).ceil().max(1.0) as u16;
            }
            // Account for cursor on the last line
            let last_line_len = buf.rsplit('\n').next().map_or(buf.len(), |l| l.len()) + 1;
            if last_line_len > inner_width {
                let without_cursor = buf.rsplit('\n').next().map_or(buf.len(), |l| l.len());
                let lines_without = ((without_cursor as f64) / (inner_width as f64)).ceil().max(1.0) as u16;
                let lines_with = ((last_line_len as f64) / (inner_width as f64)).ceil().max(1.0) as u16;
                lines += lines_with - lines_without;
            }
            lines.max(1)
        } else {
            1
        };
        let input_height = text_lines + border_overhead + 1; // +1 for spacing below

        let constraints = if has_reply_preview {
            vec![
                Constraint::Length(header_height),
                Constraint::Min(0),     // messages
                Constraint::Length(1),  // reply preview
                Constraint::Length(input_height),
            ]
        } else {
            vec![
                Constraint::Length(header_height),
                Constraint::Min(0),     // messages
                Constraint::Length(input_height),
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        // Header with online status, username, pinned, typing
        let header_style = if is_focused {
            if self.focus_on_chat_list {
                // Show which pane will receive the next chat from list
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                // Active input pane
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            Style::default().fg(Color::Cyan)
        };

        let mut header_text = String::new();
        if is_focused && self.focus_on_chat_list {
            header_text.push_str("[TARGET] ");
        }
        header_text.push_str(&pane.header_text());
        
        let header_block = if self.show_borders {
            Block::default().borders(Borders::ALL)
        } else {
            Block::default()
        };
        let header = Paragraph::new(header_text)
            .block(header_block)
            .style(header_style);
        f.render_widget(header, chunks[0]);

        // Messages - use rich formatted data if available, otherwise plain messages
        let message_width = chunks[1].width.saturating_sub(4) as usize;
        
        // Check if this is a group chat
        let is_group_chat = if let Some(ref chat_id) = pane.chat_id {
            self.chats.iter().any(|c| &c.id == chat_id && c.is_group)
        } else {
            false
        };
        
        let display_lines = if !pane.msg_data.is_empty() {
            // Use msg_data for rich formatting
            let filter_type = pane
                .filter_type
                .as_ref()
                .map(|ft| match ft {
                    crate::widgets::FilterType::Sender => "sender",
                    crate::widgets::FilterType::Media => "media",
                    crate::widgets::FilterType::Link => "link",
                });
            let filter_value = pane.filter_value.as_deref();

            let mut lines = format_messages_for_display(
                &pane.msg_data,
                message_width,
                self.compact_mode,
                self.show_emojis,
                self.show_reactions,
                self.show_timestamps,
                self.show_line_numbers,
                filter_type,
                filter_value,
                pane.unread_count_at_load,
                &self.aliases.map,
            );
            
            // Append any status messages from pane.messages (like "✓ Replied to #5")
            if !pane.messages.is_empty() {
                lines.push(String::new()); // Separator
                lines.extend(pane.messages.clone());
            }
            lines
        } else {
            // Fallback to plain messages (for status messages, etc.)
            pane.messages.clone()
        };

        let wrap_plain_text = |text: &str, max_width: usize| -> Vec<String> {
            if max_width == 0 || text.len() <= max_width {
                return vec![text.to_string()];
            }

            let mut lines = Vec::new();
            let mut current_line = String::new();

            for word in text.split_whitespace() {
                if current_line.len() + word.len() + 1 > max_width {
                    if !current_line.is_empty() {
                        lines.push(current_line.clone());
                        current_line.clear();
                    }
                    if word.chars().count() > max_width {
                        let split_at = word
                            .char_indices()
                            .nth(max_width)
                            .map(|(i, _)| i)
                            .unwrap_or(word.len());
                        lines.push(word[..split_at].to_string());
                        current_line = word[split_at..].to_string();
                    } else {
                        current_line = word.to_string();
                    }
                } else {
                    if !current_line.is_empty() {
                        current_line.push(' ');
                    }
                    current_line.push_str(word);
                }
            }
            if !current_line.is_empty() {
                lines.push(current_line);
            }
            lines
        };

        let wrap_message_with_indent =
            |prefix: &str, sender_name: &str, message_text: &str, max_width: usize| -> Vec<String> {
                let header = format!("{}{}: ", prefix, sender_name);
                let indent_len = header.chars().count();

                if max_width == 0 {
                    return vec![format!("{}{}", header, message_text)];
                }

                if indent_len >= max_width {
                    return wrap_plain_text(&format!("{}{}", header, message_text), max_width);
                }

                let first_width = max_width.saturating_sub(indent_len);
                let wrapped = wrap_plain_text(message_text, first_width);
                if wrapped.is_empty() {
                    return vec![header.trim_end().to_string()];
                }

                let indent = " ".repeat(indent_len);
                let mut lines = Vec::with_capacity(wrapped.len());
                lines.push(format!("{}{}", header, wrapped[0]));
                for line in wrapped.iter().skip(1) {
                    lines.push(format!("{}{}", indent, line));
                }
                lines
            };

        let style_name_in_line = |line: &str, sender_name: &str, name_style: Style| -> Line {
            if sender_name.is_empty() {
                return Line::from(line.to_string());
            }

            let name_token = format!("{}:", sender_name);
            if let Some(start) = line.find(&name_token) {
                let name_end = start + sender_name.len();
                let before = &line[..start];
                let name = &line[start..name_end];
                let after = &line[name_end..];
                Line::from(vec![
                    ratatui::text::Span::raw(before.to_string()),
                    ratatui::text::Span::styled(name.to_string(), name_style),
                    ratatui::text::Span::raw(after.to_string()),
                ])
            } else {
                Line::from(line.to_string())
            }
        };

        let message_lines: Vec<Line> = display_lines
            .iter()
            .flat_map(|msg| {
                if msg.is_empty() {
                    return vec![Line::from("")];
                }

                if msg.starts_with("[REPLY_TO_ME]") {
                    let clean_msg = msg.replace("[REPLY_TO_ME]", "").trim_start().to_string();
                    return wrap_plain_text(&clean_msg, message_width)
                        .into_iter()
                        .map(|line| {
                            Line::from(line).style(
                                Style::default()
                                    .fg(Color::Red)
                                    .add_modifier(Modifier::ITALIC),
                            )
                        })
                        .collect();
                }

                if msg.starts_with("  ↳ Reply to") {
                    return wrap_plain_text(msg, message_width)
                        .into_iter()
                        .map(|line| {
                            Line::from(line).style(
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            )
                        })
                        .collect();
                }

                if msg.contains("[OUT]:") || msg.contains("[IN]:") {
                    let is_outgoing = msg.contains("[OUT]:");
                    let marker = if is_outgoing { "[OUT]:" } else { "[IN]:" };
                    let marker_len = marker.len();
                    if let Some(marker_pos) = msg.find(marker) {
                        let prefix = &msg[..marker_pos];
                        let after_marker = &msg[marker_pos + marker_len..];

                        if let Some(first_colon) = after_marker.find(':') {
                            let sender_id_str = &after_marker[..first_colon];
                            let after_id = &after_marker[first_colon + 1..];
                            if let Some(second_colon) = after_id.find(':') {
                                let sender_name = &after_id[..second_colon];
                                let message_text = &after_id[second_colon + 1..];

                                {
                                    let sender_id = sender_id_str;
                                    let base_color = if is_outgoing {
                                        Color::Green
                                    } else {
                                        Color::Cyan
                                    };
                                    let color = if is_group_chat {
                                        self.user_colors.get(sender_id).copied().unwrap_or(base_color)
                                    } else {
                                        base_color
                                    };
                                    let lines = wrap_message_with_indent(
                                        prefix,
                                        sender_name,
                                        message_text,
                                        message_width,
                                    );
                                    if self.show_user_colors {
                                        return lines
                                            .into_iter()
                                            .enumerate()
                                            .map(|(idx, line)| {
                                                if idx == 0 {
                                                    style_name_in_line(
                                                        &line,
                                                        sender_name,
                                                        Style::default().fg(color),
                                                    )
                                                } else {
                                                    Line::from(line)
                                                }
                                            })
                                            .collect();
                                    }
                                    return lines.into_iter().map(Line::from).collect();
                                }
                            }
                        }
                    }
                }

                wrap_plain_text(msg, message_width)
                    .into_iter()
                    .map(Line::from)
                    .collect()
            })
            .collect();

        let border_lines = if self.show_borders { 2 } else { 1 }; // 1 for spacing above input in borderless
        let available_height = chunks[1].height.saturating_sub(border_lines) as usize;
        let total_lines = message_lines.len();
        
        let actual_scroll = if pane.scroll_offset == 0 && total_lines > available_height {
            total_lines.saturating_sub(available_height)
        } else {
            pane.scroll_offset
        };

        let messages_block = if self.show_borders {
            Block::default().borders(Borders::ALL).title("Messages")
        } else {
            Block::default().padding(Padding::left(2))
        };
        let messages = Paragraph::new(message_lines)
            .block(messages_block)
            .scroll((actual_scroll as u16, 0));
        f.render_widget(messages, chunks[1]);

        if has_reply_preview {
            if let Some(ref preview) = pane.reply_preview {
                let reply_bar = Paragraph::new(preview.as_str())
                    .style(Style::default().fg(Color::Magenta).add_modifier(Modifier::ITALIC));
                f.render_widget(reply_bar, chunks[2]);
            }
        }

        let input_chunk = if has_reply_preview { chunks[3] } else { chunks[2] };
        let input_title = if is_focused && !self.focus_on_chat_list {
            "Input (Alt+Enter for newline, Tab to cycle)"
        } else {
            "Input"
        };
        let mut input_text = if is_focused { pane.input_buffer.clone() } else { String::new() };
        
        // Show block cursor at cursor position when focused
        if is_focused && !self.focus_on_chat_list {
            let cursor_pos = pane.input_cursor.min(input_text.len());
            input_text.insert(cursor_pos, '█');
        }
        
        let input_block = if self.show_borders {
            Block::default().borders(Borders::ALL).title(input_title)
        } else {
            Block::default()
        };
        let input = Paragraph::new(input_text)
            .block(input_block)
            .wrap(Wrap { trim: false });
        f.render_widget(input, input_chunk);
    }

    pub async fn refresh_chats(&mut self) -> Result<()> {
        self.chats = self.whatsapp.get_dialogs().await?;
        Ok(())
    }

    /// Show a status notification that auto-expires
    pub fn notify(&mut self, message: &str) {
        self.status_message = Some(message.to_string());
        self.status_expire =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
    }

    /// Show a status notification with custom timeout duration
    pub fn notify_with_duration(&mut self, message: &str, duration_secs: u64) {
        self.status_message = Some(message.to_string());
        self.status_expire =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(duration_secs));
    }

    pub async fn open_chat_in_pane(&mut self, pane_idx: usize, chat_id: String, chat_name: &str) {
        let msg_data = match self.whatsapp.get_messages(&chat_id, 50).await {
            Ok(raw_messages) => raw_messages
                .iter()
                .map(|(msg_id, sender_id, sender_name, text, reply_to_id, media_type, reactions, timestamp)| {
                    crate::widgets::MessageData {
                        msg_id: msg_id.clone(),
                        sender_id: sender_id.clone(),
                        sender_name: sender_name.clone(),
                        text: text.clone(),
                        is_outgoing: sender_id == &self.my_user_jid,
                        timestamp: *timestamp,
                        media_type: media_type.clone(),
                        media_label: None,
                        reactions: reactions.clone(),
                        reply_to_msg_id: reply_to_id.clone(),
                        reply_sender: None,
                        reply_text: None,
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        if let Some(pane) = self.panes.get_mut(pane_idx) {
            pane.chat_id = Some(chat_id.clone());
            pane.chat_name = chat_name.to_string();
            pane.msg_data = msg_data;
            pane.messages.clear();
            pane.reply_to_message = None;
            pane.hide_reply_preview();
            pane.scroll_offset = 0;
            pane.format_cache.clear();

            // Set username from chats list if available
            if let Some(chat_info) = self.chats.iter().find(|c| c.id == chat_id) {
                pane.username = chat_info.username.clone();
            }
        }

        // Mark chat as read
        if let Some(chat_info) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat_info.unread = 0;
        }
    }

    pub async fn load_pane_messages_if_needed(&mut self, pane_idx: usize) {
        if let Some(pane) = self.panes.get(pane_idx) {
            if let Some(ref _chat_id) = pane.chat_id {
                if pane.msg_data.is_empty() {
                    let _ = self.refresh_pane_messages(pane_idx).await;
                }
            }
        }
    }

    // =========================================================================
    // Split pane management
    // =========================================================================

    pub fn split_vertical(&mut self) {
        let new_pane = ChatPane::new();
        let new_idx = self.panes.len();
        self.panes.push(new_pane);

        self.split_pane_in_tree(self.focused_pane_idx, SplitDirection::Vertical, new_idx);
        self.focused_pane_idx = new_idx;
        self.focus_on_chat_list = false;
    }

    pub fn split_horizontal(&mut self) {
        let new_pane = ChatPane::new();
        let new_idx = self.panes.len();
        self.panes.push(new_pane);

        self.split_pane_in_tree(self.focused_pane_idx, SplitDirection::Horizontal, new_idx);
        self.focused_pane_idx = new_idx;
        self.focus_on_chat_list = false;
    }

    fn split_pane_in_tree(
        &mut self,
        target_idx: usize,
        direction: SplitDirection,
        new_idx: usize,
    ) {
        Self::split_node_recursive_static(&mut self.pane_tree, target_idx, direction, new_idx);
    }

    fn split_node_recursive_static(
        node: &mut PaneNode,
        target_idx: usize,
        direction: SplitDirection,
        new_idx: usize,
    ) -> bool {
        match node {
            PaneNode::Single(idx) if *idx == target_idx => {
                node.split(direction, new_idx);
                true
            }
            PaneNode::Split { children, .. } => {
                for child in children.iter_mut() {
                    if Self::split_node_recursive_static(child, target_idx, direction, new_idx) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub fn toggle_split_direction(&mut self) {
        // Find the parent split node that directly contains the focused pane
        if Self::toggle_split_direction_recursive(&mut self.pane_tree, self.focused_pane_idx) {
        } else {
            self.notify("No split to toggle - pane is not in a split");
        }
    }

    fn toggle_split_direction_recursive(node: &mut PaneNode, target_idx: usize) -> bool {
        match node {
            PaneNode::Single(_) => false,
            PaneNode::Split { direction, children } => {
                // Check if target_idx is directly a child of this split (not nested deeper)
                let is_direct_child = children.iter().any(|child| {
                    matches!(child.as_ref(), PaneNode::Single(idx) if *idx == target_idx)
                });

                if is_direct_child {
                    // This is the parent split - toggle its direction
                    *direction = match *direction {
                        SplitDirection::Vertical => SplitDirection::Horizontal,
                        SplitDirection::Horizontal => SplitDirection::Vertical,
                    };
                    true
                } else {
                    // Target might be nested deeper, search in children
                    for child in children.iter_mut() {
                        if Self::toggle_split_direction_recursive(child, target_idx) {
                            return true;
                        }
                    }
                    false
                }
            }
        }
    }

    pub fn close_pane(&mut self) {
        let pane_count_before = self.pane_tree.count_panes();
        if pane_count_before <= 1 {
            self.notify("Cannot close the last pane");
            return;
        }
        
        let focused_idx = self.focused_pane_idx;
        let removed = self.pane_tree.find_and_remove_pane(focused_idx);
        
        if removed {
            let remaining = self.pane_tree.get_pane_indices();
            if !remaining.is_empty() {
                self.focused_pane_idx = remaining[0];
            }
        } else {
            self.notify("Failed to close pane");
        }
    }

    pub fn clear_pane(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            pane.clear();
        }
    }

    pub fn cycle_focus(&mut self) {
        let all_panes = self.pane_tree.get_pane_indices();
        crate::debug_log!("cycle_focus: focus_on_chat_list={}, all_panes={:?}", self.focus_on_chat_list, all_panes);
        
        if all_panes.is_empty() {
            crate::warn_log!("cycle_focus: No panes available!");
            return;
        }

        if self.focus_on_chat_list {
            // Going from chat list to first pane
            crate::debug_log!("cycle_focus: Moving from chat list to pane {}", all_panes[0]);
            self.focus_on_chat_list = false;
            self.focused_pane_idx = all_panes[0];
            self.mark_pane_chat_read(self.focused_pane_idx);
        } else {
            // Find current pane position
            if let Some(current_pos) = all_panes.iter().position(|&idx| idx == self.focused_pane_idx) {
                if current_pos + 1 < all_panes.len() {
                    // Go to next pane
                    self.focused_pane_idx = all_panes[current_pos + 1];
                    self.mark_pane_chat_read(self.focused_pane_idx);
                } else {
                    // Last pane, go back to chat list
                    crate::debug_log!("cycle_focus: Last pane, going back to chat list");
                    self.focus_on_chat_list = true;
                }
            } else {
                // Current pane not found, reset to first
                crate::warn_log!("cycle_focus: Current pane {} not found, resetting to {}", self.focused_pane_idx, all_panes[0]);
                self.focused_pane_idx = all_panes[0];
                self.mark_pane_chat_read(self.focused_pane_idx);
            }
        }
        crate::debug_log!("cycle_focus: After cycle - focus_on_chat_list={}, focused_pane_idx={}", self.focus_on_chat_list, self.focused_pane_idx);
    }

    pub fn cycle_focus_reverse(&mut self) {
        let all_panes = self.pane_tree.get_pane_indices();
        if all_panes.is_empty() {
            return;
        }

        if self.focus_on_chat_list {
            // Go to last pane
            self.focus_on_chat_list = false;
            self.focused_pane_idx = *all_panes.last().unwrap();
            self.mark_pane_chat_read(self.focused_pane_idx);
        } else {
            if let Some(current_pos) = all_panes.iter().position(|&idx| idx == self.focused_pane_idx) {
                if current_pos > 0 {
                    self.focused_pane_idx = all_panes[current_pos - 1];
                    self.mark_pane_chat_read(self.focused_pane_idx);
                } else {
                    self.focus_on_chat_list = true;
                }
            }
        }
    }

    pub fn focus_next_pane(&mut self) {
        let all_panes = self.pane_tree.get_pane_indices();
        if all_panes.len() < 2 {
            return;
        }
        if let Some(current_pos) = all_panes.iter().position(|&idx| idx == self.focused_pane_idx) {
            let next = (current_pos + 1) % all_panes.len();
            self.focused_pane_idx = all_panes[next];
            self.focus_on_chat_list = false;
            self.mark_pane_chat_read(self.focused_pane_idx);
        }
    }

    pub fn focus_prev_pane(&mut self) {
        let all_panes = self.pane_tree.get_pane_indices();
        if all_panes.len() < 2 {
            return;
        }
        if let Some(current_pos) = all_panes.iter().position(|&idx| idx == self.focused_pane_idx) {
            let prev = if current_pos > 0 { current_pos - 1 } else { all_panes.len() - 1 };
            self.focused_pane_idx = all_panes[prev];
            self.focus_on_chat_list = false;
            self.mark_pane_chat_read(self.focused_pane_idx);
        }
    }

    // =========================================================================
    // Toggle settings (matching Python's action_toggle_*)
    // =========================================================================

    pub fn toggle_reactions(&mut self) {
        self.show_reactions = !self.show_reactions;
        let status = if self.show_reactions { "ON" } else { "OFF" };
        self.notify(&format!("Reactions: {}", status));
        self.refresh_all_pane_displays();
    }

    pub fn toggle_notifications(&mut self) {
        self.show_notifications = !self.show_notifications;
        let status = if self.show_notifications {
            "ON"
        } else {
            "OFF"
        };
        self.notify(&format!("Desktop notifications: {}", status));
    }

    pub fn toggle_compact(&mut self) {
        self.compact_mode = !self.compact_mode;
        let status = if self.compact_mode { "ON" } else { "OFF" };
        self.notify(&format!("Compact mode: {}", status));
        self.refresh_all_pane_displays();
    }

    pub fn toggle_emojis(&mut self) {
        self.show_emojis = !self.show_emojis;
        let status = if self.show_emojis { "ON" } else { "OFF" };
        self.notify(&format!("Emojis: {}", status));
        self.refresh_all_pane_displays();
    }

    pub fn toggle_line_numbers(&mut self) {
        self.show_line_numbers = !self.show_line_numbers;
        let status = if self.show_line_numbers { "ON" } else { "OFF" };
        self.notify(&format!("Line numbers: {}", status));
        self.refresh_all_pane_displays();
    }

    pub fn toggle_timestamps(&mut self) {
        self.show_timestamps = !self.show_timestamps;
        let status = if self.show_timestamps { "ON" } else { "OFF" };
        self.notify(&format!("Timestamps: {}", status));
        self.refresh_all_pane_displays();
    }

    pub fn toggle_chat_list(&mut self) {
        self.show_chat_list = !self.show_chat_list;
        self.notify(&format!("Chat list: {}", if self.show_chat_list { "ON" } else { "OFF" }));
    }

    pub fn toggle_user_colors(&mut self) {
        self.show_user_colors = !self.show_user_colors;
        let status = if self.show_user_colors { "ON" } else { "OFF" };
        self.notify(&format!("User colors: {}", status));
        self.refresh_all_pane_displays();
    }

    pub fn toggle_borders(&mut self) {
        self.show_borders = !self.show_borders;
        self.notify(&format!("Borders: {}", if self.show_borders { "ON" } else { "OFF" }));
    }

    fn chat_list_groups(&self) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let mut open_chat_ids = std::collections::HashSet::new();
        for pane in &self.panes {
            if let Some(ref chat_id) = pane.chat_id {
                open_chat_ids.insert(chat_id);
            }
        }

        let mut unread = Vec::new();
        let mut active = Vec::new();
        let mut other = Vec::new();

        for (idx, chat) in self.chats.iter().enumerate() {
            if open_chat_ids.contains(&chat.id) {
                active.push(idx);
            } else if chat.unread > 0 {
                unread.push(idx);
            } else {
                other.push(idx);
            }
        }

        (unread, active, other)
    }

    fn chat_list_order(&self) -> Vec<usize> {
        let (mut unread, mut active, mut other) = self.chat_list_groups();
        
        // Sort each group by last_message_time (most recent first)
        // We need to parse last_message_time from chats, but since ChatInfo doesn't have it,
        // we'll sort by unread count first, then by index (which should be roughly chronological)
        // For now, just reverse to get most recent first within each group
        unread.reverse();
        active.reverse();
        other.reverse();
        
        let mut ordered = Vec::with_capacity(self.chats.len());
        ordered.extend(unread);
        ordered.extend(active);
        ordered.extend(other);
        ordered
    }
    
    /// Extract phone number from JID (e.g., "46760789806@s.whatsapp.net" -> "46760789806")
    fn extract_phone_from_jid(jid: &str) -> Option<String> {
        if jid.ends_with("@s.whatsapp.net") {
            Some(jid.strip_suffix("@s.whatsapp.net")?.to_string())
        } else if jid.ends_with("@lid") {
            // For @lid JIDs, try to extract phone number from the numeric part
            // Format is usually: <phone>@lid or <some_id>@lid
            // We'll use the part before @ as a key for matching
            Some(jid.strip_suffix("@lid")?.to_string())
        } else {
            None
        }
    }
    
    /// Normalize JID - prefer @s.whatsapp.net over @lid for the same phone number
    fn normalize_jid(jid: &str, all_chats: &[ChatInfo]) -> String {
        if jid.ends_with("@lid") {
            // Try to find a matching @s.whatsapp.net JID with the same name
            let lid_phone = Self::extract_phone_from_jid(jid);
            let chat_name = all_chats.iter().find(|c| c.id == jid).map(|c| &c.name);
            
            if let Some(name) = chat_name {
                // Look for a chat with the same name but @s.whatsapp.net JID
                if let Some(matching_chat) = all_chats.iter().find(|c| {
                    c.id.ends_with("@s.whatsapp.net") && c.name == *name
                }) {
                    crate::debug_log!("refresh_chat_list: Normalizing {}@lid to {} (same name: {})", 
                        lid_phone.as_deref().unwrap_or("unknown"), matching_chat.id, name);
                    return matching_chat.id.clone();
                }
            }
        }
        jid.to_string()
    }

    /// Refresh chat list from WhatsApp
    pub async fn refresh_chat_list(&mut self) -> Result<()> {
        crate::debug_log!("refresh_chat_list: Starting refresh");
        let new_chats = self.whatsapp.get_dialogs().await?;
        crate::debug_log!("refresh_chat_list: Got {} chats from WhatsApp", new_chats.len());
        
        // Get currently open chat IDs to preserve their unread status
        let open_chat_ids: std::collections::HashSet<String> = self.panes
            .iter()
            .filter_map(|p| p.chat_id.clone())
            .collect();
        crate::debug_log!("refresh_chat_list: {} chats are currently open", open_chat_ids.len());
        
        // Normalize JIDs - prefer @s.whatsapp.net over @lid for the same chat
        let normalized_chats: Vec<ChatInfo> = new_chats.iter().map(|c| {
            let normalized_id = Self::normalize_jid(&c.id, &new_chats);
            if normalized_id != c.id {
                crate::debug_log!("refresh_chat_list: Normalizing chat {} -> {}", c.id, normalized_id);
                ChatInfo {
                    id: normalized_id,
                    name: c.name.clone(),
                    username: c.username.clone(),
                    unread: c.unread,
                    _is_channel: c._is_channel,
                    is_group: c.is_group,
                }
            } else {
                c.clone()
            }
        }).collect();
        
        // Deduplicate by phone number/name - keep only one chat per phone number
        let mut seen_phones: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut deduplicated_chats: Vec<ChatInfo> = Vec::new();
        
        for chat in normalized_chats {
            let phone_key = if chat.id.ends_with("@s.whatsapp.net") {
                Self::extract_phone_from_jid(&chat.id)
            } else if chat.id.ends_with("@lid") {
                // For @lid, use name as key since we can't extract phone reliably
                Some(chat.name.clone())
            } else {
                None
            };
            
            if let Some(key) = phone_key {
                if let Some(&existing_idx) = seen_phones.get(&key) {
                    // Already have a chat with this phone/name - prefer @s.whatsapp.net over @lid
                    let existing_chat = &deduplicated_chats[existing_idx];
                    if existing_chat.id.ends_with("@lid") && chat.id.ends_with("@s.whatsapp.net") {
                        // Replace @lid with @s.whatsapp.net
                        crate::debug_log!("refresh_chat_list: Replacing {}@lid with {}@s.whatsapp.net (same phone: {})", 
                            existing_chat.id.strip_suffix("@lid").unwrap_or("unknown"), 
                            chat.id.strip_suffix("@s.whatsapp.net").unwrap_or("unknown"),
                            key);
                        deduplicated_chats[existing_idx] = chat;
                    } else {
                        // Keep existing, skip duplicate
                        crate::debug_log!("refresh_chat_list: Skipping duplicate chat {} (already have {})", chat.id, existing_chat.id);
                    }
                } else {
                    seen_phones.insert(key.clone(), deduplicated_chats.len());
                    deduplicated_chats.push(chat);
                }
            } else {
                // No phone/normalized key, just add it
                deduplicated_chats.push(chat);
            }
        }
        
        crate::debug_log!("refresh_chat_list: After deduplication: {} chats (was {})", 
            deduplicated_chats.len(), new_chats.len());
        
        // Create sets for efficient lookup
        let new_chat_ids: std::collections::HashSet<String> = deduplicated_chats.iter().map(|c| c.id.clone()).collect();
        
        // Create a map of new chats by ID for updates
        let mut new_chats_map: std::collections::HashMap<String, ChatInfo> = deduplicated_chats
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();
        
        // Remove duplicates first - keep only one chat per JID
        let mut seen_jids = std::collections::HashSet::new();
        let before_dedup = self.chats.len();
        self.chats.retain(|c| seen_jids.insert(c.id.clone()));
        if before_dedup != self.chats.len() {
            crate::debug_log!("refresh_chat_list: Removed {} duplicate chats", before_dedup - self.chats.len());
        }
        
        // Update existing chats
        let mut updated_count = 0;
        for existing_chat in &mut self.chats {
            if let Some(new_chat) = new_chats_map.remove(&existing_chat.id) {
                // Chat still exists - update it
                let was_open = open_chat_ids.contains(&existing_chat.id);
                let old_name = existing_chat.name.clone();
                let old_unread = existing_chat.unread;
                
                // Always update name (in case contact name changed)
                existing_chat.name = new_chat.name.clone();
                
                // Update unread: if chat is open, don't update unread from WhatsApp
                // (it will be cleared when marked read, or stay 0 if already read)
                // Otherwise, use the MAX of our current unread and WhatsApp's unread
                // This prevents us from overwriting unread counts we just incremented
                if !was_open {
                    // Use the maximum to preserve any increments we made
                    existing_chat.unread = existing_chat.unread.max(new_chat.unread);
                    if existing_chat.unread != new_chat.unread {
                        crate::debug_log!("refresh_chat_list: Preserved unread {} for chat {} (WhatsApp said {})", 
                            existing_chat.unread, existing_chat.id, new_chat.unread);
                    }
                }
                // If was_open, keep existing unread (it will be cleared by mark_pane_chat_read)
                
                if old_name != existing_chat.name || old_unread != existing_chat.unread {
                    crate::debug_log!("refresh_chat_list: Updated chat {}: name '{}'->'{}', unread {}->{} (was_open={})", 
                        existing_chat.id, old_name, existing_chat.name, old_unread, existing_chat.unread, was_open);
                }
                updated_count += 1;
            }
        }
        crate::debug_log!("refresh_chat_list: Updated {} existing chats", updated_count);
        
        // Add new chats that didn't exist before (avoid duplicates)
        let mut added_count = 0;
        for (_, new_chat) in new_chats_map {
            if !seen_jids.contains(&new_chat.id) {
                crate::debug_log!("refresh_chat_list: Adding new chat {}: '{}' (unread={})", 
                    new_chat.id, new_chat.name, new_chat.unread);
                self.chats.push(new_chat);
                added_count += 1;
            }
        }
        crate::debug_log!("refresh_chat_list: Added {} new chats", added_count);
        
        // Remove chats that no longer exist (but keep them if open in a pane)
        let before_remove = self.chats.len();
        self.chats.retain(|c| new_chat_ids.contains(&c.id) || open_chat_ids.contains(&c.id));
        if before_remove != self.chats.len() {
            crate::debug_log!("refresh_chat_list: Removed {} chats that no longer exist", before_remove - self.chats.len());
        }
        
        crate::debug_log!("refresh_chat_list: Final chat count: {}", self.chats.len());
        Ok(())
    }

    fn mark_pane_chat_read(&mut self, pane_idx: usize) {
        let chat_id = match self.panes.get(pane_idx).and_then(|p| p.chat_id.clone()) {
            Some(chat_id) => chat_id,
            None => return,
        };

        if let Some(chat_info) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat_info.unread = 0;
        }

        if let Some(pane) = self.panes.get_mut(pane_idx) {
            pane.unread_count_at_load = 0;
        }
    }


    /// Handle mouse click to select pane or open chat
    pub fn handle_mouse_click(&mut self, x: u16, y: u16) {
        // Check if clicking on a pane
        for (&pane_idx, &area) in &self.pane_areas {
            if x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height {
                // Clicked on this pane - make it active and move focus from chat list
                self.focused_pane_idx = pane_idx;
                self.focus_on_chat_list = false;
                crate::debug_log!("handle_mouse_click: Clicked on pane {}, setting focus_on_chat_list=false", pane_idx);
                self.mark_pane_chat_read(self.focused_pane_idx);
                return;
            }
        }
    }

    /// Handle mouse click on chat list
    pub async fn handle_chat_list_click(&mut self, y: u16, list_area: Rect) -> Result<()> {
        // Calculate which chat was clicked based on Y position
        // Each chat item is 1 line, starting at list_area.y + border_offset (after top border if present)
        let border_offset = if self.show_borders { 1 } else { 0 };
        if y < list_area.y + border_offset || y >= list_area.y + list_area.height - border_offset {
            return Ok(()); // Clicked on border or outside
        }
        
        let relative_y = (y - list_area.y - border_offset) as usize;
        
        // Build row map matching exactly how draw_chat_list renders
        // (headers are None, chats are Some(chat_idx))
        let (unread_group, active_group, other_group) = self.chat_list_groups();
        let _ordered_chats = self.chat_list_order();

        let mut row_map: Vec<Option<usize>> = Vec::new();
        
        // Add unread group header and chats
        if !unread_group.is_empty() {
            row_map.push(None); // Header "Unread"
            for chat_idx in unread_group.iter() {
                row_map.push(Some(*chat_idx));
            }
        }
        
        // Add active group header and chats
        if !active_group.is_empty() {
            row_map.push(None); // Header "Active"
            for chat_idx in active_group.iter() {
                row_map.push(Some(*chat_idx));
            }
        }
        
        // Add other group header and chats
        if !other_group.is_empty() {
            row_map.push(None); // Header "Other"
            for chat_idx in other_group.iter() {
                row_map.push(Some(*chat_idx));
            }
        }

        crate::debug_log!("handle_chat_list_click: row_map.len()={}, relative_y={}", row_map.len(), relative_y);
        if relative_y < row_map.len() {
            // Skip if clicked on a header (None)
            if let Some(chat_idx) = row_map[relative_y] {
                crate::debug_log!("handle_chat_list_click: Clicked on chat at index {} (relative_y={})", chat_idx, relative_y);
                // Get the chat before refreshing (in case refresh changes indices)
                if chat_idx < self.chats.len() {
                    let chat_id = self.chats[chat_idx].id.clone();
                    let chat_name = self.chats[chat_idx].name.clone();
                    crate::debug_log!("handle_chat_list_click: Opening chat {}: '{}'", chat_id, chat_name);
                    
                    // Refresh chat list first to get latest data
                    let _ = self.refresh_chat_list().await;
                    
                    // Recalculate order after refresh
                    let ordered_chats = self.chat_list_order();
                    
                    // Find the chat again after refresh (ID should be stable)
                    if let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) {
                        let chat_id = chat.id.clone();
                        let chat_name = chat.name.clone();
                        let chat_username = chat.username.clone();
                        let raw_messages = self.whatsapp.get_messages(&chat_id, 50).await?;

                        let mut msg_data: Vec<crate::widgets::MessageData> = raw_messages
                            .iter()
                            .map(|(msg_id, sender_id, sender_name, text, reply_to_id, media_type, reactions, timestamp)| {
                                let reply_to_msg_id = reply_to_id.clone();
                                
                                crate::widgets::MessageData {
                                    msg_id: msg_id.clone(),
                                    sender_id: sender_id.clone(),
                                    sender_name: sender_name.clone(),
                                    text: text.clone(),
                                    is_outgoing: sender_id == &self.my_user_jid,
                                    timestamp: *timestamp,
                                    media_type: media_type.clone(),
                                    media_label: None,
                                    reactions: reactions.clone(),
                                    reply_to_msg_id,
                                    reply_sender: None,
                                    reply_text: None,
                                }
                            })
                            .collect();
                        
                        // Sort messages by timestamp (oldest first) to ensure correct order
                        msg_data.sort_by_key(|m| m.timestamp);

                        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                            crate::debug_log!("handle_chat_list_click: Updating pane {} with chat {}, scrolling to bottom", self.focused_pane_idx, chat_id);
                            pane.chat_id = Some(chat_id.clone());
                            pane.chat_name = chat_name;
                            pane.username = chat_username;
                            pane.msg_data = msg_data;
                            pane.messages.clear(); // Clear status messages when switching chats
                            pane.reply_to_message = None;
                            pane.hide_reply_preview();
                            pane.scroll_offset = 0; // Scroll to bottom (0 means bottom when rendering)

                            // Mark chat as read
                            if let Some(chat_info) = self.chats.iter_mut().find(|c| c.id == chat_id) {
                                chat_info.unread = 0;
                            }
                        } else {
                            crate::warn_log!("handle_chat_list_click: Pane {} not found!", self.focused_pane_idx);
                        }
                        
                        // Update selected_chat_idx to match the clicked chat in ordered_chats
                        if let Some(ordered_idx) = ordered_chats.iter().position(|&idx| idx < self.chats.len() && self.chats[idx].id == chat_id) {
                            self.selected_chat_idx = ordered_idx;
                            crate::debug_log!("handle_chat_list_click: Updated selected_chat_idx to {}", ordered_idx);
                        } else {
                            crate::warn_log!("handle_chat_list_click: Could not find chat {} in ordered_chats", chat_id);
                        }
                        
                        // Keep focus on chat list so user can continue navigating
                        // self.focus_on_chat_list = false;
                        crate::debug_log!("handle_chat_list_click: Keeping focus_on_chat_list=true to allow navigation");
                    } else {
                        crate::warn_log!("handle_chat_list_click: chat_idx {} >= chats.len() {}", chat_idx, self.chats.len());
                    }
                } else {
                    crate::debug_log!("handle_chat_list_click: Clicked on header (None)");
                }
            } else {
                crate::warn_log!("handle_chat_list_click: relative_y {} >= row_map.len() {}", relative_y, row_map.len());
            }
        }
        Ok(())
    }

    /// Refresh all pane message displays (after toggling display settings)
    fn refresh_all_pane_displays(&mut self) {
        // Clear format caches so they re-render with new settings
        for pane in &mut self.panes {
            pane.format_cache.clear();
        }
    }

    // =========================================================================
    // Input handling
    // =========================================================================

    pub fn handle_up(&mut self) {
        if self.focus_on_chat_list {
            let max_idx = self.chat_list_order().len().saturating_sub(1);
            if self.selected_chat_idx > max_idx {
                self.selected_chat_idx = max_idx;
            } else if self.selected_chat_idx > 0 {
                self.selected_chat_idx -= 1;
            }
        } else {
            // Browse input history
            if !self.input_history.is_empty() {
                if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                    match self.history_idx {
                        None => {
                            // Save current input and start browsing
                            self.history_temp = pane.input_buffer.clone();
                            self.history_idx = Some(self.input_history.len() - 1);
                            pane.input_buffer = self.input_history[self.input_history.len() - 1].clone();
                            pane.input_cursor = pane.input_buffer.len();
                    }
                        Some(idx) if idx > 0 => {
                            self.history_idx = Some(idx - 1);
                            pane.input_buffer = self.input_history[idx - 1].clone();
                            pane.input_cursor = pane.input_buffer.len();
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn handle_down(&mut self) {
        crate::debug_log!("handle_down: focus_on_chat_list={}, selected_chat_idx={}", self.focus_on_chat_list, self.selected_chat_idx);
        if self.focus_on_chat_list {
            let max_idx = self.chat_list_order().len().saturating_sub(1);
            crate::debug_log!("handle_down: max_idx={}", max_idx);
            if self.selected_chat_idx < max_idx {
                self.selected_chat_idx += 1;
            }
            crate::debug_log!("handle_down: New selected_chat_idx={}", self.selected_chat_idx);
        } else {
            // Browse input history
            if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                if let Some(idx) = self.history_idx {
                    if idx + 1 < self.input_history.len() {
                        self.history_idx = Some(idx + 1);
                        pane.input_buffer = self.input_history[idx + 1].clone();
                        pane.input_cursor = pane.input_buffer.len();
                    } else {
                        // Back to current input
                        self.history_idx = None;
                        pane.input_buffer = self.history_temp.clone();
                        pane.input_cursor = pane.input_buffer.len();
                    }
                }
            }
        }
    }

    pub fn handle_page_up(&mut self) {
        if !self.focus_on_chat_list {
            if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                pane.scroll_up();
            }
        }
    }

    pub fn handle_page_down(&mut self) {
        if !self.focus_on_chat_list {
            if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                pane.scroll_down();
            }
        }
    }

    /// Handle Tab key: try autocomplete first, then cycle focus
    pub fn handle_tab(&mut self) {
        let is_empty = self.panes.get(self.focused_pane_idx)
            .map_or(true, |p| p.input_buffer.is_empty());
        
        if is_empty {
            self.cycle_focus();
            return;
        }

        // Try autocomplete
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            let (completed, hint) = try_autocomplete(&pane.input_buffer);
            if let Some(completed) = completed {
                pane.input_buffer = completed;
                pane.input_cursor = pane.input_buffer.len();
            } else if let Some(hint) = hint {
                self.notify(&hint);
            } else {
                self.cycle_focus();
            }
        }
    }

    pub async fn handle_enter(&mut self) -> Result<()> {
        let input_empty = self.panes.get(self.focused_pane_idx)
            .map_or(true, |p| p.input_buffer.is_empty());
        
        crate::debug_log!("handle_enter: input_empty={}, focus_on_chat_list={}, chats.len()={}, selected_chat_idx={}", 
            input_empty, self.focus_on_chat_list, self.chats.len(), self.selected_chat_idx);
        
        if input_empty {
            if self.focus_on_chat_list && !self.chats.is_empty() {
                crate::debug_log!("handle_enter: On chat list, opening selected chat");
                // Refresh chat list first to get latest data
                let _ = self.refresh_chat_list().await;
                
                let ordered_chats = self.chat_list_order();
                crate::debug_log!("handle_enter: ordered_chats.len()={}, selected_chat_idx={}", ordered_chats.len(), self.selected_chat_idx);
                if let Some(&chat_idx) = ordered_chats.get(self.selected_chat_idx) {
                    if chat_idx < self.chats.len() {
                        let chat = &self.chats[chat_idx];
                        let chat_id = chat.id.clone();
                        let chat_name = chat.name.clone();
                        let chat_username = chat.username.clone();
                        crate::debug_log!("handle_enter: Opening chat {}: '{}'", chat_id, chat_name);
                        let raw_messages = self.whatsapp.get_messages(&chat_id, 50).await?;
                        crate::debug_log!("handle_enter: Got {} messages for chat {}: '{}'", raw_messages.len(), chat_id, chat_name);

                        // Convert to MessageData for proper formatting support
                        let mut msg_data: Vec<crate::widgets::MessageData> = raw_messages
                            .iter()
                            .map(|(msg_id, sender_id, sender_name, text, reply_to_id, media_type, reactions, timestamp)| {
                                let reply_to_msg_id = reply_to_id.clone();
                                
                                crate::widgets::MessageData {
                                    msg_id: msg_id.clone(),
                                    sender_id: sender_id.clone(),
                                    sender_name: sender_name.clone(),
                                    text: text.clone(),
                                    is_outgoing: sender_id == &self.my_user_jid,
                                    timestamp: *timestamp, // Use actual timestamp from message
                                    media_type: media_type.clone(),
                                    media_label: None,
                                    reactions: reactions.clone(),
                                    reply_to_msg_id,
                                    reply_sender: None,
                                    reply_text: None,
                                }
                            })
                            .collect();
                        
                        // Sort messages by timestamp (oldest first) to ensure correct order
                        msg_data.sort_by_key(|m| m.timestamp);

                        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                            crate::debug_log!("handle_enter: Updating pane {} with chat {}, scrolling to bottom", self.focused_pane_idx, chat_id);
                            pane.chat_id = Some(chat_id.clone());
                            pane.chat_name = chat_name;
                            pane.username = chat_username;
                            pane.msg_data = msg_data;
                            pane.messages.clear(); // Clear status messages when switching chats
                            pane.reply_to_message = None;
                            pane.hide_reply_preview();
                            pane.scroll_offset = 0; // Scroll to bottom (0 means bottom when rendering)

                            // Mark chat as read
                            if let Some(chat_info) =
                                self.chats.iter_mut().find(|c| c.id == chat_id)
                            {
                                pane.unread_count_at_load = chat_info.unread;
                                chat_info.unread = 0;
                            }
                        } else {
                            crate::warn_log!("handle_enter: Pane {} not found!", self.focused_pane_idx);
                        }
                        // Keep focus on chat list so user can continue navigating
                        // self.focus_on_chat_list = false;
                        crate::debug_log!("handle_enter: Keeping focus_on_chat_list=true to allow navigation");
                    } else {
                        crate::warn_log!("handle_enter: chat_idx {} >= chats.len() {}", chat_idx, self.chats.len());
                    }
                } else {
                    crate::warn_log!("handle_enter: selected_chat_idx {} >= ordered_chats.len() {}", self.selected_chat_idx, ordered_chats.len());
                }
            } else {
                crate::debug_log!("handle_enter: Not on chat list or chats empty");
            }
        } else if !self.focus_on_chat_list {
            // Get input from active pane
            let (input_text, _chat_id, _reply_to_id) = if let Some(pane) = self.panes.get(self.focused_pane_idx) {
                (pane.input_buffer.clone(), pane.chat_id.clone(), pane.reply_to_message.clone())
            } else {
                return Ok(());
            };

            // Save to history (no duplicates)
            if self.input_history.last().map_or(true, |last| last != &input_text) {
                self.input_history.push(input_text.clone());
                if self.input_history.len() > 100 {
                    self.input_history.remove(0);
                }
            }
            self.history_idx = None;
            self.history_temp.clear();

            // Try command handling
            if input_text.starts_with('/') {
                let focused = self.focused_pane_idx;
                let handled = CommandHandler::handle(self, &input_text, focused).await?;
                if handled {
                    if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                        pane.input_buffer.clear();
                    pane.input_cursor = 0;
                    }
                    return Ok(());
                }
            }

            // Handle reply mode or normal send
            if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
                let chat_id_opt = pane.chat_id.clone();
                let reply_to_id_opt = pane.reply_to_message.clone();
                
                if let (Some(chat_id), Some(reply_to_id)) = (chat_id_opt, reply_to_id_opt)
                {
                    // FIRST: Add message DIRECTLY to pane IMMEDIATELY - no waiting!
                    let new_msg = crate::widgets::MessageData {
                        msg_id: String::new(), // Temporary ID
                        sender_id: self.my_user_jid.clone(),
                        sender_name: "You".to_string(),
                        text: input_text.clone(),
                        is_outgoing: true,
                        timestamp: chrono::Utc::now().timestamp(),
                        media_type: None,
                        media_label: None,
                        reactions: std::collections::HashMap::new(),
                        reply_to_msg_id: Some(reply_to_id.clone()),
                        reply_sender: None,
                        reply_text: None,
                    };
                    pane.msg_data.push(new_msg);
                    pane.format_cache.clear();
                    
                    pane.reply_to_message = None;
                    pane.hide_reply_preview();
                    pane.input_buffer.clear();
                    pane.input_cursor = 0;
                    
                    // THEN: Send message in background - don't wait!
                    let whatsapp = self.whatsapp.clone();
                    let chat_id_copy = chat_id.clone();
                    let reply_to_id_copy = reply_to_id.clone();
                    let input_text_copy = input_text.clone();
                    tokio::spawn(async move {
                        let _ = whatsapp.reply_to_message(&chat_id_copy, &reply_to_id_copy, &input_text_copy).await;
                    });
                } else if let Some(ref chat_id) = pane.chat_id {
                    // FIRST: Add message DIRECTLY to pane IMMEDIATELY - no waiting!
                    let new_msg = crate::widgets::MessageData {
                        msg_id: String::new(), // Temporary ID
                        sender_id: self.my_user_jid.clone(),
                        sender_name: "You".to_string(),
                        text: input_text.clone(),
                        is_outgoing: true,
                        timestamp: chrono::Utc::now().timestamp(),
                        media_type: None,
                        media_label: None,
                        reactions: std::collections::HashMap::new(),
                        reply_to_msg_id: None,
                        reply_sender: None,
                        reply_text: None,
                    };
                    pane.msg_data.push(new_msg);
                    pane.format_cache.clear();
                    
                    pane.input_buffer.clear();
                    pane.input_cursor = 0;
                    
                    // THEN: Send message in background - don't wait!
                    let whatsapp = self.whatsapp.clone();
                    let chat_id_copy = chat_id.clone();
                    let input_text_copy = input_text.clone();
                    tokio::spawn(async move {
                        let _ = whatsapp.send_message(&chat_id_copy, &input_text_copy).await;
                    });
                }
            }
        }
        Ok(())
    }

    pub fn handle_char(&mut self, c: char) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            pane.input_buffer.insert(pane.input_cursor, c);
            pane.input_cursor += c.len_utf8();
        }
        self.history_idx = None;
    }

    pub fn handle_backspace(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            if pane.input_cursor > 0 {
                let prev = pane.input_buffer[..pane.input_cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                pane.input_buffer.remove(prev);
                pane.input_cursor = prev;
            }
        }
        self.history_idx = None;
    }

    pub fn handle_delete(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            if pane.input_cursor < pane.input_buffer.len() {
                pane.input_buffer.remove(pane.input_cursor);
            }
        }
    }

    pub fn handle_input_left(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            if pane.input_cursor > 0 {
                pane.input_cursor = pane.input_buffer[..pane.input_cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
            }
        }
    }

    pub fn handle_input_right(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            if pane.input_cursor < pane.input_buffer.len() {
                pane.input_cursor = pane.input_buffer[pane.input_cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| pane.input_cursor + i)
                    .unwrap_or(pane.input_buffer.len());
            }
        }
    }

    pub fn handle_home(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            pane.input_cursor = 0;
        }
    }

    pub fn handle_end(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane_idx) {
            pane.input_cursor = pane.input_buffer.len();
        }
    }

    // =========================================================================
    // New message handling
    // =========================================================================

    pub async fn process_whatsapp_events(&mut self) -> Result<bool> {
        // Process incoming updates
        let updates = self.whatsapp.poll_updates().await?;
        let had_updates = !updates.is_empty();
        
        if had_updates {
            crate::debug_log!("process_whatsapp_events: Got {} updates", updates.len());
        }

        for update in updates {
            match update {
                crate::whatsapp::WhatsAppUpdate::NewMessage {
                    chat_jid,
                    sender_name,
                    text,
                    is_outgoing,
                } => {
                    crate::debug_log!("NewMessage received: chat_jid={}, sender={}, text_len={}, is_outgoing={}", 
                        chat_jid, sender_name, text.len(), is_outgoing);
                    
                    // Don't process outgoing messages as "new" - they're already shown via local echo
                    if is_outgoing {
                        crate::debug_log!("Skipping outgoing message for chat {}", chat_jid);
                        continue;
                    }
                    
                    // Check if any pane has this chat open
                    let matching_panes: Vec<usize> = self
                        .panes
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| {
                            p.chat_id.as_ref() == Some(&chat_jid)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    
                    crate::debug_log!("Matching panes for chat {}: {:?}", chat_jid, matching_panes);

                if !matching_panes.is_empty() {
                    crate::debug_log!("Chat {} is open in panes {:?}, reloading messages", chat_jid, matching_panes);
                    // Chat is open - reload messages immediately to show new message
                    // Add a small delay to let sync process finish writing
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    if let Ok(raw_messages) =
                        self.whatsapp.get_messages(&chat_jid, 50).await
                    {
                        crate::debug_log!("Loaded {} messages for chat {}", raw_messages.len(), chat_jid);
                        // Convert to MessageData for proper formatting support
                        let mut msg_data: Vec<crate::widgets::MessageData> = raw_messages
                            .iter()
                            .map(|(msg_id, sender_id, sender_name, text, reply_to_id, media_type, reactions, timestamp)| {
                                let reply_to_msg_id = reply_to_id.clone();
                                
                                crate::widgets::MessageData {
                                    msg_id: msg_id.clone(),
                                    sender_id: sender_id.clone(),
                                    sender_name: sender_name.clone(),
                                    text: text.clone(),
                                    is_outgoing: sender_id == &self.my_user_jid,
                                    timestamp: *timestamp, // Use actual timestamp from message
                                    media_type: media_type.clone(),
                                    media_label: None,
                                    reactions: reactions.clone(),
                                    reply_to_msg_id,
                                    reply_sender: None,
                                    reply_text: None,
                                }
                            })
                            .collect();
                        
                        // Sort messages by timestamp (oldest first) to ensure correct order
                        msg_data.sort_by_key(|m| m.timestamp);

                        for idx in &matching_panes {
                            if let Some(pane) = self.panes.get_mut(*idx) {
                                crate::debug_log!("Updating pane {} with {} messages, scrolling to bottom", idx, msg_data.len());
                                pane.msg_data = msg_data.clone();
                                pane.format_cache.clear(); // Clear cache so messages are re-rendered
                                pane.scroll_offset = 0; // Scroll to bottom (0 means bottom when rendering)
                                // Don't clear messages - they may contain status messages
                            }
                        }
                    } else {
                        crate::warn_log!("Failed to load messages for chat {}", chat_jid);
                    }
                    
                    // Update chat list after loading messages (to update unread count)
                    crate::debug_log!("Refreshing chat list after message update");
                    let _ = self.refresh_chat_list().await;
                } else {
                        crate::debug_log!("Chat {} is not open, updating chat list and unread", chat_jid);
                        // Chat is not open - increment unread FIRST, then update chat list
                        // This way our increment won't be overwritten
                        if let Some(chat_info) = self
                            .chats
                            .iter_mut()
                            .find(|c| c.id == chat_jid)
                        {
                            let old_unread = chat_info.unread;
                            // Increment unread before refreshing (so refresh won't overwrite it if chat is not open)
                            chat_info.unread += 1;
                            crate::debug_log!("Chat {} unread: {} -> {} (before refresh)", chat_jid, old_unread, chat_info.unread);
                        }
                        
                        // Now refresh chat list (but preserve unread for chats not open)
                        let _ = self.refresh_chat_list().await;
                        
                        // Verify unread is still set (refresh_chat_list should preserve it for non-open chats)
                        if let Some(chat_info) = self
                            .chats
                            .iter_mut()
                            .find(|c| c.id == chat_jid)
                        {
                            crate::debug_log!("Chat {} unread after refresh: {}", chat_jid, chat_info.unread);
                            let chat_name = chat_info.name.clone();
                            let preview = if text.chars().count() > 50 {
                                let truncate_at = text
                                    .char_indices()
                                    .nth(50)
                                    .map(|(i, _)| i)
                                    .unwrap_or(text.len());
                                format!("{}...", &text[..truncate_at])
                            } else {
                                text.clone()
                            };

                            // Desktop notification
                            if self.show_notifications && !is_outgoing {
                                send_desktop_notification(&chat_name, &preview);
                            }

                            self.notify(&format!("{}: {}", chat_name, preview));
                        }
                    }
                }
                crate::whatsapp::WhatsAppUpdate::UserTyping {
                    chat_jid,
                    user_name,
                } => {
                    for pane in &mut self.panes {
                        if pane.chat_id.as_ref() == Some(&chat_jid)
                        {
                            pane.show_typing_indicator(&user_name);
                        }
                    }
                }
            }
        }

        Ok(had_updates)
    }

    // =========================================================================
    // State persistence
    // =========================================================================

    pub fn save_state(&self) -> Result<()> {
        let layout = LayoutData {
            panes: self
                .panes
                .iter()
                .map(|p| {
                    let filter_type_str = p.filter_type.as_ref().map(|ft| match ft {
                        crate::widgets::FilterType::Sender => "sender".to_string(),
                        crate::widgets::FilterType::Media => "media".to_string(),
                        crate::widgets::FilterType::Link => "link".to_string(),
                    });
                    PaneState {
                        chat_id: p.chat_id.clone(),
                        chat_name: p.chat_name.clone(),
                        scroll_offset: p.scroll_offset,
                        filter_type: filter_type_str,
                        filter_value: p.filter_value.clone(),
                    }
                })
                .collect(),
            focused_pane: self.focused_pane_idx,
            pane_tree: Some(self.pane_tree.clone()),
        };
        layout.save(&self.config)?;

        self.aliases.save(&self.config)?;

        let mut config = self.config.clone();
        config.settings.show_reactions = self.show_reactions;
        config.settings.show_notifications = self.show_notifications;
        config.settings.compact_mode = self.compact_mode;
        config.settings.show_emojis = self.show_emojis;
        config.settings.show_line_numbers = self.show_line_numbers;
        config.settings.show_timestamps = self.show_timestamps;
        config.settings.show_user_colors = self.show_user_colors;
        config.settings.show_borders = self.show_borders;
        config.settings.show_chat_list = self.show_chat_list;
        config.save()?;

        Ok(())
    }
}

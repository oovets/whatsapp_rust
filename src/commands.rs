use anyhow::Result;

use crate::app::App;
use crate::widgets::FilterType;

pub struct Command {
    pub name: String,
    pub args: Vec<String>,
    pub _full_text: String,
}

impl Command {
    pub fn parse(text: &str) -> Option<Self> {
        if !text.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let name = parts[0][1..].to_string();
        let args = parts[1..].iter().map(|s| s.to_string()).collect();

        Some(Command {
            name,
            args,
            _full_text: text.to_string(),
        })
    }
}

pub struct CommandHandler;

impl CommandHandler {
    pub async fn handle(app: &mut App, text: &str, pane_idx: usize) -> Result<bool> {
        let cmd = match Command::parse(text) {
            Some(c) => c,
            None => return Ok(false),
        };

        match cmd.name.as_str() {
            "reply" | "r" => {
                Self::handle_reply(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "media" | "m" => {
                Self::handle_media(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "edit" | "e" => {
                Self::handle_edit(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "delete" | "del" | "d" => {
                Self::handle_delete(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "alias" => {
                Self::handle_alias(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "unalias" => {
                Self::handle_unalias(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "filter" => {
                Self::handle_filter(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "search" | "s" => {
                Self::handle_search(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "new" => {
                Self::handle_new_chat(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "newgroup" => {
                Self::handle_new_group(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "add" => {
                Self::handle_add_member(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "kick" | "remove" => {
                Self::handle_remove_member(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "members" => {
                Self::handle_members(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            "forward" | "fwd" | "f" => {
                Self::handle_forward(app, &cmd, pane_idx).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn handle_reply(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /reply N [text]");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /reply N [text]");
                return Ok(());
            }
        };

        if let Some(pane) = app.panes.get_mut(pane_idx) {
            if cmd.args.len() > 1 {
                // Reply with inline text
                let text = cmd.args[1..].join(" ");
                if let Some(ref chat_id) = pane.chat_id {
                    // Get actual message ID from msg_data
                    if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                        match app
                            .whatsapp
                            .reply_to_message(chat_id, &msg_data.msg_id, &text)
                            .await
                        {
                            Ok(_) => pane.add_message(format!("✓ Replied to #{}", msg_num)),
                            Err(e) => pane.add_message(format!("✗ Reply failed: {}", e)),
                        }
                    } else {
                        pane.add_message(format!("✗ Message #{} not found", msg_num));
                    }
                }
            } else {
                // Set reply mode with preview - find actual message ID from msg_data
                if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                    let actual_msg_id = msg_data.msg_id.clone();
                    pane.reply_to_message = Some(actual_msg_id);
                    
                    // Get first line of message for preview (max 60 chars)
                    let first_line = msg_data.text.lines().next().unwrap_or(&msg_data.text);
                    let preview_text = if first_line.chars().count() > 60 {
                        let truncate_at = first_line.char_indices().nth(60).map(|(i, _)| i).unwrap_or(first_line.len());
                        format!("{}...", &first_line[..truncate_at])
                    } else {
                        first_line.to_string()
                    };
                    
                    pane.show_reply_preview(format!("Reply to #{}: {}", msg_num, preview_text));
                    app.notify(&format!("Replying to message #{}. Type your reply.", msg_num));
                } else {
                    pane.add_message(format!("✗ Message #{} not found", msg_num));
                }
            }
        }

        Ok(())
    }

    async fn handle_media(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        crate::info_log!("handle_media: Command received with args: {:?}", cmd.args);
        
        if cmd.args.is_empty() {
            app.notify("Usage: /media N or /m N");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /media N");
                return Ok(());
            }
        };

        crate::info_log!("handle_media: Parsed msg_num: {}", msg_num);

        // Get the actual WhatsApp message ID from the pane's message data
        let (chat_id, whatsapp_msg_id) = if let Some(pane) = app.panes.get(pane_idx) {
            if let Some(ref chat_id) = pane.chat_id {
                // msg_num is 1-indexed, msg_data is 0-indexed
                if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                    crate::info_log!("handle_media: Found message in pane.msg_data - whatsapp msg_id: {}, text: '{}'", 
                        msg_data.msg_id, msg_data.text);
                    (Some(chat_id.clone()), Some(msg_data.msg_id.clone()))
                } else {
                    crate::error_log!("handle_media: Message #{} not found in pane (have {} messages)", 
                        msg_num, pane.msg_data.len());
                    app.notify(&format!("Message #{} not found", msg_num));
                    return Ok(());
                }
            } else {
                crate::error_log!("handle_media: No chat_id in pane");
                app.notify("No chat selected");
                return Ok(());
            }
        } else {
            crate::error_log!("handle_media: Pane not found");
            app.notify("Pane not found");
            return Ok(());
        };

        if let (Some(chat_id), Some(whatsapp_msg_id)) = (chat_id, whatsapp_msg_id) {
            app.notify(&format!("Downloading media from #{}...", msg_num));
            let downloads_dir = std::env::temp_dir();

            match app
                .whatsapp
                .download_media_by_id(&chat_id, &whatsapp_msg_id, &downloads_dir)
                .await
            {
                Ok(path) => {
                    #[cfg(target_os = "macos")]
                    {
                        let _ = std::process::Command::new("open").arg(&path).spawn();
                    }
                    #[cfg(target_os = "linux")]
                    {
                        let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                    }
                    app.notify_with_duration(
                        &format!(
                            "✓ {}",
                            std::path::Path::new(&path)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                        ),
                        3,
                    );
                }
                Err(e) => {
                    app.notify(&format!("✗ {}", e));
                }
            }
        }

        Ok(())
    }

    async fn handle_edit(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.len() < 2 {
            app.notify("Usage: /edit N new_text");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /edit N new_text");
                return Ok(());
            }
        };

        let new_text = cmd.args[1..].join(" ");

        if let Some(pane) = app.panes.get_mut(pane_idx) {
            if let Some(ref chat_id) = pane.chat_id {
                // Get actual message ID from msg_data
                if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                    match app
                        .whatsapp
                        .edit_message(chat_id, &msg_data.msg_id, &new_text)
                        .await
                    {
                        Ok(_) => {
                            pane.add_message(format!("✓ Edited message #{}", msg_num));
                            app.notify("Message edited");
                        }
                        Err(e) => {
                            pane.add_message(format!("✗ Edit failed: {}", e));
                            app.notify(&format!("Edit failed: {}", e));
                        }
                    }
                } else {
                    pane.add_message(format!("✗ Message #{} not found", msg_num));
                }
            }
        }

        Ok(())
    }

    async fn handle_delete(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /delete N");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /delete N");
                return Ok(());
            }
        };

        if let Some(pane) = app.panes.get_mut(pane_idx) {
            if let Some(ref chat_id) = pane.chat_id {
                // Get actual message ID from msg_data
                if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                    match app.whatsapp.delete_message(chat_id, &msg_data.msg_id).await {
                        Ok(_) => {
                            pane.add_message(format!("✓ Deleted message #{}", msg_num));
                            app.notify("Message deleted");
                        }
                        Err(e) => {
                            pane.add_message(format!("✗ Delete failed: {}", e));
                            app.notify(&format!("Delete failed: {}", e));
                        }
                    }
                } else {
                    pane.add_message(format!("✗ Message #{} not found", msg_num));
                }
            }
        }

        Ok(())
    }

    async fn handle_alias(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.len() < 2 {
            app.notify("Usage: /alias N name");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /alias N name");
                return Ok(());
            }
        };

        let alias = cmd.args[1..].join(" ");

        if let Some(pane) = app.panes.get_mut(pane_idx) {
            if let Some(ref _chat_id) = pane.chat_id {
                // Get sender from msg_data
                if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                    let sender_id = msg_data.sender_id.clone();
                    app.aliases.insert(sender_id, alias.clone());
                    app.aliases.save(&app.config)?;
                    pane.add_message(format!("✓ Alias set: {}", alias));
                    app.notify(&format!("Alias set: {}", alias));
                } else {
                    pane.add_message(format!("✗ Message #{} not found", msg_num));
                }
            }
        }

        Ok(())
    }

    async fn handle_unalias(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /unalias N");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /unalias N");
                return Ok(());
            }
        };

        if let Some(pane) = app.panes.get_mut(pane_idx) {
            if let Some(ref _chat_id) = pane.chat_id {
                // Get sender from msg_data
                if let Some(msg_data) = pane.msg_data.get((msg_num - 1) as usize) {
                    let sender_id = msg_data.sender_id.clone();
                    if app.aliases.remove(&sender_id).is_some() {
                        app.aliases.save(&app.config)?;
                        pane.add_message("✓ Alias removed".to_string());
                        app.notify("Alias removed");
                    } else {
                        pane.add_message("✗ No alias found".to_string());
                        app.notify("No alias set for this user");
                    }
                } else {
                    pane.add_message(format!("✗ Message #{} not found", msg_num));
                }
            }
        }

        Ok(())
    }

    async fn handle_filter(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            if let Some(pane) = app.panes.get(pane_idx) {
                if pane.filter_type.is_some() {
                    let ft = match &pane.filter_type {
                        Some(FilterType::Sender) => "sender",
                        Some(FilterType::Media) => "media",
                        Some(FilterType::Link) => "link",
                        None => "",
                    };
                    let fv = pane.filter_value.as_deref().unwrap_or("");
                    app.notify(&format!("Current filter: {}={}", ft, fv));
                } else {
                    app.notify("Usage: /filter off | photo | video | audio | doc | link | <name>");
                }
            }
            return Ok(());
        }

        let filter_arg = cmd.args[0].to_lowercase();

        if filter_arg == "off" {
            if let Some(pane) = app.panes.get_mut(pane_idx) {
                pane.filter_type = None;
                pane.filter_value = None;
                pane.format_cache.clear();
            }
            app.notify("Filter disabled");
            return Ok(());
        }

        // Media type filters
        let media_types: &[(&str, &str)] = &[
            ("photo", "photo"),
            ("photos", "photo"),
            ("video", "video"),
            ("videos", "video"),
            ("audio", "audio"),
            ("voice", "voice"),
            ("doc", "document"),
            ("document", "document"),
            ("documents", "document"),
            ("file", "document"),
            ("files", "document"),
            ("link", "link"),
            ("links", "link"),
            ("url", "link"),
            ("sticker", "sticker"),
            ("stickers", "sticker"),
            ("gif", "gif"),
            ("gifs", "gif"),
        ];

        let notify_msg;
        if let Some((_, media_type)) = media_types.iter().find(|(k, _)| *k == filter_arg) {
            if let Some(pane) = app.panes.get_mut(pane_idx) {
                if *media_type == "link" {
                    pane.filter_type = Some(FilterType::Link);
                } else {
                    pane.filter_type = Some(FilterType::Media);
                }
                pane.filter_value = Some(media_type.to_string());
                pane.format_cache.clear();
            }
            notify_msg = format!("Filtering: {} only", media_type);
        } else {
            let filter_val = cmd.args.join(" ");
            notify_msg = format!("Filtering: messages from '{}'", filter_val);
            if let Some(pane) = app.panes.get_mut(pane_idx) {
                pane.filter_type = Some(FilterType::Sender);
                pane.filter_value = Some(filter_val);
                pane.format_cache.clear();
            }
        }
        app.notify(&notify_msg);

        Ok(())
    }

    async fn handle_search(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /search <query> or /s <query>");
            return Ok(());
        }

        let query = cmd.args.join(" ");

        if let Some(pane) = app.panes.get(pane_idx) {
            if pane.chat_id.is_none() {
                app.notify("Select a chat first");
                return Ok(());
            }
        }

        let chat_id = app.panes.get(pane_idx).and_then(|p| p.chat_id.clone());
        if let Some(ref chat_id) = chat_id {
            app.notify(&format!("Searching for '{}'...", query));

            match app.whatsapp.search_messages(chat_id, &query, 100).await {
                Ok(results) => {
                    let count = results.len();
                    if count == 0 {
                        app.notify("No results found");
                    } else {
                        // Convert to MessageData for proper formatting support
                        let msg_data: Vec<crate::widgets::MessageData> = results
                            .iter()
                            .map(|(msg_id, sender_id, sender_name, text, reply_to_id, reactions)| {
                                let reply_to_msg_id = reply_to_id.clone();
                                
                                crate::widgets::MessageData {
                                    msg_id: msg_id.clone(),
                                    sender_id: sender_id.clone(),
                                    sender_name: sender_name.clone(),
                                    text: text.clone(),
                                    is_outgoing: sender_id == &app.my_user_jid,
                                    timestamp: chrono::Utc::now().timestamp(),
                                    media_type: None,
                                    media_label: None,
                                    reactions: reactions.clone(),
                                    reply_to_msg_id,
                                    reply_sender: None,
                                    reply_text: None,
                                }
                            })
                            .collect();

                        if let Some(pane) = app.panes.get_mut(pane_idx) {
                            pane.msg_data = msg_data;
                            // Don't clear messages - they may contain status messages
                            pane.chat_name = format!(
                                "{} | Search: '{}' ({} results)",
                                pane.chat_name.split(" | Search:").next().unwrap_or(&pane.chat_name),
                                query,
                                count
                            );
                            pane.scroll_offset = 0;
                        }
                        app.notify(&format!("Found {} results", count));
                    }
                }
                Err(e) => {
                    app.notify(&format!("Search failed: {}", e));
                }
            }
        }

        Ok(())
    }

    async fn handle_new_chat(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /new @username");
            return Ok(());
        }

        let username = &cmd.args[0];
        app.notify(&format!("Looking up {}...", username));

        match app.whatsapp.resolve_username(username).await {
            Ok(Some((chat_id, chat_name, _is_group))) => {
                app.open_chat_in_pane(pane_idx, chat_id, &chat_name).await;
            }
            Ok(None) => {
                app.notify(&format!("User '{}' not found", username));
            }
            Err(e) => {
                app.notify(&format!("Lookup failed: {}", e));
            }
        }

        Ok(())
    }

    async fn handle_new_group(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /newgroup <name>");
            return Ok(());
        }

        let group_name = cmd.args.join(" ");
        app.notify(&format!("Creating group '{}'...", group_name));

        match app.whatsapp.create_group(&group_name, vec![]).await {
            Ok(chat_id) => {
                // Refresh chat list and open the new group
                let _ = app.refresh_chats().await;
                app.open_chat_in_pane(pane_idx, chat_id, &group_name).await;
                app.notify(&format!("Group '{}' created", group_name));
            }
            Err(e) => {
                app.notify(&format!("Failed to create group: {}", e));
            }
        }

        Ok(())
    }

    async fn handle_add_member(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /add @username");
            return Ok(());
        }

        let username = &cmd.args[0];
        let chat_id = if let Some(pane) = app.panes.get(pane_idx) {
            match &pane.chat_id {
                Some(id) => id.clone(),
                None => {
                    app.notify("Open a group chat first");
                    return Ok(());
                }
            }
        } else {
            return Ok(());
        };

        app.notify(&format!("Adding {}...", username));

        match app.whatsapp.add_member(&chat_id, username).await {
            Ok(_) => {
                if let Some(pane) = app.panes.get_mut(pane_idx) {
                    pane.add_message(format!("✓ Added {} to group", username));
                }
                app.notify(&format!("{} added to group", username));
            }
            Err(e) => {
                app.notify(&format!("Failed to add {}: {}", username, e));
            }
        }

        Ok(())
    }

    async fn handle_remove_member(
        app: &mut App,
        cmd: &Command,
        pane_idx: usize,
    ) -> Result<()> {
        if cmd.args.is_empty() {
            app.notify("Usage: /kick @username or /remove @username");
            return Ok(());
        }

        let username = &cmd.args[0];
        let chat_id = if let Some(pane) = app.panes.get(pane_idx) {
            match &pane.chat_id {
                Some(id) => id.clone(),
                None => {
                    app.notify("Open a group chat first");
                    return Ok(());
                }
            }
        } else {
            return Ok(());
        };

        app.notify(&format!("Removing {}...", username));

        match app.whatsapp.remove_member(&chat_id, username).await {
            Ok(_) => {
                if let Some(pane) = app.panes.get_mut(pane_idx) {
                    pane.add_message(format!("✓ Removed {} from group", username));
                }
                app.notify(&format!("{} removed from group", username));
            }
            Err(e) => {
                app.notify(&format!("Failed to remove {}: {}", username, e));
            }
        }

        Ok(())
    }

    async fn handle_members(app: &mut App, _cmd: &Command, pane_idx: usize) -> Result<()> {
        let chat_id = if let Some(pane) = app.panes.get(pane_idx) {
            match &pane.chat_id {
                Some(id) => id.clone(),
                None => {
                    app.notify("Open a group chat first");
                    return Ok(());
                }
            }
        } else {
            return Ok(());
        };

        app.notify("Loading members...");

        match app.whatsapp.get_members(&chat_id).await {
            Ok(members) => {
                if let Some(pane) = app.panes.get_mut(pane_idx) {
                    pane.add_message(format!("--- Members ({}) ---", members.len()));
                    for (id, name, role) in &members {
                        pane.add_message(format!("  {} (id:{}) - {}", name, id, role));
                    }
                    pane.add_message("---".to_string());
                }
                app.notify(&format!("{} members", members.len()));
            }
            Err(e) => {
                app.notify(&format!("Failed to load members: {}", e));
            }
        }

        Ok(())
    }

    async fn handle_forward(app: &mut App, cmd: &Command, pane_idx: usize) -> Result<()> {
        if cmd.args.len() < 2 {
            app.notify("Usage: /forward N @username or /fwd N @username");
            return Ok(());
        }

        let msg_num: i32 = match cmd.args[0].trim_start_matches('#').parse() {
            Ok(n) => n,
            Err(_) => {
                app.notify("Usage: /forward N @username");
                return Ok(());
            }
        };

        let target = &cmd.args[1];

        let (from_chat_id, message_id) = if let Some(pane) = app.panes.get(pane_idx) {
            let from_id = match &pane.chat_id {
                Some(id) => id.clone(),
                None => {
                    app.notify("No chat selected");
                    return Ok(());
                }
            };
            // Get actual WhatsApp message ID from msg_data
            let msg_id = match pane.msg_data.get((msg_num - 1) as usize) {
                Some(msg) => msg.msg_id.clone(),
                None => {
                    app.notify(&format!("Message #{} not found", msg_num));
                    return Ok(());
                }
            };
            (from_id, msg_id)
        } else {
            return Ok(());
        };

        app.notify(&format!("Forwarding #{} to {}...", msg_num, target));

        // Resolve target
        match app.whatsapp.resolve_username(target).await {
            Ok(Some((to_chat_id, _name, _is_group))) => {
                match app.whatsapp.forward_message(&from_chat_id, &message_id, &to_chat_id).await {
                    Ok(_) => {
                        if let Some(pane) = app.panes.get_mut(pane_idx) {
                            pane.add_message(format!("✓ Forwarded #{} to {}", msg_num, target));
                        }
                        app.notify(&format!("Forwarded to {}", target));
                    }
                    Err(e) => {
                        app.notify(&format!("Forward failed: {}", e));
                    }
                }
            }
            Ok(None) => {
                app.notify(&format!("User '{}' not found", target));
            }
            Err(e) => {
                app.notify(&format!("Lookup failed: {}", e));
            }
        }

        Ok(())
    }
}

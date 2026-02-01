use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::process::Command as TokioCommand;
use rusqlite::{Connection, params};

fn format_phone_number(jid: &str) -> String {
    // Extract phone number from JID (e.g., "46760789806@s.whatsapp.net" -> "46760789806")
    if let Some(at_pos) = jid.find('@') {
        let phone = &jid[..at_pos];
        // Format with + prefix if it doesn't have one
        if phone.starts_with('+') {
            phone.to_string()
        } else {
            format!("+{}", phone)
        }
    } else {
        jid.to_string()
    }
}

use crate::app::ChatInfo;
use crate::config::Config;

/// Updates received from WhatsApp
#[derive(Debug, Clone)]
pub enum WhatsAppUpdate {
    NewMessage {
        chat_jid: String,
        sender_name: String,
        text: String,
        is_outgoing: bool,
    },
    #[allow(dead_code)]
    UserTyping {
        chat_jid: String,
        user_name: String,
    },
}

#[derive(Clone)]
pub struct WhatsAppClient {
    cli_path: PathBuf,
    store_path: PathBuf,
    pending_updates: Arc<Mutex<Vec<WhatsAppUpdate>>>,
    my_jid: Arc<Mutex<Option<String>>>,
    last_synced_message_id: Arc<Mutex<Option<String>>>,
    contact_cache: Arc<Mutex<std::collections::HashMap<String, String>>>, // JID -> name
}

#[derive(Debug, Deserialize)]
struct WhatsAppResponse {
    success: bool,
    data: Option<serde_json::Value>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatListItem {
    jid: String,
    name: String,
    #[serde(default)]
    unread: u32,
}

#[derive(Debug, Deserialize)]
struct MessageItem {
    id: String,
    #[serde(rename = "chat_jid")]
    chat_jid: String,
    #[serde(rename = "chat_name")]
    chat_name: Option<String>,
    sender: String,
    #[serde(rename = "sender_name")]
    sender_name: Option<String>,
    content: String,
    timestamp: String,
    #[serde(rename = "is_from_me")]
    from_me: bool,
    #[serde(rename = "media_type")]
    media_type: Option<String>,
}

impl WhatsAppClient {
    pub async fn new(config: &Config) -> Result<Self> {
        let cli_path = config.whatsapp_cli_path.clone();
        let store_path = config.store_path();
        
        // Ensure store directory exists
        std::fs::create_dir_all(&store_path)?;
        
        // Check if whatsapp-cli is authenticated
        let client = Self {
            cli_path: cli_path.clone(),
            store_path: store_path.clone(),
            pending_updates: Arc::new(Mutex::new(Vec::new())),
            my_jid: Arc::new(Mutex::new(None)),
            last_synced_message_id: Arc::new(Mutex::new(None)),
            contact_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
        };
        
        // Pre-populate contact cache from chats
        if let Ok(chats) = client.get_dialogs().await {
            let mut cache = client.contact_cache.lock().await;
            for chat in chats {
                // Cache all chats (both individual and groups) with their display names
                cache.insert(chat.id.clone(), chat.name.clone());
            }
        }
        
        // Also load contacts from whatsapp.db database in background
        let contacts_db_path = store_path.join("whatsapp.db");
        if contacts_db_path.exists() {
            let contacts_db_path_clone = contacts_db_path.clone();
            let contact_cache = client.contact_cache.clone();
            tokio::spawn(async move {
                let contacts = tokio::task::spawn_blocking(move || {
                    let mut contacts_map = std::collections::HashMap::new();
                    if let Ok(conn) = Connection::open(&contacts_db_path_clone) {
                        if let Ok(mut stmt) = conn.prepare(
                            "SELECT their_jid, COALESCE(NULLIF(full_name, ''), NULLIF(first_name, ''), NULLIF(push_name, ''), NULLIF(business_name, '')) as name 
                             FROM whatsmeow_contacts 
                             WHERE name IS NOT NULL AND name != ''"
                        ) {
                            if let Ok(rows) = stmt.query_map([], |row| {
                                Ok((
                                    row.get::<_, String>(0)?, // their_jid
                                    row.get::<_, Option<String>>(1)?, // name
                                ))
                            }) {
                                for row in rows {
                                    if let Ok((jid, Some(name))) = row {
                                        contacts_map.insert(jid, name);
                                    }
                                }
                            }
                        }
                    }
                    contacts_map
                }).await;
                
                if let Ok(contacts_map) = contacts {
                    let mut cache = contact_cache.lock().await;
                    for (jid, name) in contacts_map {
                        cache.insert(jid, name);
                    }
                }
            });
        }
        
        // Try to get account info to verify authentication
        match client.get_me().await {
            Ok(jid) => {
                *client.my_jid.lock().await = Some(jid);
                
                // Check if we have any chats
                let chats = client.get_dialogs().await.unwrap_or_default();
                if chats.is_empty() {
                    println!();
                    println!("⚠️  No chats found. This is normal the first time!");
                    println!();
                    println!("WhatsApp needs to sync messages first. You have two options:");
                    println!();
                    println!("Option 1 (Recommended): Run sync manually in another terminal:");
                    println!("  {} --store {:?} sync", cli_path.display(), store_path);
                    println!();
                    println!("Option 2: Wait - the client will sync in the background, but it may take a while.");
                    println!("         Press Ctrl+C and run sync manually if you want faster results.");
                    println!();
                    println!("Press Enter to continue anyway, or Ctrl+C to exit and run sync first...");
                    use std::io;
                    let _ = io::stdin().read_line(&mut String::new());
                }
                
                // Start sync in background
                client.start_sync_background().await;
            }
            Err(_) => {
                println!();
                println!("❌ WhatsApp not authenticated!");
                println!();
                println!("Please run:");
                println!("  {} --store {:?} auth", cli_path.display(), store_path);
                println!();
                println!("Then scan the QR code with your phone.");
                println!();
            }
        }
        
        Ok(client)
    }
    
    pub async fn get_me(&self) -> Result<String> {
        // Try to get chats list to verify authentication
        // We'll extract our own JID from messages later
        let output = Command::new(&self.cli_path)
            .args(&["--store", &self.store_path.to_string_lossy(), "chats", "list", "--limit", "1"])
            .output()?;
        
        if !output.status.success() {
            anyhow::bail!("Not authenticated. Run: {} auth", self.cli_path.display());
        }
        
        let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
        
        if !response.success {
            anyhow::bail!("Failed to verify authentication: {:?}", response.error);
        }
        
        // For now, return a placeholder - we'll get the real JID from messages
        // WhatsApp JID format: phone@s.whatsapp.net
        // We'll extract it from messages when we receive them
        Ok("unknown@s.whatsapp.net".to_string())
    }
    
    pub async fn get_dialogs(&self) -> Result<Vec<ChatInfo>> {
        crate::debug_log!("get_dialogs: Requesting chat list");
        
        let output = Command::new(&self.cli_path)
            .args(&["--store", &self.store_path.to_string_lossy(), "chats", "list"])
            .output()?;
        
        if !output.status.success() {
            crate::warn_log!("get_dialogs: Command failed: {:?}", output.status);
            return Ok(Vec::new());
        }
        
        let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
        
        if !response.success {
            crate::warn_log!("get_dialogs: Response not successful: {:?}", response.error);
            return Ok(Vec::new());
        }
        
        let mut chats = Vec::new();
        let mut seen_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut temp_chats: Vec<ChatListItem> = Vec::new();
        
        if let Some(data) = response.data {
            if let Some(chats_array) = data.as_array() {
                crate::debug_log!("get_dialogs: Got {} chats from API", chats_array.len());
                
                // First pass: collect all chats and filter obvious junk
                for chat_val in chats_array {
                    if let Ok(chat) = serde_json::from_value::<ChatListItem>(chat_val.clone()) {
                        // Filter out junk chats
                        // Skip if name is just a JID (phone@s.whatsapp.net or similar)
                        if chat.name.contains("@s.whatsapp.net") || chat.name.contains("@lid") {
                            crate::debug_log!("get_dialogs: Skipping junk chat with name '{}'", chat.name);
                            continue;
                        }
                        
                        // Skip if name is just "Q" or single letter followed by @
                        if chat.name.len() <= 2 && chat.name.contains("@") {
                            crate::debug_log!("get_dialogs: Skipping junk chat with name '{}'", chat.name);
                            continue;
                        }
                        
                        temp_chats.push(chat);
                    } else {
                        crate::warn_log!("get_dialogs: Failed to parse chat item: {:?}", chat_val);
                    }
                }
                
                // Second pass: filter out @lid chats if we have a @s.whatsapp.net version
                // Build a map of @s.whatsapp.net chats
                let mut phone_chats: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                for chat in &temp_chats {
                    if chat.jid.ends_with("@s.whatsapp.net") {
                        phone_chats.insert(chat.name.clone(), chat.jid.clone());
                    }
                }
                
                // Now process chats, skipping @lid if we have a phone version
                for chat in temp_chats {
                    // Skip @lid chats if we have a @s.whatsapp.net version with same or similar name
                    if chat.jid.ends_with("@lid") {
                        if phone_chats.contains_key(&chat.name) {
                            crate::debug_log!("get_dialogs: Skipping @lid chat '{}' - have @s.whatsapp.net version", chat.name);
                            continue;
                        }
                        // Also check if any phone chat name starts with this name (e.g., "P" vs "Patrik Wellner")
                        let mut found_match = false;
                        for phone_name in phone_chats.keys() {
                            if phone_name.starts_with(&chat.name) || chat.name.starts_with(phone_name) {
                                crate::debug_log!("get_dialogs: Skipping @lid chat '{}' - similar to '{}'", chat.name, phone_name);
                                found_match = true;
                                break;
                            }
                        }
                        if found_match {
                            continue;
                        }
                    }
                        
                        // Deduplicate: if we've seen this name before, keep the one with more unread or the group
                        if let Some(existing_jid) = seen_names.get(&chat.name) {
                            // Find the existing chat
                            if let Some(existing_idx) = chats.iter().position(|c: &ChatInfo| c.id == *existing_jid) {
                                let existing = &chats[existing_idx];
                                // Prefer groups over individual chats, or higher unread count
                                let is_group = chat.jid.ends_with("@g.us");
                                let keep_new = is_group && !existing.is_group || 
                                               (is_group == existing.is_group && chat.unread > existing.unread);
                                
                                if keep_new {
                                    crate::debug_log!("get_dialogs: Replacing duplicate '{}': {} -> {}", 
                                        chat.name, existing_jid, chat.jid);
                                    chats.remove(existing_idx);
                                    seen_names.insert(chat.name.clone(), chat.jid.clone());
                                } else {
                                    crate::debug_log!("get_dialogs: Skipping duplicate '{}': keeping {}", 
                                        chat.name, existing_jid);
                                    continue;
                                }
                            }
                        } else {
                            seen_names.insert(chat.name.clone(), chat.jid.clone());
                        }
                        
                        // Determine if it's a group (group JIDs end with @g.us)
                        let is_group = chat.jid.ends_with("@g.us");
                        
                    chats.push(ChatInfo {
                        id: chat.jid.clone(),
                        name: chat.name.clone(),
                        username: None, // WhatsApp doesn't have usernames
                        unread: chat.unread,
                        _is_channel: false,
                        is_group,
                    });
                    crate::debug_log!("get_dialogs: Chat {}: '{}' (unread={}, is_group={})", 
                        chat.jid, chat.name, chat.unread, is_group);
                }
            } else {
                crate::warn_log!("get_dialogs: Response data is not an array");
            }
        } else {
            crate::warn_log!("get_dialogs: No data in response");
        }
        
        crate::debug_log!("get_dialogs: Returning {} chats after filtering", chats.len());
        Ok(chats)
    }
    
    pub async fn get_messages(
        &self,
        chat_jid: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, String, String, Option<String>, Option<String>, std::collections::HashMap<String, u32>, i64)>> {
        crate::debug_log!("get_messages: Requesting {} messages for chat {}", limit, chat_jid);
        
        // Get chat name for better matching (since @lid and @s.whatsapp.net might have different IDs)
        let chat_name = self.get_dialogs().await?
            .iter()
            .find(|c| c.id == chat_jid)
            .map(|c| c.name.clone());
        crate::debug_log!("get_messages: Chat name for {} is {:?}", chat_jid, chat_name);
        
        // CRITICAL: whatsapp-cli --chat flag doesn't work correctly for groups
        // For groups, read directly from SQLite database since whatsapp-cli doesn't return them correctly
        // For individual chats, use whatsapp-cli (even though broken, we filter by name)
        let is_group = chat_jid.ends_with("@g.us");
        
        if is_group {
            // Read directly from SQLite database for groups
            return self.get_messages_from_db(chat_jid, limit, chat_name).await;
        }
        
        // For individual chats, use whatsapp-cli (even though broken, we filter by name)
        let store_path_str = self.store_path.to_string_lossy().to_string();
        let limit_str = limit.to_string();
        
        let mut cmd = Command::new(&self.cli_path);
        cmd.args(&[
            "--store", &store_path_str,
            "messages", "list",
            "--chat", chat_jid,
            "--limit", &limit_str,
        ]);
        
        let output = cmd.output()?;
        
        if !output.status.success() {
            crate::warn_log!("get_messages: Command failed for chat {}: {:?}", chat_jid, output.status);
            return Ok(Vec::new());
        }
        
        let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
        
        if !response.success {
            crate::warn_log!("get_messages: Response not successful for chat {}: {:?}", chat_jid, response.error);
            return Ok(Vec::new());
        }
        
        let mut messages = Vec::new();
        
        if let Some(data) = response.data {
            if let Some(msgs_array) = data.as_array() {
                crate::debug_log!("get_messages: Got {} raw messages from whatsapp-cli for chat {}", msgs_array.len(), chat_jid);
                let mut filtered_count = 0;
                let mut chat_jids_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                for msg_val in msgs_array {
                    if let Ok(msg) = serde_json::from_value::<MessageItem>(msg_val.clone()) {
                        chat_jids_seen.insert(msg.chat_jid.clone());
                        // CRITICAL: Filter messages by chat_jid since whatsapp-cli --chat flag doesn't work correctly
                        // Handle @lid vs @s.whatsapp.net - they represent the same chat but have different JIDs
                        let msg_chat_matches = if msg.chat_jid == chat_jid {
                            true
                        } else {
                            // Check if both are individual chats (not groups) and might be the same person
                            let requested_is_individual = !chat_jid.ends_with("@g.us");
                            let msg_is_individual = !msg.chat_jid.ends_with("@g.us");
                            
                            if requested_is_individual && msg_is_individual {
                                // For individual chats, be more lenient:
                                // Match by name if names are similar (e.g., "P" vs "Patrik Wellner")
                                let both_have_names = chat_name.is_some() && msg.chat_name.is_some();
                                
                                if both_have_names {
                                    let req_name = chat_name.as_ref().unwrap();
                                    let msg_name = msg.chat_name.as_ref().unwrap();
                                    
                                    // Allow if names match (case-insensitive, trimmed)
                                    let names_match = req_name.trim().eq_ignore_ascii_case(msg_name.trim());
                                    
                                    // Also allow if one name is a substring of the other (e.g., "P" vs "Patrik Wellner")
                                    let names_related = req_name.trim().to_lowercase().contains(&msg_name.trim().to_lowercase())
                                        || msg_name.trim().to_lowercase().contains(&req_name.trim().to_lowercase());
                                    
                                    if names_match || names_related {
                                        crate::debug_log!("get_messages: Matching individual chat by name: '{}'/'{}' (requested: {}, message: {})", 
                                            req_name, msg_name, chat_jid, msg.chat_jid);
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    // No names to match, require exact JID
                                    false
                                }
                            } else {
                                // Groups or mixed - require exact JID match (whatsapp-cli should return correct messages for groups)
                                // But if it doesn't, we still need to filter strictly
                                false
                            }
                        };
                        
                        if !msg_chat_matches {
                            filtered_count += 1;
                            if filtered_count <= 5 {
                                crate::debug_log!("get_messages: Filtering out message from chat {} (requested: {}, msg_name: {:?}, requested_name: {:?})", 
                                    msg.chat_jid, chat_jid, msg.chat_name, chat_name);
                            }
                            continue; // Skip messages that don't belong to this chat
                        }
                        
                        // Format: (msg_id, sender_jid, sender_name, text, reply_to_id, media_type, reactions, timestamp)
                        // Try to get sender name from various sources, including contacts database
                        let sender_name = if msg.from_me {
                            "You".to_string()
                        } else {
                            // First try to get from contact cache (which we'll populate from contacts DB)
                            let cache = self.contact_cache.lock().await;
                            if let Some(cached_name) = cache.get(&msg.sender) {
                                cached_name.clone()
                            } else if let Some(name) = msg.sender_name {
                                name
                            } else if let Some(chat_name) = &msg.chat_name {
                                // For individual chats, use chat_name as sender name
                                if !msg.chat_jid.ends_with("@g.us") {
                                    chat_name.clone()
                                } else {
                                    format_phone_number(&msg.sender)
                                }
                            } else {
                                format_phone_number(&msg.sender)
                            }
                        };
                        
                        // Parse timestamp from string (format: "2024-01-01T12:00:00Z" or similar)
                        let timestamp = msg.timestamp.parse::<i64>()
                            .or_else(|_| {
                                // Try parsing as ISO 8601 datetime
                                chrono::DateTime::parse_from_rfc3339(&msg.timestamp)
                                    .map(|dt| dt.timestamp())
                                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(&msg.timestamp, "%Y-%m-%d %H:%M:%S")
                                        .map(|dt| dt.and_utc().timestamp()))
                            })
                            .unwrap_or_else(|_| {
                                // Fallback to current time if parsing fails
                                chrono::Utc::now().timestamp()
                            });
                        
                        let media_type = msg.media_type.clone();
                        messages.push((
                            msg.id,
                            msg.sender,
                            sender_name,
                            msg.content,
                            None, // reply_to_id - TODO: extract from message
                            media_type, // media_type
                            std::collections::HashMap::new(), // reactions - TODO: extract reactions
                            timestamp, // timestamp
                        ));
                    } else {
                        crate::warn_log!("get_messages: Failed to parse message item: {:?}", msg_val);
                    }
                }
                crate::debug_log!("get_messages: Found messages from {} different chats: {:?}", chat_jids_seen.len(), chat_jids_seen);
                if filtered_count > 0 {
                    crate::debug_log!("get_messages: Filtered out {} messages that didn't match chat {} (kept {})", filtered_count, chat_jid, messages.len());
                }
                
                // If this is a group chat and we got 0 messages, try to force sync
                if chat_jid.ends_with("@g.us") && messages.is_empty() {
                    let has_group_messages = chat_jids_seen.iter().any(|jid| jid.ends_with("@g.us"));
                    if !has_group_messages {
                        crate::warn_log!("get_messages: No group messages found in database for chat {}. whatsapp-cli may not have synced historical messages from this group.", chat_jid);
                        crate::warn_log!("get_messages: Note: whatsapp-cli sync only syncs NEW messages, not historical ones. Group messages will appear once new messages arrive.");
                        // Force sync for this group by running sync in background (in case there are new messages)
                        self.force_sync_group(chat_jid).await;
                        // Wait longer for sync to fetch messages (groups may take more time)
                        tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;
                        // Try fetching messages again by re-running the same logic (but not recursively)
                        // We'll fetch again with the same parameters
                        let fetch_limit = (limit * 50).max(2000);
                        let store_path_str = self.store_path.to_string_lossy().to_string();
                        let limit_str = fetch_limit.to_string();
                        
                        let output = Command::new(&self.cli_path)
                            .args(&[
                                "--store", &store_path_str,
                                "messages", "list",
                                "--limit", &limit_str,
                            ])
                            .output()?;
                        
                        if output.status.success() {
                            let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
                            if let Some(data) = response.data {
                                if let Some(msgs_array) = data.as_array() {
                                    let mut retry_messages = Vec::new();
                                    let mut retry_chat_jids_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                                    
                                    for msg_val in msgs_array {
                                        if let Ok(msg) = serde_json::from_value::<MessageItem>(msg_val.clone()) {
                                            retry_chat_jids_seen.insert(msg.chat_jid.clone());
                                            if msg.chat_jid == chat_jid {
                                                // Same parsing logic as above...
                                                let sender_name = if msg.from_me {
                                                    "You".to_string()
                                                } else if let Some(name) = msg.sender_name {
                                                    name
                                                } else if let Some(chat_name) = &msg.chat_name {
                                                    if !msg.chat_jid.ends_with("@g.us") {
                                                        chat_name.clone()
                                                    } else {
                                                        let cache = self.contact_cache.lock().await;
                                                        cache.get(&msg.sender)
                                                            .cloned()
                                                            .unwrap_or_else(|| format_phone_number(&msg.sender))
                                                    }
                                                } else {
                                                    let cache = self.contact_cache.lock().await;
                                                    cache.get(&msg.sender)
                                                        .cloned()
                                                        .unwrap_or_else(|| format_phone_number(&msg.sender))
                                                };
                                                
                                                let timestamp = msg.timestamp.parse::<i64>()
                                                    .unwrap_or_else(|_| chrono::Utc::now().timestamp());
                                                
                                                let media_type = msg.media_type.clone();
                                                retry_messages.push((
                                                    msg.id,
                                                    msg.sender,
                                                    sender_name,
                                                    msg.content,
                                                    None,
                                                    media_type,
                                                    std::collections::HashMap::new(),
                                                    timestamp,
                                                ));
                                            }
                                        }
                                    }
                                    
                                    if !retry_messages.is_empty() {
                                        crate::info_log!("get_messages: Found {} messages after sync for group {}", retry_messages.len(), chat_jid);
                                        retry_messages.sort_by_key(|m| m.7);
                                        if retry_messages.len() > limit {
                                            retry_messages.reverse();
                                            retry_messages.truncate(limit);
                                            retry_messages.reverse();
                                        }
                                        messages = retry_messages;
                                    }
                                }
                            }
                        }
                    }
                }
                
                // Sort by timestamp (oldest first) and take only the requested limit
                if messages.len() > limit {
                    // Keep only the most recent messages
                    messages.sort_by_key(|m| m.7); // Sort by timestamp (oldest first)
                    messages.reverse(); // Reverse to get newest first
                    messages.truncate(limit); // Take only limit
                    messages.reverse(); // Reverse back to oldest-first
                    crate::debug_log!("get_messages: Trimmed to {} most recent messages", limit);
                } else {
                    // Still sort by timestamp even if we don't need to truncate
                    messages.sort_by_key(|m| m.7);
                }
            } else {
                crate::warn_log!("get_messages: Response data is not an array for chat {}", chat_jid);
            }
        } else {
            crate::warn_log!("get_messages: No data in response for chat {}", chat_jid);
        }
        
        crate::debug_log!("get_messages: Returning {} messages for chat {}", messages.len(), chat_jid);
        if !messages.is_empty() {
            let first_chat = &messages[0].2; // sender_name
            let first_text_preview = if messages[0].3.len() > 30 {
                format!("{}...", &messages[0].3[..30])
            } else {
                messages[0].3.clone()
            };
            crate::debug_log!("get_messages: First message preview: sender={}, text='{}'", first_chat, first_text_preview);
        }
        Ok(messages)
    }
    
    pub async fn send_message(&self, chat_jid: &str, text: &str) -> Result<()> {
        let output = Command::new(&self.cli_path)
            .args(&[
                "--store", &self.store_path.to_string_lossy(),
                "send",
                "--to", chat_jid,
                "--message", text,
            ])
            .output()?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to send message: {}", stderr);
        }
        
        let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
        
        if !response.success {
            anyhow::bail!("Failed to send message: {:?}", response.error);
        }
        
        Ok(())
    }
    
    pub async fn reply_to_message(
        &self,
        chat_jid: &str,
        _message_id: &str,
        text: &str,
    ) -> Result<()> {
        // WhatsApp CLI doesn't have a direct reply command, so we send a regular message
        // TODO: Check if whatsapp-cli supports --reply-to flag
        let output = Command::new(&self.cli_path)
            .args(&[
                "--store", &self.store_path.to_string_lossy(),
                "send",
                "--to", chat_jid,
                "--message", text,
            ])
            .output()?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to send reply: {}", stderr);
        }
        
        let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
        
        if !response.success {
            anyhow::bail!("Failed to send reply: {:?}", response.error);
        }
        
        Ok(())
    }
    
    pub async fn edit_message(
        &self,
        _chat_jid: &str,
        _message_id: &str,
        _new_text: &str,
    ) -> Result<()> {
        // WhatsApp doesn't support editing messages
        anyhow::bail!("WhatsApp does not support editing messages")
    }
    
    pub async fn delete_message(&self, _chat_jid: &str, _message_id: &str) -> Result<()> {
        // WhatsApp CLI doesn't support deleting messages yet
        anyhow::bail!("Message deletion is not supported by whatsapp-cli yet")
    }
    
    pub async fn resolve_username(&self, phone: &str) -> Result<Option<(String, String, bool)>> {
        // WhatsApp uses phone numbers, not usernames
        // Format: +1234567890 -> 1234567890@s.whatsapp.net
        let clean_phone = phone.trim_start_matches('+').replace(['-', ' ', '(', ')'], "");
        let jid = format!("{}@s.whatsapp.net", clean_phone);
        
        // Try to get chat info
        let chats = self.get_dialogs().await?;
        if let Some(chat) = chats.iter().find(|c| c.id == jid) {
            Ok(Some((chat.id.clone(), chat.name.clone(), chat.is_group)))
        } else {
            // Create JID anyway - it might be a valid contact
            Ok(Some((jid.clone(), phone.to_string(), false)))
        }
    }
    
    pub async fn search_messages(
        &self,
        chat_jid: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, String, String, Option<String>, std::collections::HashMap<String, u32>)>> {
        // WhatsApp CLI search doesn't support --chat filter, so we search all and filter manually
        let output = Command::new(&self.cli_path)
            .args(&[
                "--store", &self.store_path.to_string_lossy(),
                "messages", "search",
                "--query", query,
                "--limit", &limit.to_string(),
            ])
            .output()?;
        
        if !output.status.success() {
            return Ok(Vec::new());
        }
        
        let response: WhatsAppResponse = serde_json::from_slice(&output.stdout)?;
        
        if !response.success {
            return Ok(Vec::new());
        }
        
        let mut messages = Vec::new();
        
        if let Some(data) = response.data {
            if let Some(msgs_array) = data.as_array() {
                for msg_val in msgs_array {
                    if let Ok(msg) = serde_json::from_value::<MessageItem>(msg_val.clone()) {
                        // Filter by chat_jid since search doesn't support --chat
                        if msg.chat_jid == chat_jid {
                            // Use same logic as get_messages for sender name
                            let sender_name = if msg.from_me {
                                "You".to_string()
                            } else if let Some(name) = msg.sender_name {
                                name
                            } else if let Some(chat_name) = &msg.chat_name {
                                if !msg.chat_jid.ends_with("@g.us") {
                                    chat_name.clone()
                                } else {
                                    let cache = self.contact_cache.lock().await;
                                    cache.get(&msg.sender)
                                        .cloned()
                                        .unwrap_or_else(|| format_phone_number(&msg.sender))
                                }
                            } else {
                                let cache = self.contact_cache.lock().await;
                                cache.get(&msg.sender)
                                    .cloned()
                                    .unwrap_or_else(|| format_phone_number(&msg.sender))
                            };
                            
                            messages.push((
                                msg.id,
                                msg.sender,
                                sender_name,
                                msg.content,
                                None, // reply_to_id
                                std::collections::HashMap::new(), // reactions
                            ));
                        }
                    }
                }
            }
        }
        
        Ok(messages)
    }
    
    #[allow(dead_code)]
    pub async fn get_message_sender(
        &self,
        _chat_jid: &str,
        message_id: &str,
    ) -> Result<Option<String>> {
        // Extract sender from message ID or return None
        // WhatsApp message IDs contain sender info
        Ok(Some(message_id.to_string()))
    }
    
    pub async fn download_media_by_id(
        &self,
        chat_jid: &str,
        message_id: &str,
        path: &std::path::Path,
    ) -> Result<String> {
        // Use whatsapp-cli media download command
        let output = Command::new(&self.cli_path)
            .arg("--store")
            .arg(&self.store_path)
            .arg("media")
            .arg("download")
            .arg("--message-id")
            .arg(message_id)
            .arg("--chat")
            .arg(chat_jid)
            .arg("--output")
            .arg(path)
            .output()?;

        if !output.status.success() {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Media download failed: {}", error_msg);
        }

        // Parse JSON response
        let stdout = String::from_utf8_lossy(&output.stdout);
        let response: WhatsAppResponse = serde_json::from_str(&stdout)?;

        if !response.success {
            let error = response.error.unwrap_or_else(|| "Unknown error".to_string());
            anyhow::bail!("Media download failed: {}", error);
        }

        // Extract path from response
        if let Some(data) = response.data {
            if let Some(path_value) = data.get("path") {
                if let Some(path_str) = path_value.as_str() {
                    return Ok(path_str.to_string());
                }
            }
        }

        anyhow::bail!("Media download succeeded but no path in response")
    }
    
    pub async fn create_group(&self, _title: &str, _user_jids: Vec<String>) -> Result<String> {
        // TODO: Implement group creation via whatsapp-cli
        anyhow::bail!("Group creation not yet implemented")
    }
    
    pub async fn add_member(&self, _chat_jid: &str, _phone: &str) -> Result<()> {
        // TODO: Implement add member via whatsapp-cli
        anyhow::bail!("Add member not yet implemented")
    }
    
    pub async fn remove_member(&self, _chat_jid: &str, _phone: &str) -> Result<()> {
        // TODO: Implement remove member via whatsapp-cli
        anyhow::bail!("Remove member not yet implemented")
    }
    
    pub async fn get_members(&self, _chat_jid: &str) -> Result<Vec<(String, String, String)>> {
        // TODO: Implement get members via whatsapp-cli
        // Returns (jid, name, role)
        Ok(Vec::new())
    }
    
    /// Get messages directly from SQLite database for groups
    async fn get_messages_from_db(
        &self,
        chat_jid: &str,
        limit: usize,
        _chat_name: Option<String>,
    ) -> Result<Vec<(String, String, String, String, Option<String>, Option<String>, std::collections::HashMap<String, u32>, i64)>> {
        let db_path = self.store_path.join("messages.db");
        let contacts_db_path = self.store_path.join("whatsapp.db");
        
        if !db_path.exists() {
            crate::warn_log!("get_messages_from_db: Database not found at {:?}", db_path);
            return Ok(Vec::new());
        }
        
        // Open database connection (we need to do this in a blocking task)
        let db_path_clone = db_path.clone();
        let contacts_db_path_clone = contacts_db_path.clone();
        let chat_jid_clone = chat_jid.to_string();
        let limit_clone = limit * 2; // Get more to account for filtering out reactions
        let contact_cache = self.contact_cache.clone();
        
        let (messages, contacts_map) = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path_clone)?;
            
            // Load contacts from whatsapp.db if it exists
            let mut contacts_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            if contacts_db_path_clone.exists() {
                if let Ok(contacts_conn) = Connection::open(&contacts_db_path_clone) {
                    let mut contacts_stmt = contacts_conn.prepare(
                        "SELECT their_jid, COALESCE(NULLIF(full_name, ''), NULLIF(first_name, ''), NULLIF(push_name, ''), NULLIF(business_name, '')) as name 
                         FROM whatsmeow_contacts 
                         WHERE name IS NOT NULL AND name != ''"
                    )?;
                    
                    let contacts_rows = contacts_stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?, // their_jid
                            row.get::<_, Option<String>>(1)?, // name
                        ))
                    })?;
                    
                    for contact_row in contacts_rows {
                        if let Ok((jid, Some(name))) = contact_row {
                            contacts_map.insert(jid, name);
                        }
                    }
                }
            }
            
            let mut stmt = conn.prepare(
                "SELECT id, sender, content, timestamp, is_from_me, media_type 
                 FROM messages 
                 WHERE chat_jid = ? 
                 ORDER BY timestamp DESC 
                 LIMIT ?"
            )?;
            
            let rows = stmt.query_map(params![chat_jid_clone, limit_clone], |row| {
                Ok((
                    row.get::<_, String>(0)?, // id
                    row.get::<_, String>(1)?, // sender
                    row.get::<_, Option<String>>(2)?, // content
                    row.get::<_, String>(3)?, // timestamp
                    row.get::<_, bool>(4)?, // is_from_me
                    row.get::<_, Option<String>>(5)?, // media_type
                ))
            })?;
            
            let mut messages = Vec::new();
            for row in rows {
                let (id, sender, content, timestamp_str, is_from_me, media_type) = row?;
                
                // Get content string
                let content_str = content.unwrap_or_default();
                
                // Skip reactions in GROUP chats: empty content or double braces (unless it has media)
                let has_media = media_type.is_some();
                let trimmed = content_str.trim();
                let is_reaction = !has_media && (
                    trimmed.is_empty() || 
                    (trimmed.starts_with("{{") && trimmed.ends_with("}}"))
                );
                
                crate::debug_log!("DB message check: content='{}', len={}, starts_with={{={{: {}, ends_with=}}={}, has_media={}, is_reaction={}", 
                    if trimmed.len() > 50 { &trimmed[..50] } else { trimmed },
                    trimmed.len(),
                    trimmed.starts_with("{{"),
                    trimmed.ends_with("}}"),
                    has_media,
                    is_reaction
                );
                
                if is_reaction {
                    crate::debug_log!("Filtering out reaction message");
                    continue;
                }
                
                // Parse timestamp - try different formats
                let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp_str)
                    .map(|dt| dt.timestamp())
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(&timestamp_str, "%Y-%m-%d %H:%M:%S%.f%z")
                            .map(|dt| dt.and_utc().timestamp())
                    })
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(&timestamp_str, "%Y-%m-%d %H:%M:%S%z")
                            .map(|dt| dt.and_utc().timestamp())
                    })
                    .unwrap_or_else(|_| chrono::Utc::now().timestamp());
                
                // Get sender name from contacts map
                let sender_name = if is_from_me {
                    "You".to_string()
                } else {
                    contacts_map.get(&sender)
                        .cloned()
                        .unwrap_or_else(|| format_phone_number(&sender))
                };
                
                messages.push((
                    id,
                    sender,
                    sender_name,
                    content_str,
                    None, // reply_to_id
                    media_type, // media_type
                    std::collections::HashMap::new(), // reactions
                    timestamp,
                ));
            }
            
            // Reverse to get oldest first
            messages.reverse();
            
            Ok::<(Vec<_>, std::collections::HashMap<String, String>), rusqlite::Error>((messages, contacts_map))
        }).await??;
        
        // Update contact cache with names we found
        {
            let mut contact_cache = contact_cache.lock().await;
            for (jid, name) in contacts_map {
                contact_cache.insert(jid, name);
            }
        }
        
        crate::debug_log!("get_messages_from_db: Found {} messages for group {} (after filtering reactions)", messages.len(), chat_jid);
        Ok(messages)
    }
    
    pub async fn forward_message(
        &self,
        _from_chat_jid: &str,
        _message_id: &str,
        _to_chat_jid: &str,
    ) -> Result<()> {
        // TODO: Implement forward message via whatsapp-cli
        anyhow::bail!("Forward message not yet implemented")
    }
    
    /// Force sync for a specific group chat
    async fn force_sync_group(&self, chat_jid: &str) {
        let cli_path = self.cli_path.clone();
        let store_path = self.store_path.clone();
        let chat_jid = chat_jid.to_string();
        
        tokio::spawn(async move {
            crate::info_log!("force_sync_group: Starting sync for group {}", chat_jid);
            // Run sync for longer to fetch messages from this group
            // whatsapp-cli sync runs continuously, so we'll kill it after enough time
            let mut sync_process = match TokioCommand::new(&cli_path)
                .arg("--store")
                .arg(&store_path)
                .arg("sync")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(p) => p,
                Err(e) => {
                    crate::warn_log!("force_sync_group: Failed to start sync: {}", e);
                    return;
                }
            };
            
            // Wait longer for sync to fetch messages (groups may take more time)
            // Check periodically if messages have been synced
            for _ in 0..10 {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                
                // Check if messages exist for this group
                let check_output = match Command::new(&cli_path)
                    .arg("--store")
                    .arg(&store_path)
                    .arg("messages")
                    .arg("list")
                    .arg("--limit")
                    .arg("100")
                    .output()
                {
                    Ok(o) => o,
                    Err(_) => continue,
                };
                
                if check_output.status.success() {
                    if let Ok(response) = serde_json::from_slice::<WhatsAppResponse>(&check_output.stdout) {
                        if let Some(data) = response.data {
                            if let Some(msgs_array) = data.as_array() {
                                let has_group_msgs = msgs_array.iter().any(|msg_val| {
                                    if let Ok(msg) = serde_json::from_value::<MessageItem>(msg_val.clone()) {
                                        msg.chat_jid == chat_jid
                                    } else {
                                        false
                                    }
                                });
                                
                                if has_group_msgs {
                                    crate::info_log!("force_sync_group: Found messages for group {}, stopping sync", chat_jid);
                                    let _ = sync_process.kill().await;
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            
            // Kill the sync process after timeout
            let _ = sync_process.kill().await;
            crate::info_log!("force_sync_group: Sync completed for group {} (timeout reached)", chat_jid);
        });
    }
    
    /// Start sync process in background
    async fn start_sync_background(&self) {
        let cli_path = self.cli_path.clone();
        let store_path = self.store_path.clone();
        let pending_updates = self.pending_updates.clone();
        let last_synced_message_id = self.last_synced_message_id.clone();
        let my_jid = self.my_jid.clone();
        let contact_cache = self.contact_cache.clone();
        
        tokio::spawn(async move {
            // Start whatsapp-cli sync in background
            let mut sync_process = match TokioCommand::new(&cli_path)
                .arg("--store")
                .arg(&store_path)
                .arg("sync")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Failed to start whatsapp-cli sync: {}", e);
                    return;
                }
            };
            
            // Wait a bit for initial sync to settle before we start polling
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            
            // Poll for new messages periodically (less frequently to avoid race conditions)
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            crate::info_log!("Sync background process started");
            
            loop {
                interval.tick().await;
                crate::debug_log!("Sync: Polling for new messages");
                
                // Check if sync process is still running
                if let Ok(Some(status)) = sync_process.try_wait() {
                    if !status.success() {
                        crate::error_log!("WhatsApp sync process exited with error: {:?}", status);
                        // Try to restart
                        match TokioCommand::new(&cli_path)
                            .arg("--store")
                            .arg(&store_path)
                            .arg("sync")
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                        {
                            Ok(p) => {
                                crate::info_log!("Sync: Restarted sync process");
                                sync_process = p;
                                // Wait a bit after restart
                                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            },
                            Err(e) => {
                                crate::error_log!("Failed to restart sync: {}", e);
                                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                continue;
                            }
                        }
                    }
                }
                
                // Poll for new messages - get latest messages across all chats
                // Use a small delay to let sync process finish writing
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                
                let output = match Command::new(&cli_path)
                    .arg("--store")
                    .arg(&store_path)
                    .arg("messages")
                    .arg("list")
                    .arg("--limit")
                    .arg("20")
                    .output()
                {
                    Ok(o) => o,
                    Err(e) => {
                        crate::warn_log!("Sync: Failed to execute messages list command: {}", e);
                        continue;
                    },
                };
                
                if !output.status.success() {
                    crate::warn_log!("Sync: messages list command failed with status: {:?}", output.status);
                    continue;
                }
                
                let stdout = match String::from_utf8(output.stdout) {
                    Ok(s) => s,
                    Err(e) => {
                        crate::warn_log!("Sync: Failed to parse stdout: {}", e);
                        continue;
                    },
                };
                
                let response: WhatsAppResponse = match serde_json::from_str(&stdout) {
                    Ok(r) => r,
                    Err(e) => {
                        crate::warn_log!("Sync: Failed to parse JSON response: {}", e);
                        continue;
                    },
                };
                
                if !response.success {
                    crate::warn_log!("Sync: Response not successful: {:?}", response.error);
                    continue;
                }
                
                if let Some(data) = response.data {
                    if let Some(messages) = data.as_array() {
                        crate::debug_log!("Sync: Checking {} messages for new ones", messages.len());
                        let last_id = last_synced_message_id.lock().await.clone();
                        crate::debug_log!("Sync: Last synced message ID: {:?}", last_id);
                        
                        // Process messages in reverse order (newest first)
                        let mut new_message_count = 0;
                        let mut newest_message_id: Option<String> = None;
                        
                        for msg_json in messages.iter().rev() {
                            if let Some(msg) = Self::parse_message_item(msg_json) {
                                // Track the newest message ID (first one we see in reverse order)
                                if newest_message_id.is_none() {
                                    newest_message_id = Some(msg.id.clone());
                                }
                                
                                // Check if this is a new message
                                if last_id.as_ref().map_or(true, |id| &msg.id != id) {
                                    new_message_count += 1;
                                    crate::debug_log!("Sync: Found new message: id={}, chat={}, sender={}, text_len={}, from_me={}", 
                                        msg.id, msg.chat_jid, msg.sender, msg.content.len(), msg.from_me);
                                    // This is a new message
                                    // Use same logic as get_messages for sender name
                                    let sender_name = if msg.from_me {
                                        "You".to_string()
                                    } else if let Some(name) = msg.sender_name {
                                        name
                                    } else if let Some(chat_name) = &msg.chat_name {
                                        if !msg.chat_jid.ends_with("@g.us") {
                                            chat_name.clone()
                                        } else {
                                            let cache = contact_cache.lock().await;
                                            cache.get(&msg.sender)
                                                .cloned()
                                                .unwrap_or_else(|| format_phone_number(&msg.sender))
                                        }
                                    } else {
                                        let cache = contact_cache.lock().await;
                                        cache.get(&msg.sender)
                                            .cloned()
                                            .unwrap_or_else(|| format_phone_number(&msg.sender))
                                    };
                                    
                                    // Update our JID if this is an outgoing message
                                    if msg.from_me {
                                        let mut my_jid_guard = my_jid.lock().await;
                                        if my_jid_guard.is_none() || my_jid_guard.as_ref().unwrap() == "unknown@s.whatsapp.net" {
                                            *my_jid_guard = Some(msg.sender.clone());
                                        }
                                    }
                                    
                                    let update = WhatsAppUpdate::NewMessage {
                                        chat_jid: msg.chat_jid.clone(),
                                        sender_name,
                                        text: msg.content.clone(),
                                        is_outgoing: msg.from_me,
                                    };
                                    
                                    pending_updates.lock().await.push(update);
                                    crate::debug_log!("Sync: Added update to pending_updates queue");
                                    
                                    // Process all new messages, not just the first one
                                    // (but break after processing a batch to avoid overwhelming)
                                } else {
                                    crate::debug_log!("Sync: Message {} already synced, skipping", msg.id);
                                    // Found the last synced message, we can stop here
                                    break;
                                }
                            } else {
                                crate::warn_log!("Sync: Failed to parse message item");
                            }
                        }
                        
                        // Update last synced message ID to the newest message we saw (if any)
                        if let Some(newest_id) = newest_message_id {
                            *last_synced_message_id.lock().await = Some(newest_id.clone());
                            crate::debug_log!("Sync: Updated last_synced_message_id to {}", newest_id);
                        }
                        if new_message_count > 0 {
                            crate::info_log!("Sync: Found {} new messages", new_message_count);
                        }
                    } else {
                        crate::warn_log!("Sync: Response data is not an array");
                    }
                } else {
                    crate::warn_log!("Sync: No data in response");
                }
            }
        });
    }
    
    fn parse_message_item(value: &serde_json::Value) -> Option<MessageItem> {
        serde_json::from_value(value.clone()).ok()
    }
    
    /// Poll for updates - returns any pending updates
    pub async fn poll_updates(&self) -> Result<Vec<WhatsAppUpdate>> {
        let mut pending = self.pending_updates.lock().await;
        let updates = std::mem::take(&mut *pending);
        Ok(updates)
    }
}

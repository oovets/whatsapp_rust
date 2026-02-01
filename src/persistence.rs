use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

use crate::config::Config;
use crate::split_view::PaneNode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutData {
    pub panes: Vec<PaneState>,
    pub focused_pane: usize,
    #[serde(default)]
    pub pane_tree: Option<PaneNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneState {
    pub chat_id: Option<String>,
    pub chat_name: String,
    pub scroll_offset: usize,
    #[serde(default)]
    pub filter_type: Option<String>,
    #[serde(default)]
    pub filter_value: Option<String>,
}

impl LayoutData {
    pub fn new() -> Self {
        Self {
            panes: vec![PaneState {
                chat_id: None,
                chat_name: "No chat selected".to_string(),
                scroll_offset: 0,
                filter_type: None,
                filter_value: None,
            }],
            focused_pane: 0,
            pane_tree: None,
        }
    }

    pub fn load(config: &Config) -> Result<Self> {
        let path = config.layout_path();
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let layout: LayoutData = serde_json::from_str(&content)?;
            Ok(layout)
        } else {
            Ok(Self::new())
        }
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        let path = config.layout_path();
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

impl Default for LayoutData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Aliases {
    #[serde(flatten)]
    pub map: HashMap<String, String>, // user_jid -> alias
}

impl Aliases {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn load(config: &Config) -> Result<Self> {
        let path = config.aliases_path();
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let aliases: Aliases = serde_json::from_str(&content)?;
            Ok(aliases)
        } else {
            Ok(Self::new())
        }
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        let path = config.aliases_path();
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn _get(&self, user_jid: &str) -> Option<&String> {
        self.map.get(user_jid)
    }

    pub fn insert(&mut self, user_jid: String, alias: String) {
        self.map.insert(user_jid, alias);
    }

    pub fn remove(&mut self, user_jid: &String) -> Option<String> {
        self.map.remove(user_jid)
    }
}

impl Default for Aliases {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub settings: AppSettings,
    pub aliases: Aliases,
    pub layout: LayoutData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_true")]
    pub show_reactions: bool,
    
    #[serde(default = "default_true")]
    pub show_notifications: bool,
    
    #[serde(default)]
    pub compact_mode: bool,
    
    #[serde(default = "default_true")]
    pub show_emojis: bool,
    
    #[serde(default)]
    pub show_line_numbers: bool,
    
    #[serde(default = "default_true")]
    pub show_timestamps: bool,
    
    #[serde(default = "default_true")]
    pub show_user_colors: bool,
    
    #[serde(default = "default_true")]
    pub show_borders: bool,

    #[serde(default = "default_true")]
    pub show_chat_list: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_reactions: true,
            show_notifications: true,
            compact_mode: false,
            show_emojis: true,
            show_line_numbers: false,
            show_timestamps: true,
            show_user_colors: true,
            show_borders: true,
            show_chat_list: true,
        }
    }
}

fn default_true() -> bool {
    true
}

impl AppState {
    pub fn load(config: &Config) -> Result<Self> {
        Ok(Self {
            settings: AppSettings {
                show_reactions: config.settings.show_reactions,
                show_notifications: config.settings.show_notifications,
                compact_mode: config.settings.compact_mode,
                show_emojis: config.settings.show_emojis,
                show_line_numbers: config.settings.show_line_numbers,
                show_timestamps: config.settings.show_timestamps,
                show_user_colors: config.settings.show_user_colors,
                show_borders: config.settings.show_borders,
                show_chat_list: config.settings.show_chat_list,
            },
            aliases: Aliases::load(config)?,
            layout: LayoutData::load(config)?,
        })
    }

    pub fn _save(&self, config: &Config) -> Result<()> {
        self.aliases.save(config)?;
        self.layout.save(config)?;
        // Settings are saved as part of config
        Ok(())
    }
}

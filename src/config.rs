use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub whatsapp_cli_path: PathBuf,
    
    #[serde(default)]
    pub settings: Settings,
    
    #[serde(skip)]
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
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

impl Default for Settings {
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

impl Config {
    pub fn load() -> Result<Self> {
        let config_dir = Self::get_config_dir();
        let config_path = config_dir.join("whatsapp_config.json");

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let mut config: Config = serde_json::from_str(&content)?;
            config.config_dir = config_dir;
            
            // Expand relative paths to absolute
            if config.whatsapp_cli_path.is_relative() {
                if let Ok(absolute) = config.whatsapp_cli_path.canonicalize() {
                    config.whatsapp_cli_path = absolute;
                } else {
                    // Try to find it in common locations
                    if let Some(found) = Self::find_whatsapp_cli() {
                        config.whatsapp_cli_path = found.canonicalize().unwrap_or(found);
                    }
                }
            }
            
            Ok(config)
        } else {
            // Create new config
            let config = Self::create_new(config_dir)?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_path = self.config_dir.join("whatsapp_config.json");
        let content = serde_json::to_string_pretty(&self)?;
        fs::write(config_path, content)?;
        Ok(())
    }

    fn find_whatsapp_cli() -> Option<PathBuf> {
        // Try to find whatsapp-cli in common locations
        let candidates = vec![
            // In PATH
            "whatsapp-cli",
            // Common installation locations
            "~/.local/bin/whatsapp-cli",
            "/usr/local/bin/whatsapp-cli",
            "/opt/homebrew/bin/whatsapp-cli",
            // Current directory
            "./whatsapp-cli",
        ];
        
        for candidate in candidates {
            let path = if candidate.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(&candidate[2..])
                } else {
                    continue
                }
            } else if candidate.starts_with("./") {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(&candidate[2..])
            } else {
                // Try to find in PATH using std::env::var("PATH")
                let path_var = std::env::var("PATH").unwrap_or_default();
                let paths: Vec<&str> = if cfg!(target_os = "windows") {
                    path_var.split(';').collect()
                } else {
                    path_var.split(':').collect()
                };
                
                let mut found = None;
                for path_str in paths {
                    let test_path = PathBuf::from(path_str).join(candidate);
                    if test_path.exists() {
                        found = Some(test_path);
                        break;
                    }
                }
                
                if let Some(path) = found {
                    path
                } else {
                    continue
                }
            };
            
            if path.exists() {
                return Some(path);
            }
        }
        
        None
    }

    fn create_new(config_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&config_dir)?;

        println!("=== WhatsApp Client Setup ===");
        println!("This client uses whatsapp-cli (https://github.com/vicentereig/whatsapp-cli)");
        println!();
        
        // Try to find whatsapp-cli automatically
        let default_path = Self::find_whatsapp_cli()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "./whatsapp-cli".to_string());
        
        print!("Enter path to whatsapp-cli binary (default: {}): ", default_path);
        use std::io::{self, Write};
        io::stdout().flush()?;
        let mut cli_path_str = String::new();
        io::stdin().read_line(&mut cli_path_str)?;
        let cli_path_str = cli_path_str.trim();
        
        let whatsapp_cli_path = if cli_path_str.is_empty() {
            if let Some(found) = Self::find_whatsapp_cli() {
                found.canonicalize().unwrap_or(found)
            } else {
                PathBuf::from("./whatsapp-cli")
            }
        } else {
            // Expand ~ in user input
            let expanded = if cli_path_str.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(&cli_path_str[2..])
                } else {
                    PathBuf::from(cli_path_str)
                }
            } else {
                PathBuf::from(cli_path_str)
            };
            // Try to canonicalize to absolute path
            expanded.canonicalize().unwrap_or(expanded)
        };

        // Verify whatsapp-cli exists
        if !whatsapp_cli_path.exists() {
            println!();
            println!("Warning: whatsapp-cli not found at {:?}", whatsapp_cli_path);
            println!("Please download it from: https://github.com/vicentereig/whatsapp-cli");
            println!("Or build it with: go build -o whatsapp-cli .");
            println!();
            println!("If you already installed it, make sure it's in your PATH or provide the full path.");
            println!();
        } else {
            println!("Found whatsapp-cli at: {:?}", whatsapp_cli_path);
        }

        let config = Config {
            whatsapp_cli_path,
            settings: Settings::default(),
            config_dir,
        };

        config.save()?;
        Ok(config)
    }

    fn get_config_dir() -> PathBuf {
        // First check current directory
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let local_config = current_dir.join("whatsapp_config.json");
        
        if local_config.exists() {
            return current_dir;
        }
        
        // Fall back to standard config locations
        if let Ok(config_dir) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(config_dir).join("whatsapp_client_rs")
        } else if let Some(home) = dirs::home_dir() {
            home.join(".config").join("whatsapp_client_rs")
        } else {
            PathBuf::from(".whatsapp_client_rs")
        }
    }

    pub fn store_path(&self) -> PathBuf {
        self.config_dir.join("store")
    }

    pub fn layout_path(&self) -> PathBuf {
        self.config_dir.join("whatsapp_layout.json")
    }

    pub fn aliases_path(&self) -> PathBuf {
        self.config_dir.join("whatsapp_aliases.json")
    }
}

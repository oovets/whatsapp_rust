use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<String>> = Mutex::new(None);

pub fn init_logging(log_file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Store log file path
    *LOG_FILE.lock().unwrap() = Some(log_file_path.to_string());
    
    // Initialize tracing subscriber with file output
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;
    
    let file = std::fs::File::create(log_file_path)?;
    let file_writer = std::io::BufWriter::new(file);
    
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(Mutex::new(file_writer))
        .with_ansi(false)
        .with_target(true)
        .with_line_number(true)
        .with_file(true);
    
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("debug"));
    
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .init();
    
    tracing::info!("Logging initialized to: {}", log_file_path);
    
    Ok(())
}

pub fn send_desktop_notification(_title: &str, _message: &str) {
    // TODO: Implement desktop notifications
    // For now, just log it
    crate::debug_log!("Notification: {} - {}", _title, _message);
}

pub fn try_autocomplete(text: &str) -> (Option<String>, Option<String>) {
    // Simple autocomplete for commands
    let commands = vec!["/reply", "/media", "/edit", "/delete", "/alias", "/search", "/forward"];
    
    if text.starts_with('/') {
        for cmd in commands {
            if cmd.starts_with(text) {
                return (Some(cmd.to_string()), None);
            }
        }
    }
    
    (None, None)
}

pub fn log_debug(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(guard) = LOG_FILE.lock() {
        if let Some(ref log_path) = *guard {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "[{}] DEBUG: {}", timestamp, message);
            }
        }
    }
    tracing::debug!("{}", message);
}


pub fn log_info(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(guard) = LOG_FILE.lock() {
        if let Some(ref log_path) = *guard {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "[{}] INFO: {}", timestamp, message);
            }
        }
    }
    tracing::info!("{}", message);
}

pub fn log_warn(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(guard) = LOG_FILE.lock() {
        if let Some(ref log_path) = *guard {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "[{}] WARN: {}", timestamp, message);
            }
        }
    }
    tracing::warn!("{}", message);
}

pub fn log_error(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(guard) = LOG_FILE.lock() {
        if let Some(ref log_path) = *guard {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "[{}] ERROR: {}", timestamp, message);
            }
        }
    }
    tracing::error!("{}", message);
}

#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        $crate::utils::log_debug(&format!($($arg)*));
    };
}

#[macro_export]
macro_rules! info_log {
    ($($arg:tt)*) => {
        $crate::utils::log_info(&format!($($arg)*));
    };
}

#[macro_export]
macro_rules! warn_log {
    ($($arg:tt)*) => {
        $crate::utils::log_warn(&format!($($arg)*));
    };
}

#[macro_export]
macro_rules! error_log {
    ($($arg:tt)*) => {
        $crate::utils::log_error(&format!($($arg)*));
    };
}

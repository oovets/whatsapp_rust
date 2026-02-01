use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

mod app;
mod commands;
mod config;
mod formatting;
mod persistence;
mod split_view;
mod whatsapp;
mod utils;
mod widgets;


use app::App;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to file
    let log_file = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".config")
        .join("whatsapp_client_rs")
        .join("debug.log");
    
    // Create log directory if it doesn't exist
    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    utils::init_logging(log_file.to_str().unwrap()).map_err(|e| anyhow::anyhow!("Failed to initialize logging: {}", e))?;
    crate::info_log!("=== WhatsApp Client Starting ===");
    
    // Create app BEFORE entering TUI mode (so authentication can work)
    let mut app = App::new().await?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run app
    let _res = run_app(&mut terminal, &mut app).await;
    
    // Save state before exiting (even if there was an error)
    let _ = app.save_state();

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut last_whatsapp_check = std::time::Instant::now();
    let mut last_chat_list_refresh = std::time::Instant::now();

    loop {
        // Only redraw when something changed
        if app.needs_redraw {
            terminal.draw(|f| app.draw(f))?;
            app.needs_redraw = false;
        }

        // Refresh chat list every 5 seconds to get latest messages
        if last_chat_list_refresh.elapsed() >= std::time::Duration::from_secs(5) {
            let _ = app.refresh_chat_list().await;
            last_chat_list_refresh = std::time::Instant::now();
            app.needs_redraw = true;
        }

        // Process WhatsApp events every 500ms
        if last_whatsapp_check.elapsed() >= std::time::Duration::from_millis(500) {
            let had_updates = app.process_whatsapp_events().await?;
            last_whatsapp_check = std::time::Instant::now();
            if had_updates {
                app.needs_redraw = true;
            }
        }

        // Sleep until next check (or cap at 500ms)
        let poll_timeout = std::time::Duration::from_millis(500)
            .saturating_sub(last_whatsapp_check.elapsed())
            .max(std::time::Duration::from_millis(16));

        if event::poll(poll_timeout)? {
            let event = event::read()?;
            match event {
                Event::Key(key) => {
                    app.needs_redraw = true;
                    match key.code {
                    // Ctrl+Q: Quit
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.save_state()?;
                        break;
                    }
                    // Ctrl+R: Refresh chats
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.refresh_chats().await?;
                    }
                    // Ctrl+V: Split vertical
                    KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.split_vertical();
                    }
                    // Ctrl+B: Split horizontal
                    KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.split_horizontal();
                    }
                    // Ctrl+K: Toggle split direction
                    KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_split_direction();
                    }
                    // Ctrl+W: Close pane
                    KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.close_pane();
                    }                    // Ctrl+S: Toggle chat list (Sidebar)
                    KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_chat_list();
                    }                    // Ctrl+L: Clear pane
                    KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.clear_pane();
                    }
                    // Ctrl+E: Toggle reactions
                    KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_reactions();
                    }
                    // Ctrl+N: Toggle notifications
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_notifications();
                    }
                    // Ctrl+D: Toggle compact mode
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_compact();
                    }
                    // Ctrl+O: Toggle emojis
                    KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_emojis();
                    }
                    // Ctrl+G: Toggle line numbers
                    KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_line_numbers();
                    }
                    // Ctrl+T: Toggle timestamps
                    KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_timestamps();
                    }
                    // Ctrl+U: Toggle user colors
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_user_colors();
                    }
                    // Ctrl+Y: Toggle borders
                    KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.toggle_borders();
                    }
                    // Esc: Cancel reply mode
                    KeyCode::Esc => {
                        if let Some(pane) = app.panes.get_mut(app.focused_pane_idx) {
                            if pane.reply_to_message.is_some() {
                                pane.reply_to_message = None;
                                pane.hide_reply_preview();
                            }
                        }
                    }
                    // Shift+Tab: Cycle focus backwards (only if input empty)
                    KeyCode::BackTab => {
                        let input_empty = app
                            .panes
                            .get(app.focused_pane_idx)
                            .map_or(true, |p| p.input_buffer.is_empty());
                        if app.focus_on_chat_list || input_empty {
                            app.cycle_focus_reverse();
                        }
                    }
                    // Tab: Autocomplete or cycle focus
                    KeyCode::Tab => {
                        app.handle_tab();
                    }
                    // Alt+Left/Right: Focus previous/next pane
                    KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                        app.focus_prev_pane();
                    }
                    KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                        app.focus_next_pane();
                    }
                    // Arrow keys
                    KeyCode::Up => {
                        app.handle_up();
                    }
                    KeyCode::Down => {
                        app.handle_down();
                    }
                    KeyCode::Left => {
                        if !app.focus_on_chat_list {
                            app.handle_input_left();
                        }
                    }
                    KeyCode::Right => {
                        if !app.focus_on_chat_list {
                            app.handle_input_right();
                        }
                    }
                    // Home/End: Move cursor to start/end
                    KeyCode::Home => {
                        if !app.focus_on_chat_list {
                            app.handle_home();
                        }
                    }
                    KeyCode::End => {
                        if !app.focus_on_chat_list {
                            app.handle_end();
                        }
                    }
                    // PageUp/PageDown: Scroll messages
                    KeyCode::PageUp => {
                        app.handle_page_up();
                    }
                    KeyCode::PageDown => {
                        app.handle_page_down();
                    }
                    // Enter: Submit
                    KeyCode::Enter => {
                        app.handle_enter().await?;
                    }
                    // Character input (only when not on chat list)
                    KeyCode::Char(c) => {
                        if !app.focus_on_chat_list {
                            app.handle_char(c);
                        }
                    }
                    // Backspace
                    KeyCode::Backspace => {
                        if !app.focus_on_chat_list {
                            app.handle_backspace();
                        }
                    }
                    // Delete
                    KeyCode::Delete => {
                        if !app.focus_on_chat_list {
                            app.handle_delete();
                        }
                    }
                    _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    app.needs_redraw = true;
                    if let event::MouseEventKind::Down(event::MouseButton::Left) = mouse.kind {
                        // Check if clicking on chat list first
                        if let Some(area) = app.chat_list_area {
                            if mouse.column >= area.x && mouse.column < area.x + area.width 
                                && mouse.row >= area.y && mouse.row < area.y + area.height {
                                // Clicked on chat list
                                app.handle_chat_list_click(mouse.row, area).await?;
                            }
                        }
                        // Check if clicking on a pane
                        app.handle_mouse_click(mouse.column, mouse.row);
                        // Load messages for focused pane if needed
                        app.load_pane_messages_if_needed(app.focused_pane_idx).await;
                    }
                }
                Event::Resize(_, _) => {
                    app.needs_redraw = true;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

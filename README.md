# WhatsApp TUI Client

A terminal-based WhatsApp client written in Rust, providing a fast and efficient way to use WhatsApp from the command line.

## Features

- Full WhatsApp messaging support
- Multi-pane interface for viewing multiple chats simultaneously
- Modern TUI with keyboard shortcuts
- Smart chat filtering (removes duplicate and junk chats)
- Automatic reaction filtering in group chats
- Contact name resolution from WhatsApp database
- Background message syncing
- Message history with proper sender names

## Prerequisites

- Rust (latest stable version)
- [whatsapp-cli](https://github.com/tulir/whatsmeow/tree/main/mdtest) - The underlying WhatsApp client

### Installing whatsapp-cli

```bash
# Install Go if you don't have it
brew install go  # macOS
# or
apt install golang  # Linux

# Install whatsapp-cli
go install github.com/tulir/whatsmeow/mdtest@latest

# The binary will be in ~/go/bin/mdtest
# You may want to add ~/go/bin to your PATH
export PATH=$PATH:~/go/bin
```

## Installation

1. Clone the repository:
```bash
git clone https://github.com/oovets/whatsapp_rust.git
cd whatsapp_rust
```

2. Build the project:
```bash
cargo build --release
```

The binary will be available at `target/release/whatsapp_client_rs`

## Setup

### 1. Authenticate with WhatsApp

Before using the client, you need to authenticate with WhatsApp:

```bash
# Create a store directory for WhatsApp data
mkdir -p ~/.config/whatsapp_client_rs/store

# Authenticate (this will show a QR code)
mdtest --store ~/.config/whatsapp_client_rs/store auth
```

Scan the QR code with your phone (WhatsApp → Settings → Linked Devices → Link a Device)

### 2. Initial Sync

After authentication, sync your messages:

```bash
# Run sync to download your chats and messages
mdtest --store ~/.config/whatsapp_client_rs/store sync
```

This will download your chat history. Let it run for a few minutes to get your recent messages.

Press `Ctrl+C` when you see messages being synced.

### 3. Run the Client

```bash
# Run the client
cargo run --release

# Or if you've installed it:
./target/release/whatsapp_client_rs
```

## Configuration

The client looks for `whatsapp-cli` (mdtest) in these locations:
1. `~/go/bin/mdtest`
2. `/usr/local/bin/mdtest`
3. `/usr/bin/mdtest`
4. `mdtest` in PATH

Store location: `~/.config/whatsapp_client_rs/store/`

## Usage

### Keyboard Shortcuts

#### Navigation
- `Tab` / `Shift+Tab` - Switch between chat list and message panes
- `↑` / `↓` - Navigate chats or messages
- `Enter` - Open selected chat
- `Esc` - Return to chat list

#### Pane Management
- `Ctrl+N` - Create new pane (split view)
- `Ctrl+W` - Close current pane
- `Ctrl+→` / `Ctrl+←` - Switch between panes

#### Messaging
- Type and press `Enter` - Send message
- `Ctrl+C` - Copy selected message
- `Ctrl+V` - Paste

#### Other
- `Ctrl+R` - Refresh chat list
- `Ctrl+Q` - Quit application
- `?` - Show help

### Chat List

The chat list is organized into three sections:
- **Unread** - Chats with unread messages
- **Active** - Currently open chats
- **Other** - All other chats

### Features

#### Smart Chat Filtering
The client automatically filters out:
- Duplicate chats (e.g., same contact with different JID formats)
- Junk chats with raw JIDs as names
- Legacy @lid chats when @s.whatsapp.net version exists

#### Reaction Filtering
In group chats, reaction messages (messages with only `{{...}}`) are automatically filtered out to keep the conversation clean.

#### Contact Names
The client reads contact names from the WhatsApp database, showing real names instead of phone numbers.

## Architecture

### Components

- **whatsapp.rs** - WhatsApp client wrapper, handles communication with whatsapp-cli and direct SQLite database access
- **app.rs** - Main application state and UI rendering
- **commands.rs** - Command handling and execution
- **widgets.rs** - Custom TUI widgets
- **split_view.rs** - Multi-pane layout management

### How It Works

1. **Authentication**: Uses `whatsapp-cli` for QR code authentication
2. **Message Retrieval**: 
   - Group chats: Direct SQLite database access for reliability
   - Individual chats: Uses `whatsapp-cli messages list`
3. **Contact Resolution**: Reads from `whatsapp.db` contacts table
4. **Background Sync**: Runs `whatsapp-cli sync` in background to keep messages updated

## Troubleshooting

### No chats showing up
Run sync manually:
```bash
mdtest --store ~/.config/whatsapp_client_rs/store sync
```
Let it run for a few minutes, then restart the client.

### Messages not appearing in group chats
The client reads directly from the SQLite database. Make sure sync has run at least once.

### Authentication issues
Remove the store and re-authenticate:
```bash
rm -rf ~/.config/whatsapp_client_rs/store
mdtest --store ~/.config/whatsapp_client_rs/store auth
```

### Debug logs
Logs are written to `~/.config/whatsapp_client_rs/debug.log`

```bash
tail -f ~/.config/whatsapp_client_rs/debug.log
```

## Development

### Building
```bash
cargo build
```

### Running in development
```bash
cargo run
```

### Release build
```bash
cargo build --release
```

## Dependencies

- `ratatui` - Terminal UI framework
- `crossterm` - Terminal manipulation
- `tokio` - Async runtime
- `serde` / `serde_json` - JSON serialization
- `rusqlite` - SQLite database access
- `chrono` - Date/time handling

## Known Limitations

- Media download not yet implemented
- Message editing limited by whatsapp-cli capabilities
- No voice message support
- Group admin functions not available

## License

MIT

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Acknowledgments

- [whatsmeow](https://github.com/tulir/whatsmeow) - The underlying WhatsApp library
- [ratatui](https://github.com/ratatui-org/ratatui) - Terminal UI framework

# Tutorial 01: Setting Up and Using Ace Term (Phase 1)

This guide walks through installing Ace Term, configuring your terminal and AI provider, and using all Phase 1 features across the terminal grid, shell integration, background project detection, and inline AI blocks.

---

## 1. Installation

### Option A: Prebuilt Binary (after a tagged release is published)

**macOS & Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/princetonacelopez/ace-term/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/princetonacelopez/ace-term/main/install.ps1 | iex
```

### Option B: Build from Source

Prerequisites:
- Rust `1.88+` (2024 edition)
- On Linux, development headers for `xkbcommon`, `wayland`, `xcb`, `fontconfig`, `freetype`, `dbus`, `openssl`, and `vulkan-loader`

Clone and build:

```sh
git clone https://github.com/princetonacelopez/ace-term.git
cd ace-term
cargo build --release -p ace-ui
```

Run the binary directly or install it to `~/.local/bin`:

```sh
./target/release/ace-term
# Optional:
cp target/release/ace-term ~/.local/bin/ace-term
```

---

## 2. Configuration

Ace Term starts cleanly with built-in defaults and reads overrides from:
1. Global config: `~/.config/ace-term/config.toml`
2. Workspace override (optional): `.ace/config.toml` in the launch directory

Create `~/.config/ace-term/config.toml`:

```toml
[terminal]
shell = ""                  # Empty auto-detects $SHELL / pwsh.exe
scrollback_lines = 10000
cursor_style = "block"      # "block" | "bar" | "underline"

[ai]
provider = "antigravity"    # "antigravity" | "ollama" | "lmstudio" | "anthropic" | "openai"
model = "gemini-4-argon-medium"
base_url = ""               # Optional OpenAI-compatible endpoint
approval_mode = "default"   # "default" | "auto"
auto_intercept = true

[ui]
theme = "Default Dark"      # "Default Dark" | "Default Light" | "Dracula" | "Nord"
opacity = 1.0
padding = 8

[font]
family = "JetBrains Mono"
fallback_families = ["Symbols Nerd Font", "Noto Color Emoji"]
size = 14.0
line_height = 1.35
ligatures = true
```

---

## 3. Window Controls, Context Menu, and Settings UI

- **Native Window Controls**: The title bar respects OS window controls (minimize, maximize, close) and displays the active shell/window title.
- **Right-Click Context Menu**: Right-click anywhere in the terminal canvas to open the context menu with `Copy` (`Ctrl+Shift+C`), `Paste` (`Ctrl+Shift+V`), `Clear` (`Ctrl+L`), and `Settings` (`Ctrl+,`).
- **Settings UI (`Ctrl+,` / `Cmd+,`)**: Adjust Theme (`Default Dark`, `Default Light`, `Dracula`, `Nord`), Font Family, Font Size, Line Height, Cursor Style (`Block`, `Bar`, `Underline`), Padding, Opacity, and AI settings (Provider, Model, Base URL, Approval Mode, Auto Intercept, and OS Keychain API key), then click **Save & Apply** to write `~/.config/ace-term/config.toml` and apply immediately.

## 4. AI Authentication (`ace-term auth`)

Store your provider API key in the OS keychain (`keyring`) rather than plaintext config files:

```sh
ace-term auth set anthropic
ace-term auth set openai
ace-term auth status
ace-term auth clear anthropic
```

You can also pass a key inline (`ace-term auth set anthropic sk-...`), configure it in the Settings UI, or export `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`.

## 5. Shell Integration, Status Bar, and Inline AI Blocks

- **Non-Destructive Shell Hooks (`ace-shell`)**: Ace Term injects lightweight `zsh`, `bash`, or `pwsh` integration hooks (`OSC 7` working directory updates and `OSC 133` prompt/command markers) without modifying your `~/.zshrc` or `~/.bashrc`.
- **Background Project Detection (`ace-env`)**: The bottom status bar displays your current Git branch and dirty indicator (`git:branch*`) alongside detected project toolchains (`Rust`, `Node`, `Python`, `Go`).
- **Zero-Prefix Routing & Mode Toggle**:
  - In `[AUTO]` mode (when `auto_intercept = true`), Ace Term inspects your input on `Enter` using `tree-sitter-bash` and `$PATH` executable resolution: valid shell commands go directly to the PTY, while natural-language prompts route to the AI orchestrator.
  - Press `Shift+Tab` (or `Tab` on an empty prompt) or click the status-bar pill to cycle `[AUTO]`, `[AI]`, and `[SHELL]`.
- **Inline AI Blocks (`AiBlockView`)**:
  - Streaming responses, unified file diffs (`[Accept]` / `[Reject]`), and proposed shell commands (`[Approve]` / `[Deny]`) render inline above the prompt.
  - When a shell command exits non-zero, the status bar offers `Exit N — Ask AI to fix?` to send the recent terminal context to the AI.

## 6. Live Hot-Reloading

Ace Term watches both `~/.config/ace-term/config.toml` and `.ace/config.toml` for changes:

1. Launch `ace-term`.
2. Open `~/.config/ace-term/config.toml` in an editor.
3. Change `theme = "Dracula"`, `font.size = 16.0`, or `cursor_style = "bar"` and save.
4. The window updates immediately without restarting your shell.

If you save an invalid TOML edit, Ace Term logs a warning and keeps the last valid configuration active.

---

## 7. Terminal Controls & Features

- **Shell Execution & Signals**: Ace Term wraps your existing shell (`zsh`, `bash`, `pwsh`) over an async PTY. Keystrokes and `Ctrl+C` (`0x03`) are written directly to the PTY without waiting on render frames.
- **Mouse Selection & Clipboard**:
  - Click and drag with the left mouse button to select text; single-click or type to clear the selection.
  - Copy: `Ctrl+Shift+C` (Linux/Windows) or `Cmd+C` (macOS).
  - Paste: `Ctrl+Shift+V` (Linux/Windows) or `Cmd+V` (macOS).
- **Scrollback**: Use the mouse wheel to scroll through up to `scrollback_lines` of terminal history.
- **Window Resize Reflow**: Resizing the window recalculates `(cols, rows)` from your font metrics and notifies the PTY (`TIOCSWINSZ`) for line reflow.
- **Full-Screen TUIs**: Tools such as `vim` and `htop` automatically switch to the alternate screen buffer and use DEC Mode 2026 (`\x1b[?2026h` / `\x1b[?2026l`) synchronized frame updates with a 1000ms safety flush timer.

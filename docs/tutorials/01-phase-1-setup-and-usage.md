# Tutorial 01: Setting Up and Using Ace Term (Phase 1 / Milestone 1)

This guide walks through installing Ace Term, configuring your terminal profile, and using the core features available in Milestone 1 (`v0.1.0-m1`).

> **Scope note:** Milestone 1 delivers the GPU-accelerated terminal canvas (`ace-ui`), virtual terminal grid (`ace-vt`), async PTY layer (`ace-pty`), and live configuration store (`ace-config`). Shell integration hooks (`ace-shell`), background Git/toolchain detection (`ace-env`), and inline AI blocks (`ace-agent`) are scheduled for Milestones 2 and 3.

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

## 3. Live Hot-Reloading

Ace Term watches both `~/.config/ace-term/config.toml` and `.ace/config.toml` for changes:

1. Launch `ace-term`.
2. Open `~/.config/ace-term/config.toml` in an editor.
3. Change `theme = "Dracula"`, `font.size = 16.0`, or `cursor_style = "bar"` and save.
4. The window updates immediately without restarting your shell.

If you save an invalid TOML edit, Ace Term logs a warning and keeps the last valid configuration active.

---

## 4. Terminal Controls & Features

- **Shell Execution & Signals**: Ace Term wraps your existing shell (`zsh`, `bash`, `pwsh`) over an async PTY. Keystrokes and `Ctrl+C` (`0x03`) are written directly to the PTY without waiting on render frames.
- **Mouse Selection & Clipboard**:
  - Click and drag with the left mouse button to select text; single-click or type to clear the selection.
  - Copy: `Ctrl+Shift+C` (Linux/Windows) or `Cmd+C` (macOS).
  - Paste: `Ctrl+Shift+V` (Linux/Windows) or `Cmd+V` (macOS).
- **Scrollback**: Use the mouse wheel to scroll through up to `scrollback_lines` of terminal history.
- **Window Resize Reflow**: Resizing the window recalculates `(cols, rows)` from your font metrics and notifies the PTY (`TIOCSWINSZ`) for line reflow.
- **Full-Screen TUIs**: Tools such as `vim` and `htop` automatically switch to the alternate screen buffer and use DEC Mode 2026 (`\x1b[?2026h` / `\x1b[?2026l`) synchronized frame updates with a 1000ms safety flush timer.

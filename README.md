# Ace Term (`ace-term`)

Ace Term is an open-source, GPU-accelerated, multiplatform terminal emulator with integrated AI workflows. It wraps your existing native shell (`zsh`, `bash`, or `pwsh`) rather than replacing it, combining a hardware-accelerated terminal grid with inline AI blocks, diffs, and project-aware context.

## Architecture

The repository is organized as a seven-crate Rust workspace (Rust 2024 edition, MSRV 1.88):

- `crates/ace-vt` — Virtual terminal grid, scrollback history, selection, and DEC Mode 2026 synchronized output built on `alacritty_terminal`.
- `crates/ace-pty` — Cross-platform async PTY layer built on `rust-pty` and `tokio`.
- `crates/ace-shell` — Non-destructive shell integration hooks (`OSC 7`, `OSC 133`) and zero-prefix command classification.
- `crates/ace-env` — Asynchronous background Git (`gix`) and project toolchain detection.
- `crates/ace-agent` — Streaming LLM orchestrator, built-in file and shell tools, and MCP client support.
- `crates/ace-config` — Configuration loader (`config.toml`) with lock-free `ArcSwap` hot-reloading and OS keychain secret storage.
- `crates/ace-ui` — Binary crate (`ace-term`) rendering the terminal canvas, inline AI blocks, and status bar via `gpui-kit`.

For the full roadmap and technical requirements, see [SPEC.md](SPEC.md). For step-by-step guides, see [docs/tutorials](docs/tutorials/README.md).

## Installation

### Prebuilt Binaries

**macOS & Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/princetonacelopez/ace-term/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/princetonacelopez/ace-term/main/install.ps1 | iex
```

## Building from Source

### Prerequisites

- Rust 1.88 or newer (2024 edition)
- On Linux: development headers for `xkbcommon`, `wayland`, `xcb`, `fontconfig`, `freetype`, `dbus`, `openssl`, and `vulkan-loader`
- Optional: `cargo-deny` for license compliance checks

### Build and Run

```bash
cargo build --workspace
cargo run -p ace-ui
```

### Verification

```bash
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo deny check licenses
```

## Configuration

Ace Term reads user configuration from `~/.config/ace-term/config.toml` and optional workspace overrides from `.ace/config.toml`. Changes to `config.toml` are hot-reloaded while running.

Example configuration:

```toml
[terminal]
shell = ""
scrollback_lines = 10000
cursor_style = "block"

[ai]
provider = "anthropic"
model = "claude-sonnet-4-6"
base_url = ""
approval_mode = "default"
auto_intercept = true

[ui]
theme = "Default Dark"
opacity = 1.0
padding = 8

[font]
family = "JetBrains Mono"
fallback_families = ["Symbols Nerd Font", "Noto Color Emoji"]
size = 14.0
line_height = 1.35
ligatures = true
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. All dependencies are checked to ensure no GPL or AGPL crates are included.

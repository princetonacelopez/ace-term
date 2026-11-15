# Ace Term (`ace-term`) — Product Roadmap & Technical Requirements Specification

---

## 1. Product Roadmap: v0.1 MVP to v1.0 GA

### Phase 1: `v0.1.0` — Core Terminal & Zero-Prefix AI MVP (Weeks 1–4)
*Goal: A daily-drivable, GPU-accelerated terminal wrapping the user's default shell with seamless inline AI blocks.*
* **GPU Terminal Canvas (`gpui-kit` + `alacritty_terminal`)**: Custom GPUI terminal grid element with 60/120 Hz frame pacing, mouse text selection, scrollback reflow, and DEC Mode 2026 synchronized output.
* **Existing Shell Integration (`zsh`, `bash`, `pwsh`)**: Automatic shell hook injection (`OSC 7` CWD tracking, `OSC 133` prompt/input/output semantic zones, and `zle`/`READLINE_LINE`/`PSReadLine` buffer inspection).
* **Zero-Prefix AI Router**: Sub-2ms classification on `Enter` (`tree-sitter-bash` / `tree-sitter-powershell` + `$PATH` binary table) with `Tab` manual mode toggle (`[SHELL]` ↔ `[AI]`).
* **Inline Rich AI Blocks**: Streaming Markdown, syntax-highlighted code (`gpui-component` / `syntect`), unified file diffs (`imara-diff`) with `[Accept]`/`[Reject]` actions, and `Default` (ask) vs. `Auto` execution approval.
* **Async Context & Config**: Non-blocking `gix` Git status + manifest toolchain detection (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`), `ArcSwap` hot-reloaded `config.toml`, and OS `keyring` API key storage.

### Phase 2: `v0.3.0` — Rich Media, Split Panes & MCP Ecosystem (Weeks 5–8)
*Goal: Elevate Ace Term from an AI-assisted terminal into a multimodal generative workspace.*
* **Dockable Panes & Tabs**: Leverage `gpui-component`'s `DockArea` and `TabGroup` for split terminals, persistent agent sidebars, and serializable workspace layouts.
* **Inline Images, Diagrams & HTML**: Support Kitty graphics protocol + `resvg` SVG rasterization (with zoom-aware texture scaling) and pure-Rust `blitz` inline HTML/CSS rendering for rich agent artifacts.
* **MCP & ACP Interoperability**: Full Model Context Protocol (`rmcp`) client support so users can plug in any MCP server via `config.toml`, plus headless JSON-RPC / ACP mode.
* **Context Compaction Engine**: Prompt-cache-aware message splitting (Blake2b static boundary) with tiered tool-output pruning and structured session summarization.

### Phase 3: `v0.6.0` — Video/Web Canvas, OS Sandboxing & Local Voice (Weeks 9–12)
*Goal: Complete the full multimodal vision with hardware-isolated execution and hands-free voice.*
* **Embedded Web & Video Playback**: Offscreen Chromium (`download-cef`) textures for live web previews/docs alongside `openh264` + `symphonia` + `awedio` inline video/audio playback.
* **OS-Enforced Agent Sandboxing**: Kernel-level execution confinement (`landlock` + `seccompiler` on Linux, Seatbelt profiles on macOS, restricted tokens + WFP egress rules on Windows) with explicit `~/.ssh` and `~/.aws` read denials.
* **Optional Voice I/O (`--features voice`)**: Lock-free `cpal` audio capture → Silero VAD (`voice_activity_detector`) → `sherpa-onnx` / `whisper-rs` transcription injected directly into the active shell line editor, paired with local `Kokoro-82M` TTS responses.

### Phase 4: `v1.0.0` — Public Release, Plugin Hooks & Packaging (Weeks 13–16)
*Goal: Production stability, public `crates.io` publication, and native OS installers.*
* **Optional Lua Scripting (`mlua-pkg`)**: Programmable status-bar widgets, custom keybindings, and event hooks (`update-status`, `on-command-finish`).
* **Hardened Multiplatform Packaging**: Baseline-ISA release builds (preventing `SIGILL` on older CPUs), signed macOS `.dmg`/Homebrew cask, Windows MSI/Winget package, Linux `.deb`/`.rpm`/AppImage/Nix flake, and `crates.io` crate publication.

---

## 2. Technical Requirements & Agent Execution Spec

### 2.1 System Identity & Constraints
- **Project:** Ace Term (`ace-term`) — Open-source, GPU-accelerated, multiplatform agentic terminal emulator.
- **Language:** Rust (2024 Edition, MSRV 1.88+).
- **License:** `MIT OR Apache-2.0`. Strict enforcement: zero GPL/AGPL crates anywhere in the dependency tree. Every PR/CI run must pass `cargo deny check licenses`.
- **UI Stack:** `gpui-kit` (`0.9.x`, Apache-2.0) providing GPUI rendering, `gpui-component` styled primitives (Markdown, syntax highlighting, docking, themes), and Lucide icons.
- **Shell Strategy:** Wrap the user's existing native shell (`zsh`, `bash`, `pwsh`) via PTY and shell-integration hooks (`OSC 7`, `OSC 133`, and line-editor widgets). Do not replace the user's shell binary in v0.1.
- **Target Platforms:** macOS (aarch64, x86_64), Linux (Wayland & X11; x86_64, aarch64), Windows 10/11 (ConPTY; x86_64, aarch64).
- **Compilation Rule:** Never set `-C target-cpu=native` in workspace Cargo profiles or CI release builds so binaries run safely across older x86_64 and aarch64 CPUs without `SIGILL`.

### 2.2 Cargo Workspace Layout
Create a Cargo workspace at the repository root with 7 crates:

1. `crates/ace-vt` (Library)
   - Wraps `alacritty_terminal` (`0.26.x`) behind an internal `TerminalGrid` trait.
   - Manages grid cells, scrollback history, selection ranges, dirty-line tracking, and DEC Private Mode 2026 (`\x1b[?2026h` / `\x1b[?2026l`) with a 1000ms safety flush timer.
2. `crates/ace-pty` (Library)
   - Cross-platform async PTY layer built on `rust-pty` (`0.6.x`) + `tokio`.
   - Unix: `openpty`, `termios`, `TIOCSWINSZ` resize propagation, child `exit` monitoring (do not hang on inherited background FDs).
   - Windows: ConPTY (`CreatePseudoConsole`, `ResizePseudoConsole`) with `PSEUDOCONSOLE_RESIZE_QUIRK` (`0x2`) and `PSEUDOCONSOLE_WIN32_INPUT_MODE` (`0x4`); gate `PSEUDOCONSOLE_PASSTHROUGH_MODE` (`0x8`) to Windows build >= 22621.
   - Normalize macOS APFS NFD path strings to Unicode NFC on all directory updates.
3. `crates/ace-shell` (Library)
   - Bundles shell integration hook scripts (`resources/shell/ace.zsh`, `ace.bash`, `ace.ps1`) injected non-destructively at PTY spawn (via `ZDOTDIR` wrapper for zsh, `--rcfile`/init injection for bash, `-NoExit -Command` hook for pwsh).
   - Hooks emit `OSC 7` (CWD), `OSC 133;A/B/C/D` (prompt start, command input start, command execution start, command exit code), and pre-exec line-buffer interception (`zle` widget in zsh, `READLINE_LINE` in bash, `PSReadLine` in pwsh).
   - Implements the `< 2ms` zero-prefix command classifier using `tree-sitter-bash` / `tree-sitter-powershell` and an in-memory indexed `$PATH` / `PATHEXT` executable cache.
4. `crates/ace-env` (Library)
   - Asynchronous background environment detector running on `tokio::spawn` whenever `OSC 7` reports a CWD change.
   - Git inspection via pure-Rust `gix` (configured with LRU object cache and first-parent traversal; respects `gix` repository trust boundaries).
   - Project toolchain detection via bounded manifest checks (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `.envrc`) with `(binary_path, mtime)`-keyed version caching.
5. `crates/ace-agent` (Library)
   - Stateful async agent orchestrator supporting OpenAI-compatible (`/v1/chat/completions`) and Anthropic (`/v1/messages`) streaming SSE APIs via `reqwest`.
   - Built-in tools: `read_file`, `write_file` (generates unified diff via `imara-diff`), `list_dir`, `search_code`, and `run_shell` (executes in a hidden or inline PTY with exit code + bounded output capture).
   - MCP client integration via `rmcp` for user-configured external tool servers.
   - Prompt cache optimization: keep static system prompt, `ace-env` workspace summary, and JSON tool schemas at the immutable prefix of the message history.
6. `crates/ace-config` (Library)
   - Parses `~/.config/ace-term/config.toml` and optional workspace `.ace/config.toml` via `serde` + `toml`.
   - Exposes lock-free hot-reloaded state via `ArcSwap<AceConfig>` watched via `notify`.
   - Stores and retrieves LLM API keys exclusively via the OS keychain (`keyring` crate). Includes regex secret scrubbing before sending terminal output to remote LLMs.
7. `crates/ace-ui` (Binary: `ace-term`)
   - Entry point using `gpui_kit::init(cx)` and `gpui_kit::open_window(...)`.
   - Implements `TerminalCanvasView` (custom GPUI `Element` rendering `ace-vt` cells, cursor, selection highlight, and inline Kitty/`resvg` images).
   - Renders interleaved `AiBlockView` cards in the unified scrollback using `gpui-component` Markdown, code blocks, inline diff reviewers (`[Accept]` / `[Reject]`), and tool-approval banners.
   - Renders bottom status bar displaying `ace-env` Git branch/dirty status, detected language runtimes, active AI model, and `[SHELL]`/`[AI]` mode indicator.

### 2.3 Core Architectural Contracts

#### Decoupled PTY-to-GPU Pipeline
- **PTY Reader Task:** Dedicated Tokio task reads raw bytes from PTY master → feeds `ace-vt` parser → marks dirty rows → sends lightweight `WakeUi` notification on a bounded channel (`tokio::sync::watch`). High-throughput stdout must never block UI input or `Ctrl+C` (`0x03`) writes.
- **UI Render Loop:** GPUI renders on vsync/dirty invalidation, reading only visible viewport rows + active `AiBlockView` items.

#### Zero-Prefix Classification Rules (`ace-shell`)
When the user presses `Enter` (or submits via shell hook):
1. **Alt-Screen Guard:** If `terminal.is_alt_screen()` is true (e.g., `vim`, `less`, `htop`), forward raw `\r` directly to PTY. Bypass AI completely.
2. **Explicit Mode Override:** If user toggled the active prompt to `[AI]` via `Tab`, route directly to `ace-agent`. If toggled to `[SHELL]`, dispatch directly to PTY.
3. **Automatic Classification (`Auto` default):**
   - Trim input. If empty, forward `\r` to PTY.
   - If input starts with valid shell control operators/syntax (`|`, `&&`, `||`, `;`, `>`, `<`, `$(`, backticks, or `VAR=val cmd`) parsed cleanly by `tree-sitter`, route to **PTY**.
   - Extract the leading command token. If it matches a shell builtin (`cd`, `export`, `alias`, `echo`, `source`, `.`, etc.), a relative/absolute path (`./`, `../`, `/`, `~/`), or an executable in the cached `$PATH` index, route to **PTY**.
   - Otherwise, if input has `>= 2` words or ends with `?` (natural language intent), intercept the line, clear the shell's line buffer without executing in the shell, and spawn an `AiBlockView` in `ace-agent`.
   - If single unknown token (likely a typo), dispatch to PTY so the user sees standard shell feedback (plus optional inline "Ask AI to fix?" action on non-zero `OSC 133;D` exit).

#### Configuration Schema (`~/.config/ace-term/config.toml`)
```toml
[terminal]
shell = ""                  # Empty = auto-detect $SHELL / pwsh.exe
scrollback_lines = 10000
cursor_style = "block"      # "block" | "bar" | "underline"

[ai]
provider = "anthropic"      # "anthropic" | "openai" | "ollama"
model = "claude-sonnet-4-6"
base_url = ""               # Optional custom endpoint
approval_mode = "default"   # "default" (prompt before write/shell) | "auto"
auto_intercept = true       # Enable zero-prefix natural language detection

[ui]
theme = "Default Dark"      # Matches gpui-kit Theme names + custom themes
opacity = 1.0
padding = 8

[font]
family = "JetBrains Mono"
fallback_families = ["Symbols Nerd Font", "Noto Color Emoji"]
size = 14.0
line_height = 1.35
ligatures = true
```

### 2.4 Phased Implementation Milestones & Acceptance Criteria

#### Milestone 1: Workspace & Functional GPU Terminal (`v0.1-m1`)
- **Tasks:** Scaffold 7-crate Cargo workspace; implement `ace-vt`, `ace-pty`, `ace-config`, and `ace-ui` `TerminalCanvasView`.
- **Acceptance Criteria:**
  - `cargo build --workspace` compiles cleanly on macOS, Linux, and Windows.
  - Launches user's default shell in `< 150ms`, supports full-screen TUIs (`vim`, `htop`), mouse text selection/copy/paste, window resize reflow, and live font/theme hot-reloading from `config.toml`.

#### Milestone 2: Shell Hooks, Project Detection & Zero-Prefix Classifier (`v0.1-m2`)
- **Tasks:** Implement `ace-shell` (`zsh`/`bash`/`pwsh` hook scripts + `tree-sitter` & `$PATH` classifier) and `ace-env` (`gix` + toolchain detectors).
- **Acceptance Criteria:**
  - `cd` updates status bar Git branch/dirty indicator and detected language badges asynchronously without prompt lag.
  - Typing `git status` executes in shell with zero delay; typing `find all rust files modified today` spawns an inline AI block; `Tab` toggles `[SHELL]`/`[AI]` mode on the active prompt.

#### Milestone 3: Streaming Agent Loop, Diffs & Keyring Auth (`v0.1-m3`)
- **Tasks:** Implement `ace-agent` SSE streaming, `keyring` credential CLI/UI setup (`ace-term auth`), tool execution (`read_file`, `write_file`, `run_shell`), and `AiBlockView` Markdown + `imara-diff` rendering.
- **Acceptance Criteria:**
  - Multi-turn AI conversation streams smoothly inside terminal scrollback without flicker.
  - File edits render unified diffs with working `[Accept]` (writes file) and `[Reject]` buttons; shell tool calls pause for user confirmation in `default` mode.

#### Milestone 4: Inline Graphics, MCP & CI Hardening (`v0.1-m4` -> `v0.3`)
- **Tasks:** Add Kitty graphics / `resvg` image rendering in `TerminalCanvasView` and `AiBlockView`, wire `rmcp` client in `ace-agent`, and configure GitHub Actions CI (`clippy`, `test`, `cargo-deny`, cross-platform release builds).
- **Acceptance Criteria:**
  - `cargo deny check licenses` exits `0`.
  - Release binaries run cleanly on non-AVX2/older x86_64 and aarch64 machines.

---

## 3. Coding Agent Kickoff Prompt

Pass `SPEC.md` into your repository root and give your first AI coding agent this prompt:

> Read `SPEC.md` completely. Initialize the 7-crate Rust Cargo workspace (`ace-vt`, `ace-pty`, `ace-shell`, `ace-env`, `ace-agent`, `ace-config`, `ace-ui`) using Rust 2024 edition and `MIT OR Apache-2.0` licensing, and add a `deny.toml` enforcing permissive licenses. Then implement **Milestone 1** (`ace-vt`, `ace-pty`, `ace-config`, and `ace-ui` with `gpui-kit` rendering an interactive shell with `config.toml` hot-reloading). Verify everything compiles and passes `cargo clippy --workspace -- -D warnings`.
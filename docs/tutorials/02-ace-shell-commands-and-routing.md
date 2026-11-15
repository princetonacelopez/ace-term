# Tutorial 02: Ace Shell Commands, Routing, and Shortcuts

Ace Term (`ace-shell`) does **not** replace your shell — it wraps your existing native shell (`zsh`, `bash`, or `pwsh`) with non-destructive hooks, zero-prefix AI routing, and project context detection.

---

## 1. Native Shell Commands

Every command that works in your normal terminal works in Ace Term:

- **Shell builtins**: `cd`, `pwd`, `export`, `alias`, `unalias`, `source`, `echo`, `printf`, `history`, `jobs`, `fg`, `bg`, `kill`, `exit`, `which`, `type`, etc.
- **Executables on `$PATH`**: `git`, `cargo`, `rustc`, `node`, `npm`, `python`, `go`, `docker`, `vim`, `htop`, etc.
- **Explicit paths**: `./scripts/build.sh`, `../tool`, `/usr/local/bin/app`, `~/bin/my-cli`.
- **Shell pipelines, redirects, and env prefixes**:
  - Pipes & chains: `git status | head -n 10`, `cargo test && cargo build`
  - Redirects & subshells: `ls -la > files.txt`, `echo $(git branch --show-current)`
  - Environment prefixes: `RUST_LOG=debug cargo run -p ace-ui`
- **Your existing profile**: Your `~/.zshrc`, `~/.bashrc`, `~/.bash_profile`, aliases, functions, and prompts (such as Starship) load automatically.

---

## 2. Zero-Prefix AI Routing (`[AUTO]` Mode)

When the status-bar pill is in `[AUTO]` mode (and `ai.auto_intercept = true`), Ace Term inspects what you type on `Enter` in under 2ms using `tree-sitter-bash` and a `$PATH` executable cache:

| Input Example | Destination | Why |
| --- | --- | --- |
| `ls -la` | Shell PTY | `ls` is on `$PATH` |
| `cd ~/projects` | Shell PTY | `cd` is a shell builtin |
| `./deploy.sh` | Shell PTY | Explicit path (`./`) |
| `cat Cargo.toml \| grep name` | Shell PTY | Valid shell pipeline syntax |
| `gti` | Shell PTY | Single unknown word (preserves normal typo feedback) |
| `explain why the build failed` | AI Agent | 2+ words, not a command or shell syntax |
| `what?` | AI Agent | Ends with `?` |
| Typing inside `vim` / `htop` | Shell PTY | Alternate screen always routes directly to PTY |

### Switching Modes Manually

- Press `Shift+Tab` (or `Tab` on an empty line), or click the `[AUTO]` / `[AI]` / `[SHELL]` pill in the bottom-right status bar:
  - `[AUTO]`: Smart zero-prefix classification.
  - `[AI]`: Forces all non-empty prompts to the AI agent.
  - `[SHELL]`: Forces all input directly to the PTY shell.

---

## 3. `ace-term` CLI Commands

Run these from any terminal (including inside Ace Term):

```sh
# Store an API key in the OS keychain (for cloud providers)
ace-term auth set anthropic
ace-term auth set openai

# Check configured cloud provider keys
ace-term auth status

# Remove a stored key from the OS keychain
ace-term auth clear anthropic
```

> Local providers (`antigravity` via `agy`, `ollama`, and `lmstudio`) do not require an API key.

---

## 4. Inline AI & Status Bar Actions

- **Non-zero exit quick-fix**: When a command exits with a non-zero status, the status bar displays `Exit N — Ask AI to fix?`. Click it to send the recent terminal output to the AI.
- **File edit diffs**: When the AI proposes a file edit, an inline unified diff appears with `[Accept]` and `[Reject]` buttons.
- **Shell tool approval**: When `approval_mode = "default"`, AI-proposed shell commands pause for `[Approve]` or `[Deny]`.

---

## 5. Essential Keyboard Shortcuts & Mouse Controls

| Action | Linux / Windows | macOS |
| --- | --- | --- |
| Toggle Input Mode (`[AUTO]` / `[AI]`) | `Shift+Tab` (or `Tab` on empty line) | `Shift+Tab` (or `Tab` on empty line) |
| Open / Close Settings | `Ctrl+,` | `Cmd+,` |
| Copy Selection | `Ctrl+Shift+C` | `Cmd+C` |
| Paste Clipboard | `Ctrl+Shift+V` | `Cmd+V` |
| Send Interrupt (`SIGINT`) | `Ctrl+C` | `Ctrl+C` |
| Context Menu (Copy, Paste, Clear, Settings) | Right-click | Right-click |
| Close Context Menu / Settings | `Esc` | `Esc` |

---

## 6. Recommended Next Steps to Learn

1. **Setup & Configuration**: Read [Tutorial 01: Setup and Usage](01-phase-1-setup-and-usage.md) to configure themes, fonts, and `~/.config/ace-term/config.toml`.
2. **Local AI & Models**: Open Settings (`Ctrl+,` / `Cmd+,`) to view auto-detected local AI runtimes (`Antigravity`, `Ollama`, `LM Studio`) and switch models on the fly.
3. **Project Context**: `cd` into any Git, Rust, Node, Python, or Go repository to see live `git:branch*` and toolchain indicators update in the status bar.

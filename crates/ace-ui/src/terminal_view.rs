//! `TerminalCanvasView`: GPUI terminal canvas, top bar, context menu, and settings UI.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ace_agent::{
    AgentEvent, ChatMessage, DiscoveredProvider, discover_local_ai, is_local_provider,
    run_agent_turn,
};
use ace_config::secrets::{load_api_key, store_api_key};
use ace_config::{AceConfig, ApprovalMode, ConfigStore, CursorStyle};
use ace_env::{EnvDetector, EnvSnapshot};
use ace_pty::{PtyWriter, spawn_shell_with_options};
use ace_shell::{
    CommandClassifier, InputMode, OscScanner, RouteTarget, ShellEvent, prepare_shell_launch,
};
use ace_vt::{
    GridSnapshot, RgbColor, SelectionType, TerminalGrid, TerminalSize, VtEvent, VtTerminal,
};
use gpui_kit::*;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use crate::ai_block::{AiBlock, AiBlockColors, render_ai_block};

const TOP_BAR_HEIGHT: f32 = 34.0;
const STATUS_BAR_HEIGHT: f32 = 28.0;

/// Theme palette derived from `AceConfig::ui.theme`.
#[derive(Debug, Clone, Copy)]
struct ThemePalette {
    bg: RgbColor,
    surface: RgbColor,
    surface_hover: RgbColor,
    border: RgbColor,
    fg: RgbColor,
    muted: RgbColor,
    accent: RgbColor,
    cursor: RgbColor,
}

impl ThemePalette {
    fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "dracula" => Self {
                bg: RgbColor::new(0x28, 0x2a, 0x36),
                surface: RgbColor::new(0x21, 0x22, 0x2c),
                surface_hover: RgbColor::new(0x34, 0x37, 0x46),
                border: RgbColor::new(0x44, 0x47, 0x5a),
                fg: RgbColor::new(0xf8, 0xf8, 0xf2),
                muted: RgbColor::new(0x9a, 0xa2, 0xc2),
                accent: RgbColor::new(0xbd, 0x93, 0xf9),
                cursor: RgbColor::new(0xff, 0x79, 0xc6),
            },
            "nord" => Self {
                bg: RgbColor::new(0x2e, 0x34, 0x40),
                surface: RgbColor::new(0x27, 0x2c, 0x36),
                surface_hover: RgbColor::new(0x3b, 0x42, 0x52),
                border: RgbColor::new(0x43, 0x4c, 0x5e),
                fg: RgbColor::new(0xd8, 0xde, 0xe9),
                muted: RgbColor::new(0x88, 0x92, 0xa6),
                accent: RgbColor::new(0x88, 0xc0, 0xd0),
                cursor: RgbColor::new(0x88, 0xc0, 0xd0),
            },
            "default light" | "light" => Self {
                bg: RgbColor::new(0xf8, 0xf9, 0xfc),
                surface: RgbColor::new(0xee, 0xf0, 0xf5),
                surface_hover: RgbColor::new(0xe2, 0xe5, 0xee),
                border: RgbColor::new(0xd0, 0xd5, 0xe0),
                fg: RgbColor::new(0x1e, 0x1e, 0x2e),
                muted: RgbColor::new(0x6c, 0x6f, 0x85),
                accent: RgbColor::new(0x1e, 0x66, 0xf5),
                cursor: RgbColor::new(0x1e, 0x66, 0xf5),
            },
            _ => Self {
                bg: RgbColor::new(0x11, 0x11, 0x1b),
                surface: RgbColor::new(0x18, 0x18, 0x25),
                surface_hover: RgbColor::new(0x26, 0x26, 0x38),
                border: RgbColor::new(0x31, 0x32, 0x44),
                fg: RgbColor::new(0xcd, 0xd6, 0xf4),
                muted: RgbColor::new(0x7f, 0x84, 0x9c),
                accent: RgbColor::new(0x89, 0xb4, 0xfa),
                cursor: RgbColor::new(0xf5, 0xe0, 0xdc),
            },
        }
    }
}

fn to_hsla(c: RgbColor, alpha: f32) -> Hsla {
    Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: alpha,
    }
    .into()
}

#[derive(Debug, Clone)]
struct ContextMenuState {
    x: f32,
    y: f32,
    has_selection: bool,
}

#[derive(Debug, Clone)]
struct CachedFont {
    requested_family: String,
    resolved_family: SharedString,
    available_mono_presets: Vec<String>,
}

/// Interactive GPU-accelerated terminal view.
pub struct TerminalCanvasView {
    focus_handle: FocusHandle,
    vt: Arc<Mutex<VtTerminal>>,
    pty_writer: Arc<Mutex<Option<PtyWriter>>>,
    rt: Arc<tokio::runtime::Runtime>,
    config_store: Arc<ConfigStore>,
    cell_metrics: Arc<Mutex<(f32, f32)>>,
    cached_font: Arc<Mutex<Option<CachedFont>>>,
    selecting: bool,
    last_title: Option<String>,
    context_menu: Option<ContextMenuState>,
    settings_open: bool,
    draft_config: AceConfig,
    settings_status: Option<String>,
    exited: Arc<AtomicBool>,
    classifier: Arc<CommandClassifier>,
    input_mode: InputMode,
    line_buffer: String,
    env_snapshot: Arc<Mutex<EnvSnapshot>>,
    last_failed_exit: Arc<Mutex<Option<i32>>>,
    ai_block: Option<AiBlock>,
    ai_history: Vec<ChatMessage>,
    agent_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    wake_tx: watch::Sender<u64>,
    discovered_providers: Arc<Mutex<Vec<DiscoveredProvider>>>,
    discovering_ai: Arc<AtomicBool>,
}

impl TerminalCanvasView {
    /// Creates a new `TerminalCanvasView`, spawns the user's default shell, and starts the PTY reader loop.
    pub fn new(
        rt: Arc<tokio::runtime::Runtime>,
        config_store: Arc<ConfigStore>,
        mut wake_rx: watch::Receiver<u64>,
        wake_tx: watch::Sender<u64>,
        cx: &mut Context<Self>,
    ) -> Self {
        let cfg = config_store.get();
        let initial_size = TerminalSize::new(100, 32);
        let vt = Arc::new(Mutex::new(VtTerminal::new(
            initial_size,
            cfg.terminal.scrollback_lines,
        )));
        let pty_writer: Arc<Mutex<Option<PtyWriter>>> = Arc::new(Mutex::new(None));

        let env_detector = EnvDetector::new();
        let initial_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let env_snapshot = Arc::new(Mutex::new(EnvSnapshot {
            cwd: initial_cwd.clone(),
            ..EnvSnapshot::default()
        }));
        let last_failed_exit = Arc::new(Mutex::new(None));

        let env_for_init = Arc::clone(&env_snapshot);
        let wake_for_env_init = wake_tx.clone();
        let detector_init = env_detector.clone();
        rt.spawn(async move {
            let snap = detector_init.detect(initial_cwd).await;
            *env_for_init.lock().expect("env_snapshot lock poisoned") = snap;
            let next = wake_for_env_init.borrow().wrapping_add(1);
            let _ = wake_for_env_init.send(next);
        });

        let shell_cfg = cfg.terminal.shell.clone();
        let vt_for_pty = Arc::clone(&vt);
        let writer_slot = Arc::clone(&pty_writer);
        let wake_for_pty = wake_tx.clone();
        let rt_for_spawn = Arc::clone(&rt);
        let exited = Arc::new(AtomicBool::new(false));
        let exited_for_pty = Arc::clone(&exited);
        let env_for_pty = Arc::clone(&env_snapshot);
        let failed_for_pty = Arc::clone(&last_failed_exit);

        rt_for_spawn.spawn(async move {
            let cwd = std::env::current_dir().ok();
            let launch = prepare_shell_launch(Some(shell_cfg.as_str()));
            let spawned = match spawn_shell_with_options(
                Some(&launch.program),
                &launch.args,
                &launch.extra_env,
                cwd.as_deref(),
                initial_size.cols as u16,
                initial_size.rows as u16,
            )
            .await
            {
                Ok(s) => s,
                Err(err) => {
                    error!(error = %err, "failed to spawn shell PTY");
                    return;
                }
            };

            let writer = spawned.writer;
            {
                let mut slot = writer_slot.lock().expect("writer_slot lock poisoned");
                *slot = Some(writer.clone());
            }

            let mut reader = spawned.reader;
            let mut child = spawned.child;
            let mut osc_scanner = OscScanner::default();
            let mut buf = vec![0u8; 16 * 1024];
            let mut seq: u64 = 1;

            let vt_for_timer = Arc::clone(&vt_for_pty);
            let wake_for_timer = wake_for_pty.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(200));
                loop {
                    interval.tick().await;
                    let flushed = {
                        let mut guard = vt_for_timer.lock().expect("vt lock poisoned");
                        guard.flush_sync_if_expired()
                    };
                    if flushed {
                        let next = wake_for_timer.borrow().wrapping_add(1);
                        let _ = wake_for_timer.send(next);
                    }
                }
            });

            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => {
                        info!("PTY EOF reached");
                        exited_for_pty.store(true, Ordering::Release);
                        let next = wake_for_pty.borrow().wrapping_add(1);
                        let _ = wake_for_pty.send(next);
                        break;
                    }
                    Ok(n) => {
                        for shell_ev in osc_scanner.feed(&buf[..n]) {
                            match shell_ev {
                                ShellEvent::CwdChanged(new_cwd) => {
                                    let detector = env_detector.clone();
                                    let env_slot = Arc::clone(&env_for_pty);
                                    let wake = wake_for_pty.clone();
                                    tokio::spawn(async move {
                                        let snap = detector.detect(PathBuf::from(new_cwd)).await;
                                        *env_slot.lock().expect("env_snapshot lock poisoned") =
                                            snap;
                                        let next = wake.borrow().wrapping_add(1);
                                        let _ = wake.send(next);
                                    });
                                }
                                ShellEvent::CommandFinished(Some(code)) => {
                                    *failed_for_pty
                                        .lock()
                                        .expect("last_failed_exit lock poisoned") =
                                        (code != 0).then_some(code);
                                    seq = seq.wrapping_add(1);
                                    let _ = wake_for_pty.send(seq);
                                }
                                ShellEvent::PromptStart
                                | ShellEvent::CommandStart
                                | ShellEvent::CommandExecuted
                                | ShellEvent::CommandFinished(None) => {}
                            }
                        }

                        let events = {
                            let mut guard = vt_for_pty.lock().expect("vt lock poisoned");
                            guard.feed_bytes(&buf[..n]);
                            let evs = guard.drain_events();
                            if guard.take_dirty() {
                                seq = seq.wrapping_add(1);
                                let _ = wake_for_pty.send(seq);
                            }
                            evs
                        };

                        for ev in events {
                            if let VtEvent::PtyWrite(reply) = ev {
                                let _ = writer.write_all(reply.as_bytes()).await;
                            }
                        }
                    }
                    Err(err) => {
                        error!(error = %err, "PTY read error");
                        exited_for_pty.store(true, Ordering::Release);
                        let next = wake_for_pty.borrow().wrapping_add(1);
                        let _ = wake_for_pty.send(next);
                        break;
                    }
                }
            }

            let _ = child.wait().await;
        });

        cx.spawn(async move |this, cx| {
            while wake_rx.changed().await.is_ok() {
                let updated = this.update(cx, |_, cx| {
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();

        let view = Self {
            focus_handle: cx.focus_handle(),
            vt,
            pty_writer,
            rt,
            draft_config: (*cfg).clone(),
            config_store,
            cell_metrics: Arc::new(Mutex::new((8.4, 19.0))),
            cached_font: Arc::new(Mutex::new(None)),
            selecting: false,
            last_title: None,
            context_menu: None,
            settings_open: false,
            settings_status: None,
            exited,
            classifier: Arc::new(CommandClassifier::new()),
            input_mode: InputMode::Auto,
            line_buffer: String::new(),
            env_snapshot,
            last_failed_exit,
            ai_block: None,
            ai_history: Vec::new(),
            agent_rx: None,
            wake_tx,
            discovered_providers: Arc::new(Mutex::new(Vec::new())),
            discovering_ai: Arc::new(AtomicBool::new(false)),
        };
        view.refresh_ai_discovery();
        view
    }

    fn refresh_ai_discovery(&self) {
        if self
            .discovering_ai
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let providers_slot = Arc::clone(&self.discovered_providers);
        let flag = Arc::clone(&self.discovering_ai);
        let wake_tx = self.wake_tx.clone();
        self.rt.spawn(async move {
            let discovered = discover_local_ai().await;
            *providers_slot
                .lock()
                .expect("discovered_providers lock poisoned") = discovered;
            flag.store(false, Ordering::Release);
            let next = wake_tx.borrow().wrapping_add(1);
            let _ = wake_tx.send(next);
        });
    }

    fn write_to_pty(&self, bytes: Vec<u8>) {
        let writer_opt = self
            .pty_writer
            .lock()
            .expect("writer_slot lock poisoned")
            .clone();
        if let Some(writer) = writer_opt {
            self.rt.spawn(async move {
                if let Err(err) = writer.write_all(&bytes).await {
                    error!(error = %err, "failed to write to PTY");
                }
            });
        }
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let text = self.vt.lock().expect("vt lock poisoned").selection_text();
        if let Some(selected) = text
            && !selected.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(selected));
        }
        self.context_menu = None;
        cx.notify();
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard()
            && let Some(text) = item.text()
        {
            self.write_to_pty(text.into_bytes());
        }
        self.context_menu = None;
        cx.notify();
    }

    fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.input_mode = self.input_mode.toggle();
        cx.notify();
    }

    fn spawn_agent_turn(&mut self, prompt: String, cx: &mut Context<Self>) {
        let cfg = (*self.config_store.get()).clone();
        let env = self
            .env_snapshot
            .lock()
            .expect("env_snapshot lock poisoned")
            .clone();
        let history = self.ai_history.clone();
        let (tx, rx) = mpsc::unbounded_channel();

        self.ai_block = Some(AiBlock::new(prompt.clone()));
        self.agent_rx = Some(rx);

        let rt = Arc::clone(&self.rt);
        rt.spawn(async move {
            run_agent_turn(prompt, history, env, cfg, tx).await;
        });

        cx.spawn(async move |this, cx| {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(24)).await;
                let keep_going = this.update(cx, |view, cx| {
                    let changed = view.drain_agent_events();
                    if changed {
                        cx.notify();
                    }
                    view.agent_rx.is_some()
                });
                match keep_going {
                    Ok(true) => {}
                    _ => break,
                }
            }
        })
        .detach();

        cx.notify();
    }

    fn drain_agent_events(&mut self) -> bool {
        let mut drained = Vec::new();
        let mut finished = false;
        if let Some(rx) = self.agent_rx.as_mut() {
            while let Ok(ev) = rx.try_recv() {
                if matches!(ev, AgentEvent::Done | AgentEvent::Error(_)) {
                    finished = true;
                }
                drained.push(ev);
            }
        }
        if drained.is_empty() && !finished {
            return false;
        }
        if let Some(block) = self.ai_block.as_mut() {
            for ev in drained {
                block.apply_event(ev);
            }
            if finished && !block.response.is_empty() {
                self.ai_history.push(ChatMessage {
                    role: "user".to_string(),
                    content: block.prompt.clone(),
                });
                self.ai_history.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: block.response.clone(),
                });
                if self.ai_history.len() > 16 {
                    let drop_n = self.ai_history.len() - 16;
                    self.ai_history.drain(..drop_n);
                }
            }
        }
        if finished {
            self.agent_rx = None;
        }
        true
    }

    fn clear_terminal(&mut self, cx: &mut Context<Self>) {
        self.vt.lock().expect("vt lock poisoned").clear_selection();
        self.write_to_pty(vec![0x0c]); // Ctrl+L
        self.context_menu = None;
        cx.notify();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.draft_config = (*self.config_store.get()).clone();
            self.settings_status = None;
            if self
                .discovered_providers
                .lock()
                .expect("discovered_providers lock poisoned")
                .is_empty()
            {
                self.refresh_ai_discovery();
            }
        }
        self.context_menu = None;
        cx.notify();
    }

    fn save_draft_settings(&mut self, cx: &mut Context<Self>) {
        match self.config_store.save_global(self.draft_config.clone()) {
            Ok(_) => {
                *self.cached_font.lock().expect("cached_font lock poisoned") = None;
                self.settings_status = Some("Saved to config.toml".to_string());
            }
            Err(err) => {
                warn!(error = %err, "failed to save settings");
                self.settings_status = Some(format!("Save failed: {err}"));
            }
        }
        cx.notify();
    }

    fn pixel_to_cell(&self, pos: Point<Pixels>, padding: f32) -> (usize, usize) {
        let (cw, lh) = *self
            .cell_metrics
            .lock()
            .expect("cell_metrics lock poisoned");
        let x: f32 = pos.x.into();
        let y: f32 = pos.y.into();
        let col = ((x - padding).max(0.0) / cw.max(1.0)).floor() as usize;
        let row = ((y - TOP_BAR_HEIGHT - padding).max(0.0) / lh.max(1.0)).floor() as usize;
        (col, row)
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let mods = &ks.modifiers;

        if ks.key == "escape" && (self.context_menu.is_some() || self.settings_open) {
            self.context_menu = None;
            self.settings_open = false;
            cx.notify();
            return;
        }

        // Settings shortcut: Ctrl+, or Cmd+,
        if (mods.control || mods.platform) && ks.key == "," {
            self.toggle_settings(cx);
            return;
        }

        if self.settings_open {
            return;
        }

        // Copy: Ctrl+Shift+C or Cmd+C (when selection exists)
        if ((mods.control && mods.shift) || mods.platform) && ks.key.eq_ignore_ascii_case("c") {
            let text = self.vt.lock().expect("vt lock poisoned").selection_text();
            if let Some(selected) = text
                && !selected.is_empty()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(selected));
                return;
            }
        }

        // Paste: Ctrl+Shift+V or Cmd+V
        if ((mods.control && mods.shift) || mods.platform) && ks.key.eq_ignore_ascii_case("v") {
            self.paste_clipboard(cx);
            return;
        }

        let is_alt = self.vt.lock().expect("vt lock poisoned").is_alt_screen();

        // Mode toggle: Shift+Tab anytime (outside alt-screen), or Tab when line buffer is empty
        if !is_alt && ks.key == "tab" && (mods.shift || self.line_buffer.trim().is_empty()) {
            self.toggle_mode(cx);
            return;
        }

        // Zero-prefix AI routing on Enter
        if !is_alt && ks.key == "enter" {
            let line = std::mem::take(&mut self.line_buffer);
            let cfg = self.config_store.get();
            let effective_mode = if !cfg.ai.auto_intercept && self.input_mode == InputMode::Auto {
                InputMode::Shell
            } else {
                self.input_mode
            };
            if self.classifier.classify(&line, effective_mode, is_alt) == RouteTarget::Agent {
                self.context_menu = None;
                self.vt.lock().expect("vt lock poisoned").clear_selection();
                // Clear the shell's line buffer without executing in the shell
                self.write_to_pty(vec![0x15]);
                self.spawn_agent_turn(line.trim().to_string(), cx);
                return;
            }
        }

        if let Some(bytes) = encode_keystroke(ks) {
            if !is_alt {
                update_line_buffer(&mut self.line_buffer, ks, &bytes);
            }
            self.context_menu = None;
            {
                let mut guard = self.vt.lock().expect("vt lock poisoned");
                guard.clear_selection();
                guard.scroll_to_bottom();
            }
            self.write_to_pty(bytes);
        }
    }

    fn render_top_bar(&self, palette: ThemePalette, cx: &mut Context<Self>) -> impl IntoElement {
        let title_text = self
            .last_title
            .clone()
            .unwrap_or_else(|| "Shell".to_string());

        gpui_kit::component::TitleBar::new()
            .bg(to_hsla(palette.surface, 1.0))
            .border_color(to_hsla(palette.border, 1.0))
            .child(
                div()
                    .w_full()
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(to_hsla(palette.accent, 0.16))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(to_hsla(palette.accent, 1.0))
                                    .child("Ace Term"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(to_hsla(palette.muted, 1.0))
                                    .child(title_text),
                            ),
                    )
                    .child(top_bar_button(
                        "settings-btn",
                        if self.settings_open {
                            "Close Settings"
                        } else {
                            "Settings"
                        },
                        true,
                        palette,
                        cx.listener(|this, _, _, cx| this.toggle_settings(cx)),
                    )),
            )
    }

    fn render_context_menu(
        &self,
        menu: &ContextMenuState,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(180.0))
            .p(px(4.0))
            .rounded(px(8.0))
            .bg(to_hsla(palette.surface, 0.98))
            .border_1()
            .border_color(to_hsla(palette.border, 1.0))
            .shadow_md()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(context_menu_item(
                "ctx-copy",
                "Copy",
                "Ctrl+Shift+C",
                menu.has_selection,
                palette,
                cx.listener(|this, _, _, cx| this.copy_selection(cx)),
            ))
            .child(context_menu_item(
                "ctx-paste",
                "Paste",
                "Ctrl+Shift+V",
                true,
                palette,
                cx.listener(|this, _, _, cx| this.paste_clipboard(cx)),
            ))
            .child(context_menu_item(
                "ctx-clear",
                "Clear",
                "Ctrl+L",
                true,
                palette,
                cx.listener(|this, _, _, cx| this.clear_terminal(cx)),
            ))
            .child(
                div()
                    .my(px(2.0))
                    .h(px(1.0))
                    .w_full()
                    .bg(to_hsla(palette.border, 1.0)),
            )
            .child(context_menu_item(
                "ctx-settings",
                "Settings",
                "Ctrl+,",
                true,
                palette,
                cx.listener(|this, _, _, cx| this.toggle_settings(cx)),
            ))
    }

    fn render_settings_modal(
        &self,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let draft = &self.draft_config;
        let available_fonts = self
            .cached_font
            .lock()
            .expect("cached_font lock poisoned")
            .as_ref()
            .map(|c| c.available_mono_presets.clone())
            .unwrap_or_else(|| {
                vec![
                    "JetBrainsMono Nerd Font Mono".to_string(),
                    "Adwaita Mono".to_string(),
                    "DejaVu Sans Mono".to_string(),
                    "monospace".to_string(),
                ]
            });

        div()
            .absolute()
            .inset_0()
            .bg(to_hsla(RgbColor::new(0, 0, 0), 0.45))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.settings_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(560.0))
                    .max_h(px(640.0))
                    .p(px(18.0))
                    .rounded(px(12.0))
                    .bg(to_hsla(palette.surface, 1.0))
                    .border_1()
                    .border_color(to_hsla(palette.border, 1.0))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(to_hsla(palette.fg, 1.0))
                                    .child("Terminal Settings"),
                            )
                            .child(top_bar_button(
                                "settings-close-x",
                                "Close",
                                true,
                                palette,
                                cx.listener(|this, _, _, cx| {
                                    this.settings_open = false;
                                    cx.notify();
                                }),
                            )),
                    )
                    // Theme selector
                    .child(setting_row(
                        "Theme",
                        palette,
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            ["Default Dark", "Default Light", "Dracula", "Nord"]
                                .into_iter()
                                .map(|theme| {
                                    let active = draft.ui.theme.eq_ignore_ascii_case(theme);
                                    let theme_owned = theme.to_string();
                                    pill_button(
                                        format!("theme-{theme}"),
                                        theme,
                                        active,
                                        palette,
                                        cx.listener(move |this, _, _, cx| {
                                            this.draft_config.ui.theme = theme_owned.clone();
                                            cx.notify();
                                        }),
                                    )
                                }),
                        ),
                    ))
                    // Font family selector
                    .child(setting_row(
                        "Font Family",
                        palette,
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            available_fonts.into_iter().take(4).map(|fam| {
                                let active = draft.font.family.eq_ignore_ascii_case(&fam);
                                let fam_clone = fam.clone();
                                pill_button(
                                    format!("font-{fam}"),
                                    &fam,
                                    active,
                                    palette,
                                    cx.listener(move |this, _, _, cx| {
                                        this.draft_config.font.family = fam_clone.clone();
                                        cx.notify();
                                    }),
                                )
                            }),
                        ),
                    ))
                    // Font size stepper
                    .child(setting_row(
                        "Font Size",
                        palette,
                        stepper(
                            "font-size",
                            format!("{:.0} px", draft.font.size),
                            palette,
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.font.size =
                                    (this.draft_config.font.size - 1.0).max(9.0);
                                cx.notify();
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.font.size =
                                    (this.draft_config.font.size + 1.0).min(32.0);
                                cx.notify();
                            }),
                        ),
                    ))
                    // Line height stepper
                    .child(setting_row(
                        "Line Height",
                        palette,
                        stepper(
                            "line-height",
                            format!("{:.2}", draft.font.line_height),
                            palette,
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.font.line_height =
                                    ((this.draft_config.font.line_height - 0.05) * 100.0).round()
                                        / 100.0;
                                this.draft_config.font.line_height =
                                    this.draft_config.font.line_height.max(1.0);
                                cx.notify();
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.font.line_height =
                                    ((this.draft_config.font.line_height + 0.05) * 100.0).round()
                                        / 100.0;
                                this.draft_config.font.line_height =
                                    this.draft_config.font.line_height.min(2.0);
                                cx.notify();
                            }),
                        ),
                    ))
                    // Cursor style selector
                    .child(setting_row(
                        "Cursor Style",
                        palette,
                        div().flex().gap(px(6.0)).children(
                            [
                                ("Block", CursorStyle::Block),
                                ("Bar", CursorStyle::Bar),
                                ("Underline", CursorStyle::Underline),
                            ]
                            .into_iter()
                            .map(|(label, style)| {
                                let active = draft.terminal.cursor_style == style;
                                pill_button(
                                    format!("cursor-{label}"),
                                    label,
                                    active,
                                    palette,
                                    cx.listener(move |this, _, _, cx| {
                                        this.draft_config.terminal.cursor_style = style;
                                        cx.notify();
                                    }),
                                )
                            }),
                        ),
                    ))
                    // Padding stepper
                    .child(setting_row(
                        "Padding",
                        palette,
                        stepper(
                            "padding",
                            format!("{} px", draft.ui.padding),
                            palette,
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.ui.padding =
                                    this.draft_config.ui.padding.saturating_sub(2);
                                cx.notify();
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.ui.padding =
                                    (this.draft_config.ui.padding + 2).min(32);
                                cx.notify();
                            }),
                        ),
                    ))
                    // Opacity stepper
                    .child(setting_row(
                        "Opacity",
                        palette,
                        stepper(
                            "opacity",
                            format!("{:.0}%", draft.ui.opacity * 100.0),
                            palette,
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.ui.opacity =
                                    ((this.draft_config.ui.opacity - 0.05) * 100.0).round() / 100.0;
                                this.draft_config.ui.opacity =
                                    this.draft_config.ui.opacity.max(0.6);
                                cx.notify();
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.draft_config.ui.opacity =
                                    ((this.draft_config.ui.opacity + 0.05) * 100.0).round() / 100.0;
                                this.draft_config.ui.opacity =
                                    this.draft_config.ui.opacity.min(1.0);
                                cx.notify();
                            }),
                        ),
                    ))
                    // AI Provider selector
                    .child({
                        let discovered = self
                            .discovered_providers
                            .lock()
                            .expect("discovered_providers lock poisoned")
                            .clone();
                        let discovering = self.discovering_ai.load(Ordering::Acquire);
                        let provider_buttons: Vec<(String, String, String)> = if discovered
                            .is_empty()
                        {
                            vec![
                                (
                                    "antigravity".to_string(),
                                    "Antigravity".to_string(),
                                    "gemini-4-argon-medium".to_string(),
                                ),
                                (
                                    "anthropic".to_string(),
                                    "Anthropic".to_string(),
                                    "claude-sonnet-4-6".to_string(),
                                ),
                                (
                                    "openai".to_string(),
                                    "OpenAI".to_string(),
                                    "gpt-4o".to_string(),
                                ),
                            ]
                        } else {
                            discovered
                                .iter()
                                .map(|p| {
                                    let badge = if p.local && p.installed {
                                        format!("{} (Local)", p.label)
                                    } else if p.local {
                                        format!("{} (Not found)", p.label)
                                    } else {
                                        p.label.clone()
                                    };
                                    let default_model =
                                        p.models.first().map(|m| m.id.clone()).unwrap_or_default();
                                    (p.id.clone(), badge, default_model)
                                })
                                .collect()
                        };

                        setting_row(
                            "AI Provider",
                            palette,
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .children(provider_buttons.into_iter().map(
                                    |(provider, label, default_model)| {
                                        let active =
                                            draft.ai.provider.eq_ignore_ascii_case(&provider);
                                        let prov_owned = provider.clone();
                                        let model_owned = default_model;
                                        pill_button(
                                            format!("provider-{provider}"),
                                            &label,
                                            active,
                                            palette,
                                            cx.listener(move |this, _, _, cx| {
                                                this.draft_config.ai.provider = prov_owned.clone();
                                                if !model_owned.is_empty() {
                                                    this.draft_config.ai.model =
                                                        model_owned.clone();
                                                }
                                                cx.notify();
                                            }),
                                        )
                                    },
                                ))
                                .child(top_bar_button(
                                    "refresh-ai-btn",
                                    if discovering {
                                        "Scanning..."
                                    } else {
                                        "Refresh"
                                    },
                                    !discovering,
                                    palette,
                                    cx.listener(|this, _, _, cx| {
                                        this.refresh_ai_discovery();
                                        cx.notify();
                                    }),
                                )),
                        )
                    })
                    .child({
                        let discovered = self
                            .discovered_providers
                            .lock()
                            .expect("discovered_providers lock poisoned")
                            .clone();
                        let discovering = self.discovering_ai.load(Ordering::Acquire);
                        let active_provider = discovered
                            .iter()
                            .find(|p| p.id.eq_ignore_ascii_case(&draft.ai.provider))
                            .cloned();

                        let model_content = match active_provider {
                            Some(p) if !p.models.is_empty() => div()
                                .flex()
                                .flex_wrap()
                                .gap(px(4.0))
                                .children(p.models.into_iter().map(|m| {
                                    let active = draft.ai.model == m.id;
                                    let model_id = m.id.clone();
                                    pill_button(
                                        format!("model-{}", m.id),
                                        &m.label,
                                        active,
                                        palette,
                                        cx.listener(move |this, _, _, cx| {
                                            this.draft_config.ai.model = model_id.clone();
                                            cx.notify();
                                        }),
                                    )
                                }))
                                .into_any_element(),
                            Some(p) => div()
                                .text_size(px(11.0))
                                .text_color(to_hsla(palette.muted, 1.0))
                                .child(if discovering {
                                    "Scanning local models...".to_string()
                                } else {
                                    format!("No models listed ({})", p.detail)
                                })
                                .into_any_element(),
                            None => div()
                                .text_size(px(11.0))
                                .text_color(to_hsla(palette.muted, 1.0))
                                .child(if discovering {
                                    "Detecting local AI & models...".to_string()
                                } else {
                                    draft.ai.model.clone()
                                })
                                .into_any_element(),
                        };

                        setting_row("AI Model", palette, model_content)
                    })
                    // Approval mode selector
                    .child(setting_row(
                        "AI Approval",
                        palette,
                        div().flex().gap(px(6.0)).children(
                            [
                                ("Default (Ask)", ApprovalMode::Default),
                                ("Auto", ApprovalMode::Auto),
                            ]
                            .into_iter()
                            .map(|(label, mode)| {
                                let active = draft.ai.approval_mode == mode;
                                pill_button(
                                    format!("approval-{label}"),
                                    label,
                                    active,
                                    palette,
                                    cx.listener(move |this, _, _, cx| {
                                        this.draft_config.ai.approval_mode = mode;
                                        cx.notify();
                                    }),
                                )
                            }),
                        ),
                    ))
                    // Keyring API key status + paste from clipboard
                    .child(setting_row("API Key", palette, {
                        let provider_name = draft.ai.provider.clone();
                        let local_provider = is_local_provider(&provider_name);
                        let configured = !local_provider
                            && load_api_key(&provider_name).ok().flatten().is_some();
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(to_hsla(
                                        if local_provider || configured {
                                            RgbColor::new(0xa6, 0xe3, 0xa1)
                                        } else {
                                            palette.muted
                                        },
                                        1.0,
                                    ))
                                    .child(if local_provider {
                                        "Local (no key required)"
                                    } else if configured {
                                        "Keyring: Configured"
                                    } else {
                                        "Keyring: Not set"
                                    }),
                            )
                            .child(top_bar_button(
                                "paste-api-key-btn",
                                "Set from Clipboard",
                                !local_provider,
                                palette,
                                cx.listener(move |this, _, _, cx| {
                                    let prov = this.draft_config.ai.provider.clone();
                                    if let Some(item) = cx.read_from_clipboard()
                                        && let Some(key) = item.text()
                                    {
                                        let trimmed = key.trim();
                                        if !trimmed.is_empty() {
                                            match store_api_key(&prov, trimmed) {
                                                Ok(()) => {
                                                    this.settings_status = Some(format!(
                                                        "Stored {prov} key in OS keychain"
                                                    ));
                                                }
                                                Err(err) => {
                                                    this.settings_status =
                                                        Some(format!("Keyring error: {err}"));
                                                }
                                            }
                                            cx.notify();
                                        }
                                    }
                                }),
                            ))
                    }))
                    // Footer actions
                    .child(
                        div()
                            .pt(px(6.0))
                            .border_t_1()
                            .border_color(to_hsla(palette.border, 1.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(to_hsla(palette.muted, 1.0))
                                    .child(self.settings_status.clone().unwrap_or_else(|| {
                                        "Saves to ~/.config/ace-term/config.toml".to_string()
                                    })),
                            )
                            .child(
                                div()
                                    .id("save-settings-btn")
                                    .px(px(14.0))
                                    .py(px(6.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .bg(to_hsla(palette.accent, 1.0))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(to_hsla(palette.bg, 1.0))
                                    .child("Save & Apply")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_draft_settings(cx);
                                    })),
                            ),
                    ),
            )
    }
}

impl Focusable for TerminalCanvasView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalCanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.exited.load(Ordering::Acquire) {
            window.remove_window();
        }
        let cfg: Arc<AceConfig> = self.config_store.get();
        let palette = ThemePalette::from_name(&cfg.ui.theme);
        let padding = cfg.ui.padding as f32;
        let font_size = px(cfg.font.size);
        let line_height_px = (cfg.font.size * cfg.font.line_height).ceil().max(1.0);

        let vt = Arc::clone(&self.vt);
        let cell_metrics = Arc::clone(&self.cell_metrics);
        let cached_font = Arc::clone(&self.cached_font);
        let pty_writer = Arc::clone(&self.pty_writer);
        let rt = Arc::clone(&self.rt);
        let cfg_clone = Arc::clone(&cfg);
        let cfg_paint = Arc::clone(&cfg);

        // Sync window title if changed
        {
            let mut guard = self.vt.lock().expect("vt lock poisoned");
            for ev in guard.drain_events() {
                if let VtEvent::Title(title) = ev
                    && self.last_title.as_deref() != Some(&title)
                {
                    window.set_window_title(&title);
                    self.last_title = Some(title);
                }
            }
        }

        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .bg(to_hsla(palette.bg, cfg.ui.opacity))
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(self.render_top_bar(palette, cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle, cx);
                            this.context_menu = None;
                            let padding = this.config_store.get().ui.padding as f32;
                            let (col, row) = this.pixel_to_cell(event.position, padding);
                            let mut guard = this.vt.lock().expect("vt lock poisoned");
                            guard.start_selection(col, row, SelectionType::Simple);
                            this.selecting = true;
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle, cx);
                            let has_selection = this
                                .vt
                                .lock()
                                .expect("vt lock poisoned")
                                .selection_text()
                                .is_some_and(|s| !s.is_empty());
                            let win_size = window.viewport_size();
                            let max_x = (f32::from(win_size.width) - 190.0).max(8.0);
                            let max_y = (f32::from(win_size.height) - 160.0).max(TOP_BAR_HEIGHT);
                            let x = f32::from(event.position.x).clamp(8.0, max_x);
                            let y = f32::from(event.position.y).clamp(TOP_BAR_HEIGHT, max_y);
                            this.context_menu = Some(ContextMenuState {
                                x,
                                y,
                                has_selection,
                            });
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        if !this.selecting {
                            return;
                        }
                        let padding = this.config_store.get().ui.padding as f32;
                        let (col, row) = this.pixel_to_cell(event.position, padding);
                        this.vt
                            .lock()
                            .expect("vt lock poisoned")
                            .update_selection(col, row);
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                            if this.selecting {
                                let padding = this.config_store.get().ui.padding as f32;
                                let (col, row) = this.pixel_to_cell(event.position, padding);
                                this.vt
                                    .lock()
                                    .expect("vt lock poisoned")
                                    .finish_selection(col, row);
                                this.selecting = false;
                                cx.notify();
                            }
                        }),
                    )
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
                        let delta_y: f32 = match event.delta {
                            ScrollDelta::Lines(p) => p.y,
                            ScrollDelta::Pixels(p) => {
                                let y: f32 = p.y.into();
                                y / 20.0
                            }
                        };
                        let lines = delta_y.round() as i32;
                        if lines != 0 {
                            this.vt
                                .lock()
                                .expect("vt lock poisoned")
                                .scroll_display(lines);
                            cx.notify();
                        }
                    }))
                    .child(
                        canvas(
                            move |bounds, window, _cx| {
                                let resolved_family = resolve_monospace_family(
                                    window,
                                    &cached_font,
                                    &cfg_clone.font.family,
                                    &cfg_clone.font.fallback_families,
                                );
                                let font = Font {
                                    family: resolved_family,
                                    ..Font::default()
                                };
                                let font_id = window.text_system().resolve_font(&font);
                                let cw: f32 = f32::from(
                                    window.text_system().layout_width(font_id, font_size, '0'),
                                )
                                .max(6.0);
                                *cell_metrics.lock().expect("cell_metrics lock poisoned") =
                                    (cw, line_height_px);

                                let avail_w: f32 =
                                    (f32::from(bounds.size.width) - padding * 2.0).max(cw);
                                let avail_h: f32 = (f32::from(bounds.size.height) - padding * 2.0)
                                    .max(line_height_px);
                                let cols = (avail_w / cw).floor().max(1.0) as usize;
                                let rows = (avail_h / line_height_px).floor().max(1.0) as usize;

                                let snapshot: GridSnapshot = {
                                    let mut guard = vt.lock().expect("vt lock poisoned");
                                    let target = TerminalSize::new(cols, rows);
                                    if guard.size() != target {
                                        guard.resize(target);
                                        if let Some(writer) = pty_writer
                                            .lock()
                                            .expect("writer_slot lock poisoned")
                                            .clone()
                                        {
                                            rt.spawn(async move {
                                                let _ =
                                                    writer.resize(cols as u16, rows as u16).await;
                                            });
                                        }
                                    }
                                    guard.snapshot()
                                };

                                (snapshot, font, cw)
                            },
                            move |bounds, (snapshot, font, cw), window, cx| {
                                let origin_x: f32 = f32::from(bounds.origin.x) + padding;
                                let origin_y: f32 = f32::from(bounds.origin.y) + padding;

                                // Paint cell backgrounds (when different from canvas bg or selected)
                                for cell in &snapshot.cells {
                                    if cell.selected || cell.bg != RgbColor::new(0x11, 0x11, 0x1b) {
                                        let bg_color = if cell.selected {
                                            palette.accent
                                        } else if cell.bg == RgbColor::new(0x11, 0x11, 0x1b) {
                                            palette.bg
                                        } else {
                                            cell.bg
                                        };
                                        let cell_bounds = Bounds {
                                            origin: point(
                                                px(origin_x + cell.col as f32 * cw),
                                                px(origin_y + cell.row as f32 * line_height_px),
                                            ),
                                            size: size(px(cw.ceil()), px(line_height_px)),
                                        };
                                        window.paint_quad(fill(
                                            cell_bounds,
                                            to_hsla(
                                                bg_color,
                                                if cell.selected { 0.35 } else { 1.0 },
                                            ),
                                        ));
                                    }
                                }

                                // Paint characters
                                let mut char_buf = [0u8; 4];
                                for cell in &snapshot.cells {
                                    if cell.c == ' ' || cell.c == '\0' {
                                        continue;
                                    }
                                    let text: SharedString =
                                        cell.c.encode_utf8(&mut char_buf).to_owned().into();
                                    let mut run_font = font.clone();
                                    if cell.bold {
                                        run_font.weight = FontWeight::BOLD;
                                    }
                                    if cell.italic {
                                        run_font.style = FontStyle::Italic;
                                    }
                                    let fg_color = if cell.fg == RgbColor::new(0xcd, 0xd6, 0xf4)
                                        && !cell.selected
                                    {
                                        palette.fg
                                    } else {
                                        cell.fg
                                    };
                                    let run = TextRun {
                                        len: text.len(),
                                        font: run_font,
                                        color: to_hsla(fg_color, if cell.dim { 0.6 } else { 1.0 }),
                                        background_color: None,
                                        underline: cell.underline.then(|| UnderlineStyle {
                                            thickness: px(1.0),
                                            color: Some(to_hsla(fg_color, 1.0)),
                                            wavy: false,
                                        }),
                                        strikethrough: None,
                                    };
                                    let shaped = window.text_system().shape_line(
                                        text,
                                        font_size,
                                        &[run],
                                        None,
                                    );
                                    let pos = point(
                                        px(origin_x + cell.col as f32 * cw),
                                        px(origin_y + cell.row as f32 * line_height_px),
                                    );
                                    let _ = shaped.paint(
                                        pos,
                                        px(line_height_px),
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                }

                                // Paint cursor
                                if snapshot.cursor.visible {
                                    let cx_px = origin_x + snapshot.cursor.col as f32 * cw;
                                    let cy_px =
                                        origin_y + snapshot.cursor.row as f32 * line_height_px;
                                    let cursor_bounds = match cfg_paint.terminal.cursor_style {
                                        CursorStyle::Block => Bounds {
                                            origin: point(px(cx_px), px(cy_px)),
                                            size: size(px(cw), px(line_height_px)),
                                        },
                                        CursorStyle::Bar => Bounds {
                                            origin: point(px(cx_px), px(cy_px)),
                                            size: size(px(2.0), px(line_height_px)),
                                        },
                                        CursorStyle::Underline => Bounds {
                                            origin: point(
                                                px(cx_px),
                                                px(cy_px + line_height_px - 2.0),
                                            ),
                                            size: size(px(cw), px(2.0)),
                                        },
                                    };
                                    window.paint_quad(fill(
                                        cursor_bounds,
                                        to_hsla(palette.cursor, 0.75),
                                    ));
                                }
                            },
                        )
                        .size_full(),
                    ),
            );

        if let Some(block) = &self.ai_block {
            let colors = AiBlockColors {
                bg: palette.bg,
                surface: palette.surface,
                border: palette.border,
                fg: palette.fg,
                muted: palette.muted,
                accent: palette.accent,
            };
            root = root.child(render_ai_block(
                block,
                colors,
                cx.listener(|this, _, _, cx| {
                    if let Some(mut b) = this.ai_block.take() {
                        b.resolve_approval(false);
                    }
                    this.agent_rx = None;
                    cx.notify();
                }),
                cx.listener(|this, _, _, cx| {
                    if let Some(b) = this.ai_block.as_mut() {
                        b.resolve_approval(true);
                    }
                    cx.notify();
                }),
                cx.listener(|this, _, _, cx| {
                    if let Some(b) = this.ai_block.as_mut() {
                        b.resolve_approval(false);
                    }
                    cx.notify();
                }),
            ));
        }

        root = root.child(self.render_status_bar(palette, cx));

        if let Some(menu) = &self.context_menu {
            root = root.child(self.render_context_menu(menu, palette, cx));
        }

        if self.settings_open {
            root = root.child(self.render_settings_modal(palette, cx));
        }

        root
    }
}

impl TerminalCanvasView {
    fn render_status_bar(&self, palette: ThemePalette, cx: &mut Context<Self>) -> impl IntoElement {
        let env = self
            .env_snapshot
            .lock()
            .expect("env_snapshot lock poisoned")
            .clone();
        let failed_code = *self
            .last_failed_exit
            .lock()
            .expect("last_failed_exit lock poisoned");
        let cfg = self.config_store.get();

        let mut left = div().flex().items_center().gap(px(10.0));
        if let Some(git) = &env.git {
            let label = if git.dirty {
                format!("git:{}*", git.branch)
            } else {
                format!("git:{}", git.branch)
            };
            left = left.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(to_hsla(palette.accent, 0.14))
                    .text_size(px(11.0))
                    .text_color(to_hsla(palette.accent, 1.0))
                    .child(label),
            );
        }

        for tool in env.toolchains.iter().take(3) {
            let text = match &tool.version {
                Some(v) => format!("{} {}", tool.name, v),
                None => tool.name.to_string(),
            };
            left = left.child(
                div()
                    .text_size(px(11.0))
                    .text_color(to_hsla(palette.muted, 1.0))
                    .child(text),
            );
        }

        if let Some(code) = failed_code {
            left = left.child(
                div()
                    .id("ask-ai-fix-btn")
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .bg(to_hsla(RgbColor::new(0xf3, 0x8b, 0xa8), 0.16))
                    .text_size(px(11.0))
                    .text_color(to_hsla(RgbColor::new(0xf3, 0x8b, 0xa8), 1.0))
                    .child(format!("Exit {code} — Ask AI to fix?"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        *this
                            .last_failed_exit
                            .lock()
                            .expect("last_failed_exit lock poisoned") = None;
                        this.spawn_agent_turn(
                            format!("The previous command exited with code {code}. Diagnose and suggest a fix."),
                            cx,
                        );
                    })),
            );
        }

        let mode_color = match self.input_mode {
            InputMode::Ai => RgbColor::new(0xa6, 0xe3, 0xa1),
            InputMode::Shell | InputMode::Auto => palette.accent,
        };

        div()
            .h(px(STATUS_BAR_HEIGHT))
            .w_full()
            .px(px(12.0))
            .bg(to_hsla(palette.surface, 1.0))
            .border_t_1()
            .border_color(to_hsla(palette.border, 1.0))
            .flex()
            .items_center()
            .justify_between()
            .child(left)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(to_hsla(palette.muted, 1.0))
                            .child(format!("{}:{}", cfg.ai.provider, cfg.ai.model)),
                    )
                    .child(
                        div()
                            .id("mode-toggle-pill")
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .border_1()
                            .border_color(to_hsla(mode_color, 0.7))
                            .bg(to_hsla(mode_color, 0.16))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(to_hsla(mode_color, 1.0))
                            .child(format!("[{}]", self.input_mode.label()))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_mode(cx))),
                    ),
            )
    }
}

fn resolve_monospace_family(
    window: &Window,
    cache: &Arc<Mutex<Option<CachedFont>>>,
    preferred: &str,
    fallbacks: &[String],
) -> SharedString {
    {
        let guard = cache.lock().expect("cached_font lock poisoned");
        if let Some(cached) = guard.as_ref()
            && cached.requested_family == preferred
        {
            return cached.resolved_family.clone();
        }
    }

    let all_fonts = window.text_system().all_font_names();
    let compact = preferred.replace(' ', "");
    let candidates = [
        preferred.to_string(),
        format!("{compact} Nerd Font Mono"),
        format!("{compact} Nerd Font"),
        format!("{preferred} Mono"),
    ]
    .into_iter()
    .chain(fallbacks.iter().cloned())
    .chain([
        "JetBrainsMono Nerd Font Mono".to_string(),
        "Adwaita Mono".to_string(),
        "DejaVu Sans Mono".to_string(),
        "Liberation Mono".to_string(),
        "Noto Sans Mono".to_string(),
        "Fira Code".to_string(),
        "Cascadia Mono".to_string(),
        "Menlo".to_string(),
        "Consolas".to_string(),
        "monospace".to_string(),
    ]);

    let resolved = candidates
        .into_iter()
        .find(|cand| {
            all_fonts
                .iter()
                .any(|installed| installed.eq_ignore_ascii_case(cand))
        })
        .unwrap_or_else(|| "monospace".to_string());

    let mut available_mono_presets: Vec<String> = [
        "JetBrainsMono Nerd Font Mono",
        "JetBrains Mono",
        "Adwaita Mono",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
        "Fira Code",
        "Cascadia Mono",
        "Menlo",
        "Consolas",
    ]
    .into_iter()
    .filter(|cand| {
        all_fonts
            .iter()
            .any(|installed| installed.eq_ignore_ascii_case(cand))
    })
    .map(str::to_string)
    .collect();
    if available_mono_presets.is_empty() {
        available_mono_presets.push("monospace".to_string());
    }

    let shared: SharedString = resolved.into();
    *cache.lock().expect("cached_font lock poisoned") = Some(CachedFont {
        requested_family: preferred.to_string(),
        resolved_family: shared.clone(),
        available_mono_presets,
    });
    shared
}

fn top_bar_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut el = div()
        .id(id)
        .px(px(9.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .text_size(px(12.0))
        .border_1()
        .border_color(to_hsla(palette.border, 0.8))
        .bg(to_hsla(
            palette.surface_hover,
            if enabled { 0.7 } else { 0.25 },
        ))
        .text_color(to_hsla(
            if enabled { palette.fg } else { palette.muted },
            1.0,
        ))
        .child(label);

    if enabled {
        el = el.cursor_pointer().on_click(on_click);
    }
    el
}

fn context_menu_item(
    id: &'static str,
    label: &'static str,
    shortcut: &'static str,
    enabled: bool,
    palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut el = div()
        .id(id)
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_between()
        .text_size(px(12.0))
        .text_color(to_hsla(
            if enabled { palette.fg } else { palette.muted },
            1.0,
        ))
        .child(label)
        .child(
            div()
                .text_size(px(11.0))
                .text_color(to_hsla(palette.muted, 1.0))
                .child(shortcut),
        );

    if enabled {
        el = el
            .cursor_pointer()
            .hover(move |s| s.bg(to_hsla(palette.surface_hover, 1.0)))
            .on_click(on_click);
    }
    el
}

fn setting_row(
    label: &'static str,
    palette: ThemePalette,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .w(px(110.0))
                .text_size(px(12.0))
                .text_color(to_hsla(palette.muted, 1.0))
                .child(label),
        )
        .child(div().flex_1().flex().justify_end().child(control))
}

fn pill_button<T: Fn(&ClickEvent, &mut Window, &mut App) + 'static>(
    id: String,
    label: &str,
    active: bool,
    palette: ThemePalette,
    on_click: T,
) -> impl IntoElement + use<T> {
    div()
        .id(SharedString::from(id))
        .px(px(9.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .border_1()
        .border_color(to_hsla(
            if active {
                palette.accent
            } else {
                palette.border
            },
            1.0,
        ))
        .bg(to_hsla(
            if active {
                palette.accent
            } else {
                palette.surface_hover
            },
            if active { 0.2 } else { 0.6 },
        ))
        .text_size(px(12.0))
        .text_color(to_hsla(
            if active { palette.accent } else { palette.fg },
            1.0,
        ))
        .child(label.to_string())
        .on_click(on_click)
}

fn stepper(
    id_prefix: &'static str,
    value_label: String,
    palette: ThemePalette,
    on_dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .id(SharedString::from(format!("{id_prefix}-dec")))
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .border_1()
                .border_color(to_hsla(palette.border, 1.0))
                .bg(to_hsla(palette.surface_hover, 0.7))
                .text_size(px(12.0))
                .text_color(to_hsla(palette.fg, 1.0))
                .child("-")
                .on_click(on_dec),
        )
        .child(
            div()
                .min_w(px(54.0))
                .text_center()
                .text_size(px(12.0))
                .text_color(to_hsla(palette.fg, 1.0))
                .child(value_label),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id_prefix}-inc")))
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .border_1()
                .border_color(to_hsla(palette.border, 1.0))
                .bg(to_hsla(palette.surface_hover, 0.7))
                .text_size(px(12.0))
                .text_color(to_hsla(palette.fg, 1.0))
                .child("+")
                .on_click(on_inc),
        )
}

fn encode_keystroke(ks: &Keystroke) -> Option<Vec<u8>> {
    if ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.platform {
        if ks.key.len() == 1 {
            let b = ks.key.as_bytes()[0].to_ascii_lowercase();
            if b.is_ascii_lowercase() {
                return Some(vec![b - b'a' + 1]);
            }
        }
        if ks.key == "space" {
            return Some(vec![0x00]);
        }
    }

    let seq: &[u8] = match ks.key.as_str() {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        "insert" => b"\x1b[2~",
        "space" => b" ",
        _ => {
            if let Some(ch) = &ks.key_char {
                return Some(ch.as_bytes().to_vec());
            }
            if ks.key.len() == 1 && !ks.modifiers.control && !ks.modifiers.platform {
                return Some(ks.key.as_bytes().to_vec());
            }
            return None;
        }
    };

    Some(seq.to_vec())
}

fn update_line_buffer(line_buffer: &mut String, ks: &Keystroke, bytes: &[u8]) {
    if bytes == [0x03] || bytes == [0x15] {
        line_buffer.clear();
        return;
    }
    match ks.key.as_str() {
        "backspace" => {
            line_buffer.pop();
        }
        "enter" => {
            line_buffer.clear();
        }
        _ if !ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.platform => {
            if let Ok(text) = std::str::from_utf8(bytes)
                && !text.starts_with('\x1b')
            {
                line_buffer.push_str(text);
            }
        }
        _ => {}
    }
}

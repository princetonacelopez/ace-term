//! `TerminalCanvasView`: GPUI canvas rendering `ace-vt` cells, cursor, selection, and input.

use std::sync::{Arc, Mutex};

use ace_config::{AceConfig, ConfigStore, CursorStyle};
use ace_pty::{PtyWriter, spawn_shell};
use ace_vt::{GridSnapshot, RgbColor, SelectionType, TerminalGrid, TerminalSize, VtEvent, VtTerminal};
use gpui_kit::*;
use tokio::sync::watch;
use tracing::{error, info};

/// Theme palette derived from `AceConfig::ui.theme`.
#[derive(Debug, Clone, Copy)]
struct ThemePalette {
    bg: RgbColor,
    fg: RgbColor,
    cursor: RgbColor,
}

impl ThemePalette {
    fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "dracula" => Self {
                bg: RgbColor::new(0x28, 0x2a, 0x36),
                fg: RgbColor::new(0xf8, 0xf8, 0xf2),
                cursor: RgbColor::new(0xff, 0x79, 0xc6),
            },
            "nord" => Self {
                bg: RgbColor::new(0x2e, 0x34, 0x40),
                fg: RgbColor::new(0xd8, 0xde, 0xe9),
                cursor: RgbColor::new(0x88, 0xc0, 0xd0),
            },
            "default light" | "light" => Self {
                bg: RgbColor::new(0xfa, 0xfa, 0xfa),
                fg: RgbColor::new(0x1e, 0x1e, 0x2e),
                cursor: RgbColor::new(0x1e, 0x66, 0xf5),
            },
            _ => Self {
                bg: RgbColor::new(0x11, 0x11, 0x1b),
                fg: RgbColor::new(0xcd, 0xd6, 0xf4),
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

/// Interactive GPU-accelerated terminal view.
pub struct TerminalCanvasView {
    focus_handle: FocusHandle,
    vt: Arc<Mutex<VtTerminal>>,
    pty_writer: Arc<Mutex<Option<PtyWriter>>>,
    rt: Arc<tokio::runtime::Runtime>,
    config_store: Arc<ConfigStore>,
    cell_metrics: Arc<Mutex<(f32, f32)>>,
    selecting: bool,
    last_title: Option<String>,
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

        let shell_cfg = cfg.terminal.shell.clone();
        let vt_for_pty = Arc::clone(&vt);
        let writer_slot = Arc::clone(&pty_writer);
        let wake_for_pty = wake_tx.clone();
        let rt_for_spawn = Arc::clone(&rt);

        rt_for_spawn.spawn(async move {
            let cwd = std::env::current_dir().ok();
            let spawned = match spawn_shell(
                Some(shell_cfg.as_str()),
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
                        break;
                    }
                    Ok(n) => {
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

        Self {
            focus_handle: cx.focus_handle(),
            vt,
            pty_writer,
            rt,
            config_store,
            cell_metrics: Arc::new(Mutex::new((8.4, 19.0))),
            selecting: false,
            last_title: None,
        }
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

    fn pixel_to_cell(&self, pos: Point<Pixels>, padding: f32) -> (usize, usize) {
        let (cw, lh) = *self.cell_metrics.lock().expect("cell_metrics lock poisoned");
        let x: f32 = pos.x.into();
        let y: f32 = pos.y.into();
        let col = ((x - padding).max(0.0) / cw.max(1.0)).floor() as usize;
        let row = ((y - padding).max(0.0) / lh.max(1.0)).floor() as usize;
        (col, row)
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let mods = &ks.modifiers;

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
            if let Some(item) = cx.read_from_clipboard()
                && let Some(text) = item.text()
            {
                self.write_to_pty(text.into_bytes());
            }
            return;
        }

        if let Some(bytes) = encode_keystroke(ks) {
            self.vt.lock().expect("vt lock poisoned").clear_selection();
            self.write_to_pty(bytes);
        }
    }
}

impl Focusable for TerminalCanvasView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalCanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cfg: Arc<AceConfig> = self.config_store.get();
        let palette = ThemePalette::from_name(&cfg.ui.theme);
        let padding = cfg.ui.padding as f32;
        let font_size = px(cfg.font.size);
        let line_height_px = (cfg.font.size * cfg.font.line_height).ceil().max(1.0);

        let vt = Arc::clone(&self.vt);
        let cell_metrics = Arc::clone(&self.cell_metrics);
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

        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(to_hsla(palette.bg, cfg.ui.opacity))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    let padding = this.config_store.get().ui.padding as f32;
                    let (col, row) = this.pixel_to_cell(event.position, padding);
                    let mut guard = this.vt.lock().expect("vt lock poisoned");
                    guard.start_selection(col, row, SelectionType::Simple);
                    this.selecting = true;
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
                cx.listener(|this, _event: &MouseUpEvent, _window, _cx| {
                    this.selecting = false;
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
                    this.vt.lock().expect("vt lock poisoned").scroll_display(lines);
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, window, _cx| {
                        let font = Font {
                            family: cfg_clone.font.family.clone().into(),
                            ..Font::default()
                        };
                        let probe_run = TextRun {
                            len: 1,
                            font: font.clone(),
                            color: to_hsla(palette.fg, 1.0),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped_probe = window.text_system().shape_line(
                            "M".into(),
                            font_size,
                            &[probe_run],
                            None,
                        );
                        let cw: f32 = f32::from(shaped_probe.width).max(6.0);
                        *cell_metrics.lock().expect("cell_metrics lock poisoned") =
                            (cw, line_height_px);

                        let avail_w: f32 = f32::from(bounds.size.width) - padding * 2.0;
                        let avail_h: f32 = f32::from(bounds.size.height) - padding * 2.0;
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
                                        let _ = writer.resize(cols as u16, rows as u16).await;
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
                            if cell.selected || cell.bg != palette.bg {
                                let cell_bounds = Bounds {
                                    origin: point(
                                        px(origin_x + cell.col as f32 * cw),
                                        px(origin_y + cell.row as f32 * line_height_px),
                                    ),
                                    size: size(px(cw.ceil()), px(line_height_px)),
                                };
                                window.paint_quad(fill(cell_bounds, to_hsla(cell.bg, 1.0)));
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
                            let run = TextRun {
                                len: text.len(),
                                font: run_font,
                                color: to_hsla(cell.fg, if cell.dim { 0.6 } else { 1.0 }),
                                background_color: None,
                                underline: cell.underline.then(|| UnderlineStyle {
                                    thickness: px(1.0),
                                    color: Some(to_hsla(cell.fg, 1.0)),
                                    wavy: false,
                                }),
                                strikethrough: None,
                            };
                            let shaped =
                                window
                                    .text_system()
                                    .shape_line(text, font_size, &[run], None);
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
                            let cy_px = origin_y + snapshot.cursor.row as f32 * line_height_px;
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
                                    origin: point(px(cx_px), px(cy_px + line_height_px - 2.0)),
                                    size: size(px(cw), px(2.0)),
                                },
                            };
                            window.paint_quad(fill(cursor_bounds, to_hsla(palette.cursor, 0.75)));
                        }
                    },
                )
                .size_full(),
            )
    }
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

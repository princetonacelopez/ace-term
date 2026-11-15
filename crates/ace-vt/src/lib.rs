//! Virtual terminal grid, scrollback, selection, and DEC Mode 2026 synchronization.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AlacrittyEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::Selection;
pub use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

const DEC_2026_ENABLE: &[u8] = b"\x1b[?2026h";
const DEC_2026_DISABLE: &[u8] = b"\x1b[?2026l";
const DEC_2026_SAFETY_TIMEOUT: Duration = Duration::from_millis(1000);

/// Dimensions of the terminal grid in columns and visible rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: usize,
    pub rows: usize,
}

impl TerminalSize {
    /// Creates a new `TerminalSize`, clamping both dimensions to at least `1`.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Terminal events emitted by the VT parser for the host UI or PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtEvent {
    /// Window title changed via OSC 0/2.
    Title(String),
    /// Terminal requested a reply be written back to the PTY (e.g. DSR/DA).
    PtyWrite(String),
    /// Terminal bell triggered.
    Bell,
    /// Child process requested exit.
    Exit,
}

/// RGB color representation for rendered cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Snapshot of a single visible grid cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellSnapshot {
    pub col: usize,
    pub row: usize,
    pub c: char,
    pub fg: RgbColor,
    pub bg: RgbColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub selected: bool,
}

/// Snapshot of the terminal cursor in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSnapshot {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
}

/// Immutable viewport snapshot consumed by the UI renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<CellSnapshot>,
    pub cursor: CursorSnapshot,
    pub alt_screen: bool,
    pub display_offset: usize,
}

/// Abstraction over the underlying virtual terminal grid.
pub trait TerminalGrid {
    /// Feeds raw PTY output bytes into the VT parser.
    fn feed_bytes(&mut self, bytes: &[u8]);
    /// Resizes the terminal grid.
    fn resize(&mut self, size: TerminalSize);
    /// Returns the current grid dimensions.
    fn size(&self) -> TerminalSize;
    /// Returns whether the terminal is currently in the alternate screen buffer.
    fn is_alt_screen(&self) -> bool;
    /// Returns whether DEC Mode 2026 synchronized output is currently active.
    fn is_sync_active(&self) -> bool;
    /// Flushes synchronized output if the 1000ms safety timer has expired.
    fn flush_sync_if_expired(&mut self) -> bool;
    /// Returns whether new visual updates are ready to be rendered, clearing the dirty flag.
    fn take_dirty(&mut self) -> bool;
    /// Builds a snapshot of the visible viewport for rendering.
    fn snapshot(&self) -> GridSnapshot;
    /// Starts a text selection at viewport `(col, row)`.
    fn start_selection(&mut self, col: usize, row: usize, kind: SelectionType);
    /// Updates the active text selection to viewport `(col, row)`.
    fn update_selection(&mut self, col: usize, row: usize);
    /// Clears any active text selection.
    fn clear_selection(&mut self);
    /// Returns the currently selected text, if any.
    fn selection_text(&self) -> Option<String>;
    /// Scrolls the viewport by `delta` lines (positive = up into history, negative = down).
    fn scroll_display(&mut self, delta: i32);
    /// Drains pending `VtEvent` notifications.
    fn drain_events(&mut self) -> Vec<VtEvent>;
}

#[derive(Clone)]
struct VtEventProxy {
    tx: mpsc::Sender<VtEvent>,
}

impl EventListener for VtEventProxy {
    fn send_event(&self, event: AlacrittyEvent) {
        let mapped = match event {
            AlacrittyEvent::Title(title) => Some(VtEvent::Title(title)),
            AlacrittyEvent::PtyWrite(text) => Some(VtEvent::PtyWrite(text)),
            AlacrittyEvent::Bell => Some(VtEvent::Bell),
            AlacrittyEvent::Exit => Some(VtEvent::Exit),
            _ => None,
        };
        if let Some(ev) = mapped {
            let _ = self.tx.send(ev);
        }
    }
}

/// Tracks DEC Private Mode 2026 (`?2026h` / `?2026l`) synchronized output state.
#[derive(Debug, Default)]
struct Dec2026State {
    active: bool,
    started_at: Option<Instant>,
    pending_dirty: bool,
}

impl Dec2026State {
    fn observe_bytes(&mut self, bytes: &[u8]) {
        if contains_subslice(bytes, DEC_2026_ENABLE) {
            self.active = true;
            self.started_at = Some(Instant::now());
        }
        if contains_subslice(bytes, DEC_2026_DISABLE) {
            self.active = false;
            self.started_at = None;
        }
    }

    fn flush_if_expired(&mut self) -> bool {
        if self.active
            && let Some(started) = self.started_at
            && started.elapsed() >= DEC_2026_SAFETY_TIMEOUT
        {
            self.active = false;
            self.started_at = None;
            return true;
        }
        false
    }
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// `alacritty_terminal`-backed virtual terminal state.
pub struct VtTerminal {
    term: Term<VtEventProxy>,
    processor: Processor,
    size: TerminalSize,
    event_rx: mpsc::Receiver<VtEvent>,
    sync: Dec2026State,
    dirty: bool,
}

impl VtTerminal {
    /// Creates a new `VtTerminal` with the given dimensions and scrollback capacity.
    pub fn new(size: TerminalSize, scrollback_lines: usize) -> Self {
        let (tx, event_rx) = mpsc::channel();
        let proxy = VtEventProxy { tx };
        let config = TermConfig {
            scrolling_history: scrollback_lines,
            ..TermConfig::default()
        };
        let term = Term::new(config, &size, proxy);
        Self {
            term,
            processor: Processor::new(),
            size,
            event_rx,
            sync: Dec2026State::default(),
            dirty: true,
        }
    }

    fn viewport_point(&self, col: usize, row: usize) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        let line = Line(row as i32 - offset);
        let col = Column(col.min(self.size.cols.saturating_sub(1)));
        Point::new(line, col)
    }
}

impl TerminalGrid for VtTerminal {
    fn feed_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.sync.observe_bytes(bytes);
        self.processor.advance(&mut self.term, bytes);
        if self.sync.active {
            self.sync.pending_dirty = true;
        } else {
            self.dirty = true;
            self.sync.pending_dirty = false;
        }
    }

    fn resize(&mut self, size: TerminalSize) {
        if self.size == size {
            return;
        }
        self.size = size;
        self.term.resize(size);
        self.dirty = true;
    }

    fn size(&self) -> TerminalSize {
        self.size
    }

    fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn is_sync_active(&self) -> bool {
        self.sync.active
    }

    fn flush_sync_if_expired(&mut self) -> bool {
        if self.sync.flush_if_expired() && self.sync.pending_dirty {
            self.sync.pending_dirty = false;
            self.dirty = true;
            return true;
        }
        false
    }

    fn take_dirty(&mut self) -> bool {
        if self.sync.active {
            return false;
        }
        std::mem::take(&mut self.dirty)
    }

    fn snapshot(&self) -> GridSnapshot {
        let content = self.term.renderable_content();
        let display_offset = content.display_offset;
        let selection = content.selection;
        let cursor_point = content.cursor.point;
        let cursor_visible = self.term.mode().contains(TermMode::SHOW_CURSOR)
            && content.cursor.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden;

        let mut cells = Vec::with_capacity(self.size.cols * self.size.rows);
        for indexed in content.display_iter {
            let line_idx = indexed.point.line.0 + display_offset as i32;
            if line_idx < 0 || line_idx >= self.size.rows as i32 {
                continue;
            }
            let row = line_idx as usize;
            let col = indexed.point.column.0;
            let cell = indexed.cell;
            let selected = selection
                .is_some_and(|sel| sel.contains_cell(&indexed, cursor_point, content.cursor.shape));

            let (mut fg, mut bg) = (resolve_color(cell.fg), resolve_color(cell.bg));
            if cell.flags.contains(CellFlags::INVERSE) || selected {
                std::mem::swap(&mut fg, &mut bg);
            }

            cells.push(CellSnapshot {
                col,
                row,
                c: cell.c,
                fg,
                bg,
                bold: cell.flags.contains(CellFlags::BOLD),
                italic: cell.flags.contains(CellFlags::ITALIC),
                underline: cell.flags.intersects(
                    CellFlags::UNDERLINE
                        | CellFlags::DOUBLE_UNDERLINE
                        | CellFlags::UNDERCURL
                        | CellFlags::DOTTED_UNDERLINE
                        | CellFlags::DASHED_UNDERLINE,
                ),
                dim: cell.flags.contains(CellFlags::DIM),
                selected,
            });
        }

        let cursor_row_i32 = cursor_point.line.0 + display_offset as i32;
        let cursor_in_view = cursor_row_i32 >= 0 && cursor_row_i32 < self.size.rows as i32;

        GridSnapshot {
            cols: self.size.cols,
            rows: self.size.rows,
            cells,
            cursor: CursorSnapshot {
                col: cursor_point.column.0,
                row: cursor_row_i32.max(0) as usize,
                visible: cursor_visible && cursor_in_view,
            },
            alt_screen: self.is_alt_screen(),
            display_offset,
        }
    }

    fn start_selection(&mut self, col: usize, row: usize, kind: SelectionType) {
        let point = self.viewport_point(col, row);
        self.term.selection = Some(Selection::new(kind, point, Side::Left));
        self.dirty = true;
    }

    fn update_selection(&mut self, col: usize, row: usize) {
        let point = self.viewport_point(col, row);
        if let Some(sel) = self.term.selection.as_mut() {
            sel.update(point, Side::Right);
            self.dirty = true;
        }
    }

    fn clear_selection(&mut self) {
        if self.term.selection.take().is_some() {
            self.dirty = true;
        }
    }

    fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    fn scroll_display(&mut self, delta: i32) {
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(delta));
            self.dirty = true;
        }
    }

    fn drain_events(&mut self) -> Vec<VtEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            events.push(ev);
        }
        events
    }
}

fn resolve_color(color: Color) -> RgbColor {
    match color {
        Color::Spec(Rgb { r, g, b }) => RgbColor::new(r, g, b),
        Color::Named(named) => named_color_rgb(named),
        Color::Indexed(idx) => indexed_color_rgb(idx),
    }
}

fn named_color_rgb(named: NamedColor) -> RgbColor {
    match named {
        NamedColor::Black => RgbColor::new(0x1e, 0x1e, 0x2e),
        NamedColor::Red => RgbColor::new(0xf3, 0x8b, 0xa8),
        NamedColor::Green => RgbColor::new(0xa6, 0xe3, 0xa1),
        NamedColor::Yellow => RgbColor::new(0xf9, 0xe2, 0xaf),
        NamedColor::Blue => RgbColor::new(0x89, 0xb4, 0xfa),
        NamedColor::Magenta => RgbColor::new(0xcb, 0xa6, 0xf7),
        NamedColor::Cyan => RgbColor::new(0x94, 0xe2, 0xd5),
        NamedColor::White => RgbColor::new(0xcd, 0xd6, 0xf4),
        NamedColor::BrightBlack => RgbColor::new(0x58, 0x5b, 0x70),
        NamedColor::BrightRed => RgbColor::new(0xf3, 0x8b, 0xa8),
        NamedColor::BrightGreen => RgbColor::new(0xa6, 0xe3, 0xa1),
        NamedColor::BrightYellow => RgbColor::new(0xf9, 0xe2, 0xaf),
        NamedColor::BrightBlue => RgbColor::new(0x89, 0xb4, 0xfa),
        NamedColor::BrightMagenta => RgbColor::new(0xcb, 0xa6, 0xf7),
        NamedColor::BrightCyan => RgbColor::new(0x94, 0xe2, 0xd5),
        NamedColor::BrightWhite => RgbColor::new(0xff, 0xff, 0xff),
        NamedColor::Foreground | NamedColor::BrightForeground => RgbColor::new(0xcd, 0xd6, 0xf4),
        NamedColor::Background => RgbColor::new(0x11, 0x11, 0x1b),
        NamedColor::Cursor => RgbColor::new(0xf5, 0xe0, 0xdc),
        NamedColor::DimBlack => RgbColor::new(0x18, 0x18, 0x25),
        NamedColor::DimRed => RgbColor::new(0xc4, 0x6c, 0x85),
        NamedColor::DimGreen => RgbColor::new(0x84, 0xb8, 0x80),
        NamedColor::DimYellow => RgbColor::new(0xc7, 0xb4, 0x8b),
        NamedColor::DimBlue => RgbColor::new(0x6d, 0x90, 0xc8),
        NamedColor::DimMagenta => RgbColor::new(0xa2, 0x85, 0xc6),
        NamedColor::DimCyan => RgbColor::new(0x76, 0xb5, 0xaa),
        NamedColor::DimWhite | NamedColor::DimForeground => RgbColor::new(0xa6, 0xad, 0xc8),
    }
}

fn indexed_color_rgb(idx: u8) -> RgbColor {
    match idx {
        0 => named_color_rgb(NamedColor::Black),
        1 => named_color_rgb(NamedColor::Red),
        2 => named_color_rgb(NamedColor::Green),
        3 => named_color_rgb(NamedColor::Yellow),
        4 => named_color_rgb(NamedColor::Blue),
        5 => named_color_rgb(NamedColor::Magenta),
        6 => named_color_rgb(NamedColor::Cyan),
        7 => named_color_rgb(NamedColor::White),
        8 => named_color_rgb(NamedColor::BrightBlack),
        9 => named_color_rgb(NamedColor::BrightRed),
        10 => named_color_rgb(NamedColor::BrightGreen),
        11 => named_color_rgb(NamedColor::BrightYellow),
        12 => named_color_rgb(NamedColor::BrightBlue),
        13 => named_color_rgb(NamedColor::BrightMagenta),
        14 => named_color_rgb(NamedColor::BrightCyan),
        15 => named_color_rgb(NamedColor::BrightWhite),
        16..=231 => {
            let n = idx - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            RgbColor::new(scale(r), scale(g), scale(b))
        }
        232..=255 => {
            let gray = 8 + (idx - 232) * 10;
            RgbColor::new(gray, gray, gray)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_text_and_snapshots_grid() {
        let mut vt = VtTerminal::new(TerminalSize::new(80, 24), 1000);
        vt.feed_bytes(b"hello ace-term");
        assert!(vt.take_dirty());
        let snap = vt.snapshot();
        let line: String = snap
            .cells
            .iter()
            .filter(|c| c.row == 0 && c.col < 14)
            .map(|c| c.c)
            .collect();
        assert_eq!(line, "hello ace-term");
    }

    #[test]
    fn dec_2026_holds_dirty_until_disable() {
        let mut vt = VtTerminal::new(TerminalSize::new(80, 24), 1000);
        assert!(vt.take_dirty()); // initial dirty

        vt.feed_bytes(b"\x1b[?2026hframe1");
        assert!(vt.is_sync_active());
        assert!(!vt.take_dirty());

        vt.feed_bytes(b"\x1b[?2026l");
        assert!(!vt.is_sync_active());
        assert!(vt.take_dirty());
    }

    #[test]
    fn alt_screen_mode_toggles() {
        let mut vt = VtTerminal::new(TerminalSize::new(80, 24), 1000);
        assert!(!vt.is_alt_screen());
        vt.feed_bytes(b"\x1b[?1049h");
        assert!(vt.is_alt_screen());
        vt.feed_bytes(b"\x1b[?1049l");
        assert!(!vt.is_alt_screen());
    }

    #[test]
    fn resize_updates_dimensions() {
        let mut vt = VtTerminal::new(TerminalSize::new(80, 24), 1000);
        vt.resize(TerminalSize::new(120, 40));
        assert_eq!(vt.size(), TerminalSize::new(120, 40));
    }
}

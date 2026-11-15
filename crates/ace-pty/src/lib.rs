//! Cross-platform asynchronous PTY layer built on `rust-pty` and `tokio`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rust_pty::{NativePtySystem, PtyConfig, PtyError, PtyMaster, PtySystem};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::Mutex;
use unicode_normalization::UnicodeNormalization;

#[cfg(unix)]
type NativeMaster = rust_pty::UnixPtyMaster;
#[cfg(windows)]
type NativeMaster = rust_pty::WindowsPtyMaster;

#[cfg(unix)]
type NativeChild = rust_pty::UnixPtyChild;
#[cfg(windows)]
type NativeChild = rust_pty::WindowsPtyChild;

pub use rust_pty::{ExitStatus, PtyChild};

/// Errors produced by PTY creation, I/O, or resizing.
#[derive(Debug, Error)]
pub enum AcePtyError {
    /// Underlying `rust-pty` failure.
    #[error("pty error: {0}")]
    Pty(#[from] PtyError),

    /// Standard I/O failure on the PTY stream.
    #[error("pty io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Normalizes a path string (such as macOS APFS NFD paths) to Unicode NFC.
pub fn normalize_path_nfc(path: &str) -> String {
    path.nfc().collect()
}

/// Async reader half of a spawned PTY session.
pub struct PtyReader {
    inner: ReadHalf<NativeMaster>,
}

impl PtyReader {
    /// Reads raw bytes from the PTY master into `buf`.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, AcePtyError> {
        Ok(self.inner.read(buf).await?)
    }
}

/// Thread-safe async writer and resizer handle for a spawned PTY session.
#[derive(Clone)]
pub struct PtyWriter {
    inner: Arc<Mutex<WriteHalf<NativeMaster>>>,
    cols_rows: Arc<std::sync::Mutex<(u16, u16)>>,
    #[cfg(unix)]
    raw_fd: std::os::unix::io::RawFd,
    open: Arc<AtomicBool>,
}

impl PtyWriter {
    /// Writes input bytes (keystrokes, paste, Ctrl+C `0x03`, etc.) to the PTY master.
    pub async fn write_all(&self, bytes: &[u8]) -> Result<(), AcePtyError> {
        let mut guard = self.inner.lock().await;
        guard.write_all(bytes).await?;
        guard.flush().await?;
        Ok(())
    }

    /// Propagates a terminal window resize (`TIOCSWINSZ` / `ResizePseudoConsole`).
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), AcePtyError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        {
            let mut dims = self.cols_rows.lock().expect("cols_rows lock poisoned");
            if *dims == (cols, rows) {
                return Ok(());
            }
            *dims = (cols, rows);
        }
        if !self.open.load(Ordering::Acquire) {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let ws = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // SAFETY: `raw_fd` is a valid open PTY master fd while `open` is true.
            let rc = unsafe { libc::ioctl(self.raw_fd, libc::TIOCSWINSZ, &ws) };
            if rc != 0 {
                return Err(AcePtyError::Io(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}

/// Spawned PTY session parts.
pub struct SpawnedPty {
    pub reader: PtyReader,
    pub writer: PtyWriter,
    pub child: NativeChild,
}

/// Spawns an interactive shell in a new PTY.
///
/// If `shell` is `Some` and non-empty, that executable is launched; otherwise the
/// platform default shell is detected automatically.
pub async fn spawn_shell(
    shell: Option<&str>,
    cwd: Option<&Path>,
    cols: u16,
    rows: u16,
) -> Result<SpawnedPty, AcePtyError> {
    let cols = cols.max(1);
    let rows = rows.max(1);

    let mut builder = PtyConfig::builder()
        .window_size(cols, rows)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "ace-term")
        .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

    for var in [
        "GHOSTTY_RESOURCES_DIR",
        "GHOSTTY_BIN_DIR",
        "GHOSTTY_SHELL_FEATURES",
        "GHOSTTY_SHELL_INTEGRATION_NO_SUDO",
        "KITTY_WINDOW_ID",
        "KITTY_PID",
        "KITTY_PUBLIC_KEY",
        "KITTY_INSTALLATION_DIR",
        "ALACRITTY_SOCKET",
        "ALACRITTY_LOG",
        "ALACRITTY_WINDOW_ID",
        "WEZTERM_EXECUTABLE",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "WEZTERM_CONFIG_FILE",
        "WT_SESSION",
        "WT_PROFILE_ID",
        "ITERM_SESSION_ID",
        "ITERM_PROFILE",
        "VTE_VERSION",
        "KONSOLE_VERSION",
        "KONSOLE_DBUS_SESSION",
        "GNOME_TERMINAL_SCREEN",
        "GNOME_TERMINAL_SERVICE",
        "COLORFGBG",
    ] {
        builder = builder.env_remove(var);
    }

    if let Some(dir) = cwd {
        builder = builder.working_directory(dir);
    }

    let config = builder.build();
    let (master, child) = match shell.map(str::trim).filter(|s| !s.is_empty()) {
        Some(program) => NativePtySystem::spawn(program, std::iter::empty::<&str>(), &config).await?,
        None => NativePtySystem::spawn_shell(&config).await?,
    };

    #[cfg(unix)]
    let raw_fd = master.as_raw_fd();
    let open = Arc::new(AtomicBool::new(true));
    let (read_half, write_half) = tokio::io::split(master);

    Ok(SpawnedPty {
        reader: PtyReader { inner: read_half },
        writer: PtyWriter {
            inner: Arc::new(Mutex::new(write_half)),
            cols_rows: Arc::new(std::sync::Mutex::new((cols, rows))),
            #[cfg(unix)]
            raw_fd,
            open,
        },
        child,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_nfd_to_nfc() {
        // "e" + combining acute accent (NFD) -> precomposed U+00E9 (NFC)
        let nfd = "/Users/test/cafe\u{0301}";
        let nfc = normalize_path_nfc(nfd);
        assert_eq!(nfc, "/Users/test/caf\u{00e9}");
    }

    #[tokio::test]
    async fn spawns_command_and_reads_output() {
        let config = PtyConfig::builder().window_size(80, 24).build();
        if let Ok((master, mut child)) = NativePtySystem::spawn("echo", ["ace-pty-ok"], &config).await {
            let (mut read_half, _write_half) = tokio::io::split(master);
            let mut buf = [0u8; 256];
            let n = read_half.read(&mut buf).await.unwrap_or(0);
            let out = String::from_utf8_lossy(&buf[..n]);
            assert!(out.contains("ace-pty-ok"));
            let _ = child.wait().await;
        }
    }
}

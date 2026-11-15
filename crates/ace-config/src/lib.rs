//! Configuration loading, hot-reloading, and secret management for Ace Term.

pub mod secrets;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

/// Errors produced while loading or watching Ace Term configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read a configuration file from disk.
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse TOML configuration.
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// Failed to initialize file system watcher.
    #[error("failed to watch config path: {0}")]
    Watch(#[from] notify::Error),

    /// OS keychain operation failed.
    #[error("keyring error: {0}")]
    Keyring(String),
}

/// Cursor rendering style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

/// Approval policy for AI-initiated file writes and shell commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    #[default]
    Default,
    Auto,
}

/// Terminal emulation settings (`[terminal]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Explicit shell executable path; empty string auto-detects the user's default shell.
    pub shell: String,
    /// Number of scrollback history lines retained per terminal instance.
    pub scrollback_lines: usize,
    /// Cursor shape.
    pub cursor_style: CursorStyle,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: String::new(),
            scrollback_lines: 10_000,
            cursor_style: CursorStyle::Block,
        }
    }
}

/// AI provider and routing settings (`[ai]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub approval_mode: ApprovalMode,
    pub auto_intercept: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            base_url: String::new(),
            approval_mode: ApprovalMode::Default,
            auto_intercept: true,
        }
    }
}

/// User interface settings (`[ui]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    pub opacity: f32,
    pub padding: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "Default Dark".to_string(),
            opacity: 1.0,
            padding: 8,
        }
    }
}

/// Typography settings (`[font]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub fallback_families: Vec<String>,
    pub size: f32,
    pub line_height: f32,
    pub ligatures: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "JetBrains Mono".to_string(),
            fallback_families: vec![
                "Symbols Nerd Font".to_string(),
                "Noto Color Emoji".to_string(),
            ],
            size: 14.0,
            line_height: 1.35,
            ligatures: true,
        }
    }
}

/// Root configuration structure for Ace Term.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AceConfig {
    pub terminal: TerminalConfig,
    pub ai: AiConfig,
    pub ui: UiConfig,
    pub font: FontConfig,
}

impl AceConfig {
    /// Returns the default global configuration file path (`~/.config/ace-term/config.toml`).
    pub fn default_global_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("ace-term").join("config.toml"))
    }

    /// Loads configuration from `global_path` (if it exists) and overlays `workspace_path` (if it exists).
    pub fn load(
        global_path: Option<&Path>,
        workspace_path: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let mut value = toml::Value::try_from(Self::default()).expect("default config serializes");

        if let Some(path) = global_path.filter(|p| p.exists()) {
            let parsed = read_toml_value(path)?;
            merge_toml(&mut value, parsed);
        }

        if let Some(path) = workspace_path.filter(|p| p.exists()) {
            let parsed = read_toml_value(path)?;
            merge_toml(&mut value, parsed);
        }

        value.try_into().map_err(|source| ConfigError::Parse {
            path: workspace_path
                .or(global_path)
                .unwrap_or_else(|| Path::new("config.toml"))
                .to_path_buf(),
            source,
        })
    }

    /// Serializes and writes this configuration to `path`, creating parent directories if needed.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let body = toml::to_string_pretty(self).expect("AceConfig always serializes to TOML");
        std::fs::write(path, body).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

fn read_toml_value(path: &Path) -> Result<toml::Value, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str::<toml::Value>(&contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_map), toml::Value::Table(overlay_map)) => {
            for (k, v) in overlay_map {
                match base_map.get_mut(&k) {
                    Some(existing) => merge_toml(existing, v),
                    None => {
                        base_map.insert(k, v);
                    }
                }
            }
        }
        (base_slot, overlay_val) => {
            *base_slot = overlay_val;
        }
    }
}

/// Lock-free, hot-reloaded configuration store backed by `ArcSwap`.
pub struct ConfigStore {
    current: Arc<ArcSwap<AceConfig>>,
    global_path: Option<PathBuf>,
    workspace_path: Option<PathBuf>,
    _watcher: Option<RecommendedWatcher>,
}

impl ConfigStore {
    /// Loads the initial configuration and starts watching `global_path` and `workspace_path` for edits.
    ///
    /// `on_reload` is invoked whenever a valid configuration reload occurs.
    pub fn load_and_watch<F>(
        global_path: Option<PathBuf>,
        workspace_path: Option<PathBuf>,
        on_reload: F,
    ) -> Result<Self, ConfigError>
    where
        F: Fn(Arc<AceConfig>) + Send + Sync + 'static,
    {
        let initial = AceConfig::load(global_path.as_deref(), workspace_path.as_deref())?;
        let current = Arc::new(ArcSwap::from_pointee(initial));

        let current_clone = Arc::clone(&current);
        let global_clone = global_path.clone();
        let workspace_clone = workspace_path.clone();
        let callback = Arc::new(on_reload);

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else {
                return;
            };
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }
            match AceConfig::load(global_clone.as_deref(), workspace_clone.as_deref()) {
                Ok(next) => {
                    let next_arc = Arc::new(next);
                    current_clone.store(Arc::clone(&next_arc));
                    debug!("reloaded ace-term configuration");
                    callback(next_arc);
                }
                Err(err) => {
                    warn!(error = %err, "ignoring invalid config reload");
                }
            }
        })?;

        for path in [global_path.as_deref(), workspace_path.as_deref()]
            .into_iter()
            .flatten()
        {
            let watch_target = if path.exists() {
                path
            } else if let Some(parent) = path.parent().filter(|p| p.exists()) {
                parent
            } else {
                continue;
            };
            if let Err(err) = watcher.watch(watch_target, RecursiveMode::NonRecursive) {
                warn!(path = %watch_target.display(), error = %err, "failed to watch config path");
            }
        }

        Ok(Self {
            current,
            global_path,
            workspace_path,
            _watcher: Some(watcher),
        })
    }

    /// Returns a lock-free snapshot of the current configuration.
    pub fn get(&self) -> Arc<AceConfig> {
        self.current.load_full()
    }

    /// Manually reloads configuration from disk.
    pub fn reload(&self) -> Result<Arc<AceConfig>, ConfigError> {
        let next = Arc::new(AceConfig::load(
            self.global_path.as_deref(),
            self.workspace_path.as_deref(),
        )?);
        self.current.store(Arc::clone(&next));
        Ok(next)
    }

    /// Saves `config` to the global config path (or `AceConfig::default_global_path()`) and updates the store.
    pub fn save_global(&self, config: AceConfig) -> Result<Arc<AceConfig>, ConfigError> {
        let target = self
            .global_path
            .clone()
            .or_else(AceConfig::default_global_path)
            .ok_or_else(|| ConfigError::Io {
                path: PathBuf::from("config.toml"),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine user config directory",
                ),
            })?;
        config.save(&target)?;
        let next = Arc::new(config);
        self.current.store(Arc::clone(&next));
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn defaults_match_spec() {
        let cfg = AceConfig::default();
        assert_eq!(cfg.terminal.scrollback_lines, 10_000);
        assert_eq!(cfg.terminal.cursor_style, CursorStyle::Block);
        assert_eq!(cfg.ai.provider, "anthropic");
        assert_eq!(cfg.ai.model, "claude-sonnet-4-6");
        assert_eq!(cfg.ai.approval_mode, ApprovalMode::Default);
        assert!(cfg.ai.auto_intercept);
        assert_eq!(cfg.ui.theme, "Default Dark");
        assert!((cfg.ui.opacity - 1.0).abs() < f32::EPSILON);
        assert_eq!(cfg.ui.padding, 8);
        assert_eq!(cfg.font.family, "JetBrains Mono");
        assert!((cfg.font.size - 14.0).abs() < f32::EPSILON);
        assert!((cfg.font.line_height - 1.35).abs() < f32::EPSILON);
        assert!(cfg.font.ligatures);
    }

    #[test]
    fn loads_and_merges_global_and_workspace_configs() {
        let dir = std::env::temp_dir().join(format!("ace-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let global = dir.join("global.toml");
        let workspace = dir.join("workspace.toml");

        let mut gf = std::fs::File::create(&global).unwrap();
        writeln!(
            gf,
            "[font]\nsize = 16.0\n[ui]\ntheme = \"Dracula\"\n[terminal]\nshell = \"/bin/zsh\""
        )
        .unwrap();

        let mut wf = std::fs::File::create(&workspace).unwrap();
        writeln!(wf, "[font]\nsize = 18.0\n[ai]\napproval_mode = \"auto\"").unwrap();

        let cfg = AceConfig::load(Some(&global), Some(&workspace)).unwrap();
        assert_eq!(cfg.terminal.shell, "/bin/zsh");
        assert_eq!(cfg.ui.theme, "Dracula");
        assert!((cfg.font.size - 18.0).abs() < f32::EPSILON);
        assert_eq!(cfg.ai.approval_mode, ApprovalMode::Auto);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

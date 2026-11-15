//! Asynchronous Git (`gix`) and project toolchain environment detection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Git repository summary for the active working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    /// Current branch name (or `"HEAD"` when detached).
    pub branch: String,
    /// Whether the working tree or index has uncommitted changes.
    pub dirty: bool,
}

/// Detected project runtime/toolchain badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainBadge {
    pub name: &'static str,
    pub version: Option<String>,
}

/// Snapshot of detected environment metadata for a directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnvSnapshot {
    pub cwd: PathBuf,
    pub git: Option<GitInfo>,
    pub toolchains: Vec<ToolchainBadge>,
}

impl EnvSnapshot {
    /// Formats a concise summary suitable for LLM system prompt context.
    pub fn prompt_summary(&self) -> String {
        let mut parts = vec![format!("cwd: {}", self.cwd.display())];
        if let Some(git) = &self.git {
            parts.push(format!(
                "git: {}{}",
                git.branch,
                if git.dirty { " (dirty)" } else { "" }
            ));
        }
        if !self.toolchains.is_empty() {
            let tools: Vec<String> = self
                .toolchains
                .iter()
                .map(|t| match &t.version {
                    Some(v) => format!("{} {}", t.name, v),
                    None => t.name.to_string(),
                })
                .collect();
            parts.push(format!("toolchains: {}", tools.join(", ")));
        }
        parts.join(" | ")
    }
}

type VersionCache = Arc<Mutex<HashMap<(PathBuf, Option<SystemTime>), String>>>;

/// Asynchronous environment detector with `(binary_path, mtime)`-keyed version caching.
#[derive(Clone, Default)]
pub struct EnvDetector {
    version_cache: VersionCache,
}

impl EnvDetector {
    /// Creates a new `EnvDetector`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Detects Git status and project toolchains for `cwd` on a blocking threadpool.
    pub async fn detect(&self, cwd: PathBuf) -> EnvSnapshot {
        let cache = Arc::clone(&self.version_cache);
        tokio::task::spawn_blocking(move || detect_sync(&cwd, &cache))
            .await
            .unwrap_or_else(|_| EnvSnapshot::default())
    }
}

fn detect_sync(cwd: &Path, cache: &VersionCache) -> EnvSnapshot {
    let git = detect_git(cwd);
    let mut toolchains = Vec::new();

    if has_manifest_upwards(cwd, "Cargo.toml", 4) {
        toolchains.push(ToolchainBadge {
            name: "Rust",
            version: cached_tool_version("rustc", &["--version"], cache),
        });
    }
    if has_manifest_upwards(cwd, "package.json", 4) {
        toolchains.push(ToolchainBadge {
            name: "Node",
            version: cached_tool_version("node", &["--version"], cache),
        });
    }
    if has_manifest_upwards(cwd, "pyproject.toml", 4) {
        toolchains.push(ToolchainBadge {
            name: "Python",
            version: cached_tool_version("python3", &["--version"], cache),
        });
    }
    if has_manifest_upwards(cwd, "go.mod", 4) {
        toolchains.push(ToolchainBadge {
            name: "Go",
            version: cached_tool_version("go", &["version"], cache),
        });
    }

    EnvSnapshot {
        cwd: cwd.to_path_buf(),
        git,
        toolchains,
    }
}

fn detect_git(cwd: &Path) -> Option<GitInfo> {
    let repo = gix::discover(cwd).ok()?;
    let branch = repo
        .head_name()
        .ok()
        .flatten()
        .map(|name| name.shorten().to_string())
        .unwrap_or_else(|| "HEAD".to_string());
    let dirty = repo.is_dirty().unwrap_or(false);
    Some(GitInfo { branch, dirty })
}

fn has_manifest_upwards(start: &Path, filename: &str, max_depth: usize) -> bool {
    let mut current = Some(start);
    for _ in 0..=max_depth {
        let Some(dir) = current else {
            break;
        };
        if dir.join(filename).is_file() {
            return true;
        }
        current = dir.parent();
    }
    false
}

fn cached_tool_version(binary: &str, args: &[&str], cache: &VersionCache) -> Option<String> {
    let resolved = find_in_path(binary)?;
    let mtime = std::fs::metadata(&resolved)
        .ok()
        .and_then(|m| m.modified().ok());
    let key = (resolved.clone(), mtime);

    if let Some(cached) = cache.lock().expect("version_cache poisoned").get(&key) {
        return Some(cached.clone());
    }

    let output = Command::new(&resolved).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let version = parse_version_token(&raw)?;
    cache
        .lock()
        .expect("version_cache poisoned")
        .insert(key, version.clone());
    Some(version)
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn parse_version_token(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .find(|tok| tok.chars().any(|c| c.is_ascii_digit()) && tok.contains('.'))
        .map(|tok| tok.trim_start_matches("go").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_current_ace_term_workspace() {
        let detector = EnvDetector::new();
        let cwd = std::env::current_dir().unwrap();
        let snap = detector.detect(cwd).await;
        assert!(snap.git.is_some());
        assert!(snap.toolchains.iter().any(|t| t.name == "Rust"));
    }
}

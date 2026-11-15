//! Built-in agent tools (`read_file`, `write_file`, `list_dir`, `search_code`, `run_shell`).

use std::path::{Path, PathBuf};

use ace_config::secrets::scrub_secrets;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::diff::unified_diff;

const MAX_READ_BYTES: usize = 64 * 1024;
const MAX_SHELL_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_SEARCH_MATCHES: usize = 50;

/// Tool action requiring user confirmation when `approval_mode == "default"`.
#[derive(Debug, Clone)]
pub enum PendingApproval {
    WriteFile {
        id: String,
        path: PathBuf,
        display_path: String,
        new_content: String,
        diff: String,
    },
    RunShell {
        id: String,
        command: String,
        cwd: PathBuf,
    },
}

/// JSON tool definition shared across Anthropic and OpenAI-compatible payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// Returns the immutable built-in tool schemas placed in the prompt cache prefix.
pub fn builtin_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file",
            description: "Read the contents of a UTF-8 text file relative to the workspace or absolute path.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to read." }
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "write_file",
            description: "Create or overwrite a file with new content. Shows a unified diff for user approval.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to write." },
                    "content": { "type": "string", "description": "Complete new file content." }
                },
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            name: "list_dir",
            description: "List files and subdirectories in a directory.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path (defaults to current workspace)." }
                }
            }),
        },
        ToolSpec {
            name: "search_code",
            description: "Search for a substring across text files in the workspace.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Text to search for." },
                    "path": { "type": "string", "description": "Optional directory to scope search." }
                },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "run_shell",
            description: "Execute a shell command in the workspace and return its exit code and output.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute." }
                },
                "required": ["command"]
            }),
        },
    ]
}

/// Resolves a tool path against `cwd`.
pub fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Executes a read-only tool (`read_file`, `list_dir`, `search_code`) immediately.
pub fn execute_read_only(name: &str, args: &Value, cwd: &Path) -> Result<String, String> {
    match name {
        "read_file" => {
            let raw = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing 'path'".to_string())?;
            let path = resolve_path(cwd, raw);
            let bytes = std::fs::read(&path).map_err(|e| format!("read_file error: {e}"))?;
            let truncated = bytes.len() > MAX_READ_BYTES;
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_READ_BYTES)]);
            let mut out = scrub_secrets(&text);
            if truncated {
                out.push_str("\n... [truncated at 64 KiB]");
            }
            Ok(out)
        }
        "list_dir" => {
            let raw = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let path = resolve_path(cwd, raw);
            let entries = std::fs::read_dir(&path).map_err(|e| format!("list_dir error: {e}"))?;
            let mut items = Vec::new();
            for entry in entries.flatten().take(200) {
                let name = entry.file_name().to_string_lossy().into_owned();
                let suffix = if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    "/"
                } else {
                    ""
                };
                items.push(format!("{name}{suffix}"));
            }
            items.sort();
            Ok(items.join("\n"))
        }
        "search_code" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing 'query'".to_string())?;
            let raw = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let root = resolve_path(cwd, raw);
            let mut matches = Vec::new();
            search_recursive(&root, &root, query, &mut matches);
            if matches.is_empty() {
                Ok("No matches found.".to_string())
            } else {
                Ok(matches.join("\n"))
            }
        }
        other => Err(format!("unknown read-only tool: {other}")),
    }
}

fn search_recursive(root: &Path, dir: &Path, query: &str, matches: &mut Vec<String>) {
    if matches.len() >= MAX_SEARCH_MATCHES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if matches.len() >= MAX_SEARCH_MATCHES {
            break;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "target" || name_str == "node_modules" {
            continue;
        }
        if path.is_dir() {
            search_recursive(root, &path, query, matches);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            for (idx, line) in text.lines().enumerate() {
                if line.contains(query) {
                    matches.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
                    if matches.len() >= MAX_SEARCH_MATCHES {
                        break;
                    }
                }
            }
        }
    }
}

/// Prepares a `PendingApproval::WriteFile` with a computed unified diff.
pub fn prepare_write_file(id: String, args: &Value, cwd: &Path) -> Result<PendingApproval, String> {
    let raw_path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path'".to_string())?;
    let new_content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'content'".to_string())?
        .to_string();
    let path = resolve_path(cwd, raw_path);
    let old_content = std::fs::read_to_string(&path).unwrap_or_default();
    let diff = unified_diff(raw_path, &old_content, &new_content);
    Ok(PendingApproval::WriteFile {
        id,
        path,
        display_path: raw_path.to_string(),
        new_content,
        diff,
    })
}

/// Applies an approved `WriteFile` or executes an approved `RunShell`.
pub async fn execute_approved(action: &PendingApproval) -> Result<String, String> {
    match action {
        PendingApproval::WriteFile {
            path, new_content, ..
        } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create parent directory: {e}"))?;
            }
            std::fs::write(path, new_content)
                .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
            Ok(format!(
                "Wrote {} ({} bytes)",
                path.display(),
                new_content.len()
            ))
        }
        PendingApproval::RunShell { command, cwd, .. } => {
            #[cfg(unix)]
            let output = Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output()
                .await
                .map_err(|e| format!("failed to spawn shell command: {e}"))?;

            #[cfg(windows)]
            let output = Command::new("pwsh")
                .args(["-NoProfile", "-Command", command])
                .current_dir(cwd)
                .output()
                .await
                .map_err(|e| format!("failed to spawn shell command: {e}"))?;

            let code = output.status.code().unwrap_or(-1);
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            if !output.stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            if combined.len() > MAX_SHELL_OUTPUT_BYTES {
                combined.truncate(MAX_SHELL_OUTPUT_BYTES);
                combined.push_str("\n... [output truncated]");
            }
            Ok(format!("exit_code: {code}\n{}", scrub_secrets(&combined)))
        }
    }
}

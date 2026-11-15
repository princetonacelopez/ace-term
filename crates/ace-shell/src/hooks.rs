//! Non-destructive shell integration hook injection.

use std::path::{Path, PathBuf};

const ACE_ZSH: &str = include_str!("../resources/shell/ace.zsh");
const ACE_BASH: &str = include_str!("../resources/shell/ace.bash");
const ACE_PS1: &str = include_str!("../resources/shell/ace.ps1");

/// Detected shell family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Pwsh,
    Unknown,
}

/// Prepared shell launch arguments and environment overrides.
#[derive(Debug, Clone)]
pub struct ShellLaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub extra_env: Vec<(String, String)>,
    pub kind: ShellKind,
}

/// Detects the target shell executable and prepares non-destructive OSC 7 / OSC 133 hook injection.
pub fn prepare_shell_launch(configured_shell: Option<&str>) -> ShellLaunchSpec {
    let program = configured_shell
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(detect_default_shell);

    let kind = classify_shell(&program);
    let hook_dir = ensure_hook_dir();

    match (kind, hook_dir) {
        (ShellKind::Zsh, Some(dir)) => {
            let zdotdir = dir.join("zsh");
            let _ = std::fs::create_dir_all(&zdotdir);
            let _ = std::fs::write(zdotdir.join(".zshrc"), ACE_ZSH.replace("\r\n", "\n"));
            let mut extra_env = vec![(
                "ZDOTDIR".to_string(),
                zdotdir.to_string_lossy().into_owned(),
            )];
            if let Ok(orig) = std::env::var("ZDOTDIR") {
                extra_env.push(("ACE_TERM_ORIG_ZDOTDIR".to_string(), orig));
            }
            ShellLaunchSpec {
                program,
                args: Vec::new(),
                extra_env,
                kind,
            }
        }
        (ShellKind::Bash, Some(dir)) => {
            let rcfile = dir.join("ace.bash");
            let _ = std::fs::write(&rcfile, ACE_BASH.replace("\r\n", "\n"));
            ShellLaunchSpec {
                program,
                args: vec![
                    "--rcfile".to_string(),
                    rcfile.to_string_lossy().into_owned(),
                ],
                extra_env: Vec::new(),
                kind,
            }
        }
        (ShellKind::Pwsh, Some(dir)) => {
            let ps1 = dir.join("ace.ps1");
            let _ = std::fs::write(&ps1, ACE_PS1);
            ShellLaunchSpec {
                program,
                args: vec![
                    "-NoExit".to_string(),
                    "-Command".to_string(),
                    format!(". '{}'", ps1.display()),
                ],
                extra_env: Vec::new(),
                kind,
            }
        }
        _ => ShellLaunchSpec {
            program,
            args: Vec::new(),
            extra_env: Vec::new(),
            kind,
        },
    }
}

fn detect_default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string())
    }
    #[cfg(windows)]
    {
        "pwsh.exe".to_string()
    }
}

fn classify_shell(program: &str) -> ShellKind {
    let name = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if name.contains("zsh") {
        ShellKind::Zsh
    } else if name.contains("bash") {
        ShellKind::Bash
    } else if name.contains("pwsh") || name.contains("powershell") {
        ShellKind::Pwsh
    } else {
        ShellKind::Unknown
    }
}

fn ensure_hook_dir() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("ace-term-hooks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_unix_hooks_have_no_crlf() {
        let spec = prepare_shell_launch(Some("/bin/bash"));
        assert_eq!(spec.kind, ShellKind::Bash);
        let rc_path = &spec.args[1];
        let content = std::fs::read_to_string(rc_path).unwrap();
        assert!(!content.contains('\r'), "ace.bash must use LF line endings");

        let zsh_spec = prepare_shell_launch(Some("/bin/zsh"));
        assert_eq!(zsh_spec.kind, ShellKind::Zsh);
        let zdotdir = zsh_spec
            .extra_env
            .iter()
            .find(|(k, _)| k == "ZDOTDIR")
            .map(|(_, v)| v.as_str())
            .unwrap();
        let zsh_content = std::fs::read_to_string(Path::new(zdotdir).join(".zshrc")).unwrap();
        assert!(
            !zsh_content.contains('\r'),
            "ace.zsh must use LF line endings"
        );
    }
}

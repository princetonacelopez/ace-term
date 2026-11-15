//! Sub-2ms zero-prefix command vs. natural-language classifier.

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::Parser;

/// Active prompt input mode toggled by `Tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Automatic zero-prefix classification.
    #[default]
    Auto,
    /// Force dispatch to PTY shell.
    Shell,
    /// Force dispatch to AI agent.
    Ai,
}

impl InputMode {
    /// Cycles `Auto` -> `Ai` -> `Shell` -> `Auto`, or toggles `[SHELL]` <-> `[AI]`.
    pub fn toggle(self) -> Self {
        match self {
            Self::Auto | Self::Shell => Self::Ai,
            Self::Ai => Self::Auto,
        }
    }

    /// Human-readable badge label for the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Shell => "SHELL",
            Self::Ai => "AI",
        }
    }
}

/// Destination for submitted user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTarget {
    /// Forward directly to the PTY shell.
    Pty,
    /// Intercept and route to `ace-agent`.
    Agent,
}

const SHELL_BUILTINS: &[&str] = &[
    "alias", "bg", "builtin", "cd", "command", "continue", "declare", "dirs", "echo", "eval",
    "exec", "exit", "export", "false", "fg", "getopts", "hash", "history", "jobs", "kill", "let",
    "local", "popd", "printf", "pushd", "pwd", "read", "readonly", "return", "set", "shift",
    "source", "suspend", "test", "times", "trap", "true", "type", "typeset", "ulimit", "umask",
    "unalias", "unset", "wait", "which", ".", "[",
];

/// Fast in-memory classifier using `tree-sitter-bash` and an indexed `$PATH` executable cache.
pub struct CommandClassifier {
    executables: HashSet<String>,
    builtins: HashSet<&'static str>,
}

impl Default for CommandClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandClassifier {
    /// Builds the classifier and indexes all executables currently on `$PATH`.
    pub fn new() -> Self {
        let mut executables = HashSet::new();
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            executables.insert(name.to_string());
                            #[cfg(windows)]
                            if let Some(stem) = Path::new(name).file_stem().and_then(|s| s.to_str())
                            {
                                executables.insert(stem.to_ascii_lowercase());
                            }
                        }
                    }
                }
            }
        }
        let builtins = SHELL_BUILTINS.iter().copied().collect();
        Self {
            executables,
            builtins,
        }
    }

    /// Creates a classifier with a fixed executable set (useful for deterministic unit tests).
    pub fn with_executables(executables: impl IntoIterator<Item = String>) -> Self {
        Self {
            executables: executables.into_iter().collect(),
            builtins: SHELL_BUILTINS.iter().copied().collect(),
        }
    }

    /// Classifies `input` according to the SPEC zero-prefix routing contract.
    pub fn classify(&self, input: &str, mode: InputMode, is_alt_screen: bool) -> RouteTarget {
        if is_alt_screen {
            return RouteTarget::Pty;
        }
        match mode {
            InputMode::Shell => return RouteTarget::Pty,
            InputMode::Ai => {
                if !input.trim().is_empty() {
                    return RouteTarget::Agent;
                }
                return RouteTarget::Pty;
            }
            InputMode::Auto => {}
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            return RouteTarget::Pty;
        }

        if has_shell_syntax(trimmed) && parses_cleanly_as_bash(trimmed) {
            return RouteTarget::Pty;
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let Some(&first) = words.first() else {
            return RouteTarget::Pty;
        };

        if self.builtins.contains(first)
            || is_explicit_path(first)
            || self.is_known_executable(first)
        {
            return RouteTarget::Pty;
        }

        if words.len() >= 2 || trimmed.ends_with('?') {
            return RouteTarget::Agent;
        }

        RouteTarget::Pty
    }

    fn is_known_executable(&self, token: &str) -> bool {
        #[cfg(windows)]
        {
            self.executables.contains(token)
                || self.executables.contains(&token.to_ascii_lowercase())
        }
        #[cfg(not(windows))]
        {
            self.executables.contains(token)
        }
    }
}

fn is_explicit_path(token: &str) -> bool {
    token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with("~/")
        || token.starts_with(".\\")
        || token.starts_with("..\\")
        || Path::new(token).components().count() > 1
}

fn has_shell_syntax(input: &str) -> bool {
    input.contains('|')
        || input.contains("&&")
        || input.contains("||")
        || input.contains(';')
        || input.contains('>')
        || input.contains('<')
        || input.contains("$(")
        || input.contains('`')
        || is_env_assignment_prefix(input)
}

fn is_env_assignment_prefix(input: &str) -> bool {
    let Some(first) = input.split_whitespace().next() else {
        return false;
    };
    let Some((name, _)) = first.split_once('=') else {
        return false;
    };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parses_cleanly_as_bash(input: &str) -> bool {
    let mut parser = Parser::new();
    let lang = tree_sitter_bash::LANGUAGE.into();
    if parser.set_language(&lang).is_err() {
        return false;
    }
    let Some(tree) = parser.parse(input, None) else {
        return false;
    };
    !tree.root_node().has_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_classifier() -> CommandClassifier {
        CommandClassifier::with_executables(["git", "ls", "cargo", "grep", "cat"].map(String::from))
    }

    #[test]
    fn alt_screen_always_routes_to_pty() {
        let c = test_classifier();
        assert_eq!(
            c.classify("explain this error?", InputMode::Ai, true),
            RouteTarget::Pty
        );
    }

    #[test]
    fn explicit_modes_override_auto() {
        let c = test_classifier();
        assert_eq!(
            c.classify("git status", InputMode::Ai, false),
            RouteTarget::Agent
        );
        assert_eq!(
            c.classify("find all rust files", InputMode::Shell, false),
            RouteTarget::Pty
        );
    }

    #[test]
    fn auto_routes_known_commands_and_shell_syntax_to_pty() {
        let c = test_classifier();
        assert_eq!(
            c.classify("git status", InputMode::Auto, false),
            RouteTarget::Pty
        );
        assert_eq!(
            c.classify("cd /tmp", InputMode::Auto, false),
            RouteTarget::Pty
        );
        assert_eq!(
            c.classify("./target/release/ace-term", InputMode::Auto, false),
            RouteTarget::Pty
        );
        assert_eq!(
            c.classify("FOO=bar ls | grep src", InputMode::Auto, false),
            RouteTarget::Pty
        );
    }

    #[test]
    fn auto_routes_natural_language_to_agent_and_single_typo_to_pty() {
        let c = test_classifier();
        assert_eq!(
            c.classify("find all rust files modified today", InputMode::Auto, false),
            RouteTarget::Agent
        );
        assert_eq!(
            c.classify("why?", InputMode::Auto, false),
            RouteTarget::Agent
        );
        assert_eq!(c.classify("gti", InputMode::Auto, false), RouteTarget::Pty);
    }
}

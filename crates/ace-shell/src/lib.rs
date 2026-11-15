//! Shell integration hooks, OSC 7/133 parsing, and zero-prefix command classification.

pub mod classifier;
pub mod hooks;
pub mod osc;

pub use classifier::{CommandClassifier, InputMode, RouteTarget};
pub use hooks::{ShellKind, ShellLaunchSpec, prepare_shell_launch};
pub use osc::{OscScanner, ShellEvent};

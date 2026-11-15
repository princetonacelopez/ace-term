//! Unified diff generation via `imara-diff`.

use imara_diff::{Algorithm, BasicLineDiffPrinter, Diff, InternedInput, UnifiedDiffConfig};

/// Computes a unified diff string between `old` and `new` for display in `AiBlockView`.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let input = InternedInput::new(old, new);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let body = diff
        .unified_diff(
            &BasicLineDiffPrinter(&input.interner),
            UnifiedDiffConfig::default(),
            &input,
        )
        .to_string();
    if body.is_empty() {
        return String::new();
    }
    format!("--- a/{path}\n+++ b/{path}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_unified_diff() {
        let old = "fn main() {\n    println!(\"old\");\n}\n";
        let new = "fn main() {\n    println!(\"new\");\n}\n";
        let out = unified_diff("src/main.rs", old, new);
        assert!(out.contains("--- a/src/main.rs"));
        assert!(out.contains("+++ b/src/main.rs"));
        assert!(out.contains("-    println!(\"old\");"));
        assert!(out.contains("+    println!(\"new\");"));
    }
}

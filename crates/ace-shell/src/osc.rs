//! Streaming OSC 7 (CWD) and OSC 133 (semantic prompt/command zones) parser.

use unicode_normalization::UnicodeNormalization;

/// Semantic shell events extracted from PTY output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    /// Current working directory changed (`OSC 7`).
    CwdChanged(String),
    /// Prompt started (`OSC 133;A`).
    PromptStart,
    /// Command input started (`OSC 133;B`).
    CommandStart,
    /// Command execution started (`OSC 133;C`).
    CommandExecuted,
    /// Command finished with optional exit code (`OSC 133;D;<code?>`).
    CommandFinished(Option<i32>),
}

/// Stateful scanner for OSC 7 and OSC 133 sequences across chunked PTY reads.
#[derive(Debug, Default)]
pub struct OscScanner {
    buf: Vec<u8>,
}

impl OscScanner {
    /// Feeds a chunk of raw PTY output bytes and returns any parsed `ShellEvent`s.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ShellEvent> {
        let mut events = Vec::new();
        self.buf.extend_from_slice(bytes);

        // Keep buffer bounded if no ESC ] is present
        if self.buf.len() > 32 * 1024 && !self.buf.windows(2).any(|w| w == b"\x1b]") {
            self.buf.clear();
            return events;
        }

        let mut search_from = 0;
        while let Some(start_rel) = find_subslice(&self.buf[search_from..], b"\x1b]") {
            let start = search_from + start_rel;
            let payload_start = start + 2;
            let Some((term_rel, term_len)) = find_osc_terminator(&self.buf[payload_start..]) else {
                // Incomplete OSC sequence; keep from `start` for next chunk.
                self.buf.drain(..start);
                return events;
            };
            let payload = &self.buf[payload_start..payload_start + term_rel];
            if let Some(ev) = parse_osc_payload(payload) {
                events.push(ev);
            }
            search_from = payload_start + term_rel + term_len;
        }

        if search_from > 0 {
            self.buf.drain(..search_from);
        } else if self.buf.len() > 8192 {
            // Retain only trailing 64 bytes in case an ESC ] spans chunks
            let keep_from = self.buf.len() - 64;
            self.buf.drain(..keep_from);
        }

        events
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_osc_terminator(slice: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < slice.len() {
        if slice[i] == 0x07 {
            return Some((i, 1));
        }
        if slice[i] == 0x1b && i + 1 < slice.len() && slice[i + 1] == b'\\' {
            return Some((i, 2));
        }
        i += 1;
    }
    None
}

fn parse_osc_payload(payload: &[u8]) -> Option<ShellEvent> {
    let text = std::str::from_utf8(payload).ok()?;
    if let Some(rest) = text.strip_prefix("7;") {
        let path = parse_osc7_uri(rest)?;
        return Some(ShellEvent::CwdChanged(path));
    }
    if let Some(rest) = text.strip_prefix("133;") {
        let mut parts = rest.split(';');
        return match parts.next()? {
            "A" => Some(ShellEvent::PromptStart),
            "B" => Some(ShellEvent::CommandStart),
            "C" => Some(ShellEvent::CommandExecuted),
            "D" => {
                let code = parts.next().and_then(|s| s.parse::<i32>().ok());
                Some(ShellEvent::CommandFinished(code))
            }
            _ => None,
        };
    }
    None
}

fn parse_osc7_uri(uri: &str) -> Option<String> {
    let stripped = if let Some(rest) = uri.strip_prefix("file://") {
        let slash_idx = rest.find('/')?;
        &rest[slash_idx..]
    } else if uri.starts_with('/') {
        uri
    } else {
        return None;
    };
    let decoded = percent_decode(stripped);
    Some(decoded.nfc().collect())
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            out.push(val);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_osc7_and_osc133_across_chunks() {
        let mut scanner = OscScanner::default();
        assert!(scanner.feed(b"\x1b]7;file://localhost/home/").is_empty());
        let evs = scanner.feed(b"ace/projects\x07\x1b]133;D;0\x07\x1b]133;A\x07");
        assert_eq!(
            evs,
            vec![
                ShellEvent::CwdChanged("/home/ace/projects".to_string()),
                ShellEvent::CommandFinished(Some(0)),
                ShellEvent::PromptStart,
            ]
        );
    }
}

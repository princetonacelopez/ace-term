//! OS keychain API key storage and regex-based secret scrubbing.

use std::sync::LazyLock;

use keyring::Entry;
use regex::Regex;

use crate::ConfigError;

const SERVICE_NAME: &str = "ace-term";

static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"sk-ant-[A-Za-z0-9\-_]{12,}",
        r"sk-[A-Za-z0-9\-_]{16,}",
        r"ghp_[A-Za-z0-9]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"(?i)bearer\s+[A-Za-z0-9\._\-]{16,}",
        r"xox[baprs]-[A-Za-z0-9\-]{10,}",
    ]
    .into_iter()
    .map(|pat| Regex::new(pat).expect("valid secret regex"))
    .collect()
});

/// Stores an LLM API key in the OS keychain for `provider` (e.g. `"anthropic"`, `"openai"`).
pub fn store_api_key(provider: &str, api_key: &str) -> Result<(), ConfigError> {
    let entry =
        Entry::new(SERVICE_NAME, provider).map_err(|e| ConfigError::Keyring(e.to_string()))?;
    entry
        .set_password(api_key)
        .map_err(|e| ConfigError::Keyring(e.to_string()))
}

/// Retrieves an LLM API key from the OS keychain, falling back to standard environment variables.
pub fn load_api_key(provider: &str) -> Result<Option<String>, ConfigError> {
    let entry =
        Entry::new(SERVICE_NAME, provider).map_err(|e| ConfigError::Keyring(e.to_string()))?;
    match entry.get_password() {
        Ok(secret) if !secret.trim().is_empty() => return Ok(Some(secret)),
        Ok(_) | Err(keyring::Error::NoEntry) => {}
        Err(err) => return Err(ConfigError::Keyring(err.to_string())),
    }

    let env_name = match provider.to_ascii_lowercase().as_str() {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        _ => None,
    };
    if let Some(var) = env_name
        && let Ok(val) = std::env::var(var)
        && !val.trim().is_empty()
    {
        return Ok(Some(val));
    }

    Ok(None)
}

/// Deletes a stored API key for `provider` from the OS keychain.
pub fn delete_api_key(provider: &str) -> Result<(), ConfigError> {
    let entry =
        Entry::new(SERVICE_NAME, provider).map_err(|e| ConfigError::Keyring(e.to_string()))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(ConfigError::Keyring(err.to_string())),
    }
}

/// Redacts common API keys and bearer tokens before sending terminal output to an LLM.
pub fn scrub_secrets(input: &str) -> String {
    let mut out = input.to_string();
    for re in SECRET_PATTERNS.iter() {
        out = re.replace_all(&out, "[REDACTED]").into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_common_tokens() {
        let raw = "export ANTHROPIC_API_KEY=sk-ant-api03-abcdef1234567890\n\
                   Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig\n\
                   GH=ghp_12345678901234567890123456";
        let scrubbed = scrub_secrets(raw);
        assert!(!scrubbed.contains("sk-ant-api03"));
        assert!(!scrubbed.contains("eyJhbGci"));
        assert!(!scrubbed.contains("ghp_12345"));
        assert!(scrubbed.contains("[REDACTED]"));
    }
}

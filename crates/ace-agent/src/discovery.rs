//! Local AI runtime detection and dynamic model listing.

use std::path::PathBuf;
use std::time::Duration;

use ace_config::secrets::load_api_key;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single model exposed by a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub label: String,
}

/// An AI provider discovered locally or configured via API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredProvider {
    pub id: String,
    pub label: String,
    pub local: bool,
    pub installed: bool,
    pub detail: String,
    pub models: Vec<DiscoveredModel>,
}

/// Returns `true` if `provider` runs locally without requiring an OS keychain API key.
pub fn is_local_provider(provider: &str) -> bool {
    matches!(
        provider.to_ascii_lowercase().as_str(),
        "antigravity" | "ollama" | "lmstudio"
    )
}

/// Searches `$PATH` and standard user bin directories for an executable binary.
pub fn find_binary(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let candidates = [format!("{name}.exe"), name.to_string()];
    #[cfg(not(windows))]
    let candidates = [name.to_string()];

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for candidate in &candidates {
                let full = dir.join(candidate);
                if full.is_file() {
                    return Some(full);
                }
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for rel in [".local/bin", ".cargo/bin", "bin"] {
            for candidate in &candidates {
                let full = home.join(rel).join(candidate);
                if full.is_file() {
                    return Some(full);
                }
            }
        }
    }

    None
}

/// Parses `agy models` stdout into a deduplicated list of models.
pub fn parse_agy_models_output(raw: &str) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();
    for segment in raw.split(['\n', '\r']) {
        let line = segment.trim();
        if line.is_empty()
            || line.starts_with("Fetching ")
            || line.chars().next().is_some_and(|c| {
                matches!(c, '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏')
            })
        {
            continue;
        }

        let (id, label) = if let Some((left, right)) = line.split_once('\t') {
            (left.trim(), right.trim())
        } else if let Some((left, right)) = line.split_once(char::is_whitespace) {
            (left.trim(), right.trim())
        } else {
            (line, line)
        };

        if !id.is_empty() && !models.iter().any(|m: &DiscoveredModel| m.id == id) {
            models.push(DiscoveredModel {
                id: id.to_string(),
                label: if label.is_empty() {
                    id.to_string()
                } else {
                    label.to_string()
                },
            });
        }
    }
    models
}

/// Auto-detects locally installed AI runtimes (`antigravity`, `ollama`, `lmstudio`)
/// and cloud providers (`anthropic`, `openai`), listing available models for each.
pub async fn discover_local_ai() -> Vec<DiscoveredProvider> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let (antigravity, ollama, lmstudio, anthropic, openai) = tokio::join!(
        detect_antigravity(),
        detect_ollama(&client),
        detect_lmstudio(&client),
        detect_anthropic(&client),
        detect_openai(&client),
    );

    let mut providers = vec![antigravity];
    if ollama.installed {
        providers.push(ollama);
    }
    if lmstudio.installed {
        providers.push(lmstudio);
    }
    providers.push(anthropic);
    providers.push(openai);
    providers
}

async fn detect_antigravity() -> DiscoveredProvider {
    let bin = find_binary("agy");
    let Some(bin_path) = bin else {
        return DiscoveredProvider {
            id: "antigravity".to_string(),
            label: "Antigravity".to_string(),
            local: true,
            installed: false,
            detail: "agy CLI not found".to_string(),
            models: Vec::new(),
        };
    };

    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&bin_path)
            .arg("models")
            .output(),
    )
    .await;

    let models = match output {
        Ok(Ok(out)) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_agy_models_output(&stdout)
        }
        _ => Vec::new(),
    };

    DiscoveredProvider {
        id: "antigravity".to_string(),
        label: "Antigravity".to_string(),
        local: true,
        installed: true,
        detail: format!("Local CLI ({})", bin_path.display()),
        models,
    }
}

async fn detect_ollama(client: &reqwest::Client) -> DiscoveredProvider {
    let bin = find_binary("ollama");
    let mut models = Vec::new();
    let mut running = false;

    if let Ok(resp) = client.get("http://127.0.0.1:11434/api/tags").send().await
        && resp.status().is_success()
        && let Ok(json) = resp.json::<Value>().await
    {
        running = true;
        if let Some(arr) = json.get("models").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    models.push(DiscoveredModel {
                        id: name.to_string(),
                        label: name.to_string(),
                    });
                }
            }
        }
    }

    if !running
        && let Some(ref bin_path) = bin
        && let Ok(Ok(out)) = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::process::Command::new(bin_path).arg("list").output(),
        )
        .await
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines().skip(1) {
            if let Some(name) = line.split_whitespace().next()
                && !name.is_empty()
            {
                models.push(DiscoveredModel {
                    id: name.to_string(),
                    label: name.to_string(),
                });
            }
        }
    }

    let installed = bin.is_some() || running;
    let detail = if running {
        "Local server (:11434)".to_string()
    } else if installed {
        "CLI installed (server stopped)".to_string()
    } else {
        "Not installed".to_string()
    };

    DiscoveredProvider {
        id: "ollama".to_string(),
        label: "Ollama".to_string(),
        local: true,
        installed,
        detail,
        models,
    }
}

async fn detect_lmstudio(client: &reqwest::Client) -> DiscoveredProvider {
    let mut models = Vec::new();
    let mut running = false;

    if let Ok(resp) = client.get("http://127.0.0.1:1234/v1/models").send().await
        && resp.status().is_success()
        && let Ok(json) = resp.json::<Value>().await
    {
        running = true;
        if let Some(arr) = json.get("data").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    models.push(DiscoveredModel {
                        id: id.to_string(),
                        label: id.to_string(),
                    });
                }
            }
        }
    }

    DiscoveredProvider {
        id: "lmstudio".to_string(),
        label: "LM Studio".to_string(),
        local: true,
        installed: running,
        detail: if running {
            "Local server (:1234)".to_string()
        } else {
            "Not running".to_string()
        },
        models,
    }
}

async fn detect_anthropic(client: &reqwest::Client) -> DiscoveredProvider {
    let key = load_api_key("anthropic").ok().flatten().or_else(|| {
        std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    });

    let mut models = Vec::new();
    if let Some(ref api_key) = key
        && let Ok(resp) = client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
        && resp.status().is_success()
        && let Ok(json) = resp.json::<Value>().await
        && let Some(arr) = json.get("data").and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                let label = item
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id);
                models.push(DiscoveredModel {
                    id: id.to_string(),
                    label: label.to_string(),
                });
            }
        }
    }

    if models.is_empty() {
        models = vec![
            DiscoveredModel {
                id: "claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
            },
            DiscoveredModel {
                id: "claude-opus-4-6".to_string(),
                label: "Claude Opus 4.6".to_string(),
            },
            DiscoveredModel {
                id: "claude-haiku-4-5".to_string(),
                label: "Claude Haiku 4.5".to_string(),
            },
        ];
    }

    DiscoveredProvider {
        id: "anthropic".to_string(),
        label: "Anthropic".to_string(),
        local: false,
        installed: key.is_some(),
        detail: if key.is_some() {
            "API key configured".to_string()
        } else {
            "Cloud API".to_string()
        },
        models,
    }
}

async fn detect_openai(client: &reqwest::Client) -> DiscoveredProvider {
    let key = load_api_key("openai").ok().flatten().or_else(|| {
        std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    });

    let mut models = Vec::new();
    if let Some(ref api_key) = key
        && let Ok(resp) = client
            .get("https://api.openai.com/v1/models")
            .bearer_auth(api_key)
            .send()
            .await
        && resp.status().is_success()
        && let Ok(json) = resp.json::<Value>().await
        && let Some(arr) = json.get("data").and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|v| v.as_str())
                && (id.starts_with("gpt-")
                    || id.starts_with("o1")
                    || id.starts_with("o3")
                    || id.starts_with("o4"))
            {
                models.push(DiscoveredModel {
                    id: id.to_string(),
                    label: id.to_string(),
                });
            }
        }
        models.sort_by(|a, b| a.id.cmp(&b.id));
    }

    if models.is_empty() {
        models = vec![
            DiscoveredModel {
                id: "gpt-4o".to_string(),
                label: "GPT-4o".to_string(),
            },
            DiscoveredModel {
                id: "gpt-4.1".to_string(),
                label: "GPT-4.1".to_string(),
            },
            DiscoveredModel {
                id: "o3-mini".to_string(),
                label: "o3-mini".to_string(),
            },
        ];
    }

    DiscoveredProvider {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        local: false,
        installed: key.is_some(),
        detail: if key.is_some() {
            "API key configured".to_string()
        } else {
            "Cloud API".to_string()
        },
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agy_models_output_cleanly() {
        let sample = "\r⠋ Fetching available models...\r\nFetching available models...\n\
            gemini-4-argon-medium\tGemini 4 Argon (Medium)\n\
            claude-sonnet-5-5-high\tClaude Sonnet 5.5 (High)\n\
            gemini-4-argon-medium\tGemini 4 Argon (Medium)\n";
        let parsed = parse_agy_models_output(sample);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "gemini-4-argon-medium");
        assert_eq!(parsed[0].label, "Gemini 4 Argon (Medium)");
        assert_eq!(parsed[1].id, "claude-sonnet-5-5-high");
    }
}

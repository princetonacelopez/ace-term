//! Streaming AI agent orchestrator, built-in tools, and unified diff generation.

pub mod diff;
pub mod tools;

use std::path::{Path, PathBuf};

use ace_config::secrets::{load_api_key, scrub_secrets};
use ace_config::{AceConfig, ApprovalMode};
use ace_env::EnvSnapshot;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

pub use tools::{PendingApproval, ToolSpec, builtin_tool_specs, execute_approved};

const MAX_TOOL_TURNS: usize = 8;

/// Errors returned by the agent orchestrator.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("missing API key for provider '{0}'; run `ace-term auth set {0}`")]
    MissingApiKey(String),

    #[error("provider error ({status}): {body}")]
    Provider { status: u16, body: String },
}

/// A single turn in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Incremental events emitted by an active agent turn for `AiBlockView`.
#[derive(Debug)]
pub enum AgentEvent {
    /// Streaming assistant text chunk.
    Token(String),
    /// A tool call began executing.
    ToolStarted { name: String, summary: String },
    /// User confirmation is required (`Accept`/`Reject` or `Approve`/`Deny`).
    ApprovalRequired {
        action: PendingApproval,
        respond: oneshot::Sender<bool>,
    },
    /// Tool execution completed.
    ToolFinished { name: String, output: String },
    /// Assistant finished the turn.
    Done,
    /// Fatal error during the turn.
    Error(String),
}

#[derive(Debug, Clone)]
struct ToolCallRequest {
    id: String,
    name: String,
    arguments: Value,
}

#[derive(Debug)]
enum StreamResult {
    TextOnly,
    ToolCalls {
        assistant_text: String,
        calls: Vec<ToolCallRequest>,
    },
}

/// Runs a multi-step streaming agent turn with tool execution and approval gating.
pub async fn run_agent_turn(
    prompt: String,
    history: Vec<ChatMessage>,
    env: EnvSnapshot,
    cfg: AceConfig,
    events: mpsc::UnboundedSender<AgentEvent>,
) {
    let cwd = if env.cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        env.cwd.clone()
    };

    let scrubbed_prompt = scrub_secrets(&prompt);
    let system_prompt = build_system_prompt(&env);

    let provider = cfg.ai.provider.to_ascii_lowercase();
    let api_key = if provider == "ollama" {
        None
    } else {
        match load_api_key(&provider) {
            Ok(Some(k)) => Some(k),
            Ok(None) => {
                let _ = events.send(AgentEvent::Error(
                    AgentError::MissingApiKey(cfg.ai.provider.clone()).to_string(),
                ));
                return;
            }
            Err(err) => {
                let _ = events.send(AgentEvent::Error(err.to_string()));
                return;
            }
        }
    };

    let client = reqwest::Client::new();
    let result = if provider == "anthropic" {
        run_anthropic_loop(
            &client,
            api_key.as_deref().unwrap_or_default(),
            &cfg,
            &system_prompt,
            &history,
            &scrubbed_prompt,
            &cwd,
            &events,
        )
        .await
    } else {
        run_openai_loop(
            &client,
            api_key.as_deref(),
            &cfg,
            &system_prompt,
            &history,
            &scrubbed_prompt,
            &cwd,
            &events,
        )
        .await
    };

    match result {
        Ok(()) => {
            let _ = events.send(AgentEvent::Done);
        }
        Err(err) => {
            let _ = events.send(AgentEvent::Error(err.to_string()));
        }
    }
}

fn build_system_prompt(env: &EnvSnapshot) -> String {
    format!(
        "You are Ace Term's embedded engineering assistant. \
         Respond concisely with Markdown and code blocks. \
         Use the provided tools when inspecting files, editing files, or running commands.\n\
         Workspace context: {}",
        env.prompt_summary()
    )
}

async fn resolve_tool_call(
    call: &ToolCallRequest,
    cwd: &Path,
    approval_mode: ApprovalMode,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> String {
    match call.name.as_str() {
        "read_file" | "list_dir" | "search_code" => {
            let _ = events.send(AgentEvent::ToolStarted {
                name: call.name.clone(),
                summary: call.arguments.to_string(),
            });
            let out = tools::execute_read_only(&call.name, &call.arguments, cwd)
                .unwrap_or_else(|e| format!("error: {e}"));
            let _ = events.send(AgentEvent::ToolFinished {
                name: call.name.clone(),
                output: out.clone(),
            });
            out
        }
        "write_file" => match tools::prepare_write_file(call.id.clone(), &call.arguments, cwd) {
            Ok(action) => {
                let approved = request_or_auto_approve(action.clone(), approval_mode, events).await;
                if approved {
                    let out = execute_approved(&action)
                        .await
                        .unwrap_or_else(|e| format!("error: {e}"));
                    let _ = events.send(AgentEvent::ToolFinished {
                        name: "write_file".to_string(),
                        output: out.clone(),
                    });
                    out
                } else {
                    "User rejected the file write.".to_string()
                }
            }
            Err(err) => format!("error: {err}"),
        },
        "run_shell" => {
            let Some(command) = call.arguments.get("command").and_then(Value::as_str) else {
                return "error: missing 'command'".to_string();
            };
            let action = PendingApproval::RunShell {
                id: call.id.clone(),
                command: command.to_string(),
                cwd: cwd.to_path_buf(),
            };
            let approved = request_or_auto_approve(action.clone(), approval_mode, events).await;
            if approved {
                let out = execute_approved(&action)
                    .await
                    .unwrap_or_else(|e| format!("error: {e}"));
                let _ = events.send(AgentEvent::ToolFinished {
                    name: "run_shell".to_string(),
                    output: out.clone(),
                });
                out
            } else {
                "User denied shell command execution.".to_string()
            }
        }
        other => format!("error: unknown tool '{other}'"),
    }
}

async fn request_or_auto_approve(
    action: PendingApproval,
    approval_mode: ApprovalMode,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> bool {
    if approval_mode == ApprovalMode::Auto {
        return true;
    }
    let (tx, rx) = oneshot::channel();
    if events
        .send(AgentEvent::ApprovalRequired {
            action,
            respond: tx,
        })
        .is_err()
    {
        return false;
    }
    rx.await.unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
async fn run_anthropic_loop(
    client: &reqwest::Client,
    api_key: &str,
    cfg: &AceConfig,
    system_prompt: &str,
    history: &[ChatMessage],
    prompt: &str,
    cwd: &Path,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), AgentError> {
    let base = if cfg.ai.base_url.trim().is_empty() {
        "https://api.anthropic.com"
    } else {
        cfg.ai.base_url.trim_end_matches('/')
    };
    let url = format!("{base}/v1/messages");

    let tools_json: Vec<Value> = builtin_tool_specs()
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })
        })
        .collect();

    let mut messages: Vec<Value> = history
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    messages.push(json!({ "role": "user", "content": prompt }));

    for _ in 0..MAX_TOOL_TURNS {
        let body = json!({
            "model": cfg.ai.model,
            "max_tokens": 4096,
            "system": system_prompt,
            "tools": tools_json,
            "messages": messages,
            "stream": true,
        });

        let resp = client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::Provider { status, body: text });
        }

        match parse_anthropic_sse(resp, events).await? {
            StreamResult::TextOnly => break,
            StreamResult::ToolCalls {
                assistant_text,
                calls,
            } => {
                let mut content_blocks = Vec::new();
                if !assistant_text.is_empty() {
                    content_blocks.push(json!({ "type": "text", "text": assistant_text }));
                }
                for c in &calls {
                    content_blocks.push(json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": c.arguments,
                    }));
                }
                messages.push(json!({ "role": "assistant", "content": content_blocks }));

                let mut tool_results = Vec::new();
                for c in &calls {
                    let output = resolve_tool_call(c, cwd, cfg.ai.approval_mode, events).await;
                    tool_results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": c.id,
                        "content": output,
                    }));
                }
                messages.push(json!({ "role": "user", "content": tool_results }));
            }
        }
    }

    Ok(())
}

async fn parse_anthropic_sse(
    resp: reqwest::Response,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<StreamResult, AgentError> {
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut assistant_text = String::new();
    let mut tool_blocks: Vec<(String, String, String)> = Vec::new(); // (id, name, json_buf)
    let mut current_tool_idx: Option<usize> = None;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            match v.get("type").and_then(Value::as_str) {
                Some("content_block_start") => {
                    if let Some(cb) = v.get("content_block")
                        && cb.get("type").and_then(Value::as_str) == Some("tool_use")
                    {
                        let id = cb
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let name = cb
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        tool_blocks.push((id, name, String::new()));
                        current_tool_idx = Some(tool_blocks.len() - 1);
                    }
                }
                Some("content_block_delta") => {
                    if let Some(delta) = v.get("delta") {
                        match delta.get("type").and_then(Value::as_str) {
                            Some("text_delta") => {
                                if let Some(text) = delta.get("text").and_then(Value::as_str) {
                                    assistant_text.push_str(text);
                                    let _ = events.send(AgentEvent::Token(text.to_string()));
                                }
                            }
                            Some("input_json_delta") => {
                                if let (Some(idx), Some(partial)) = (
                                    current_tool_idx,
                                    delta.get("partial_json").and_then(Value::as_str),
                                ) {
                                    tool_blocks[idx].2.push_str(partial);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some("content_block_stop") => {
                    current_tool_idx = None;
                }
                _ => {}
            }
        }
    }

    if tool_blocks.is_empty() {
        Ok(StreamResult::TextOnly)
    } else {
        let calls = tool_blocks
            .into_iter()
            .map(|(id, name, raw_json)| {
                let arguments = serde_json::from_str(&raw_json).unwrap_or_else(|_| json!({}));
                ToolCallRequest {
                    id,
                    name,
                    arguments,
                }
            })
            .collect();
        Ok(StreamResult::ToolCalls {
            assistant_text,
            calls,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_openai_loop(
    client: &reqwest::Client,
    api_key: Option<&str>,
    cfg: &AceConfig,
    system_prompt: &str,
    history: &[ChatMessage],
    prompt: &str,
    cwd: &Path,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), AgentError> {
    let base = if !cfg.ai.base_url.trim().is_empty() {
        cfg.ai.base_url.trim_end_matches('/')
    } else if cfg.ai.provider.eq_ignore_ascii_case("ollama") {
        "http://127.0.0.1:11434"
    } else {
        "https://api.openai.com"
    };
    let url = format!("{base}/v1/chat/completions");

    let tools_json: Vec<Value> = builtin_tool_specs()
        .into_iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();

    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system_prompt })];
    for m in history {
        messages.push(json!({ "role": m.role, "content": m.content }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));

    for _ in 0..MAX_TOOL_TURNS {
        let body = json!({
            "model": cfg.ai.model,
            "messages": messages,
            "tools": tools_json,
            "stream": true,
        });

        let mut req = client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::Provider { status, body: text });
        }

        match parse_openai_sse(resp, events).await? {
            StreamResult::TextOnly => break,
            StreamResult::ToolCalls {
                assistant_text,
                calls,
            } => {
                let tool_calls_json: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": c.arguments.to_string(),
                            }
                        })
                    })
                    .collect();
                messages.push(json!({
                    "role": "assistant",
                    "content": assistant_text,
                    "tool_calls": tool_calls_json,
                }));

                for c in &calls {
                    let output = resolve_tool_call(c, cwd, cfg.ai.approval_mode, events).await;
                    debug!(tool = %c.name, "tool completed");
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": c.id,
                        "content": output,
                    }));
                }
            }
        }
    }

    Ok(())
}

async fn parse_openai_sse(
    resp: reqwest::Response,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<StreamResult, AgentError> {
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut assistant_text = String::new();
    let mut tool_acc: Vec<(String, String, String)> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let Some(delta) = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
            else {
                continue;
            };

            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                assistant_text.push_str(text);
                let _ = events.send(AgentEvent::Token(text.to_string()));
            }

            if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                for tc in tcs {
                    let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    while tool_acc.len() <= idx {
                        tool_acc.push((String::new(), String::new(), String::new()));
                    }
                    if let Some(id) = tc.get("id").and_then(Value::as_str) {
                        tool_acc[idx].0.push_str(id);
                    }
                    if let Some(func) = tc.get("function") {
                        if let Some(name) = func.get("name").and_then(Value::as_str) {
                            tool_acc[idx].1.push_str(name);
                        }
                        if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                            tool_acc[idx].2.push_str(args);
                        }
                    }
                }
            }
        }
    }

    if tool_acc.is_empty() {
        Ok(StreamResult::TextOnly)
    } else {
        let calls = tool_acc
            .into_iter()
            .enumerate()
            .map(|(i, (id, name, raw_json))| ToolCallRequest {
                id: if id.is_empty() {
                    format!("call_{i}")
                } else {
                    id
                },
                name,
                arguments: serde_json::from_str(&raw_json).unwrap_or_else(|_| json!({})),
            })
            .collect();
        Ok(StreamResult::ToolCalls {
            assistant_text,
            calls,
        })
    }
}

//! Inline terminal AI turn state and pending tool approval handling.

use ace_agent::AgentEvent;
use tokio::sync::oneshot;

/// Active pending tool confirmation held by an `AiBlock`.
pub struct PendingApprovalPrompt {
    pub respond: oneshot::Sender<bool>,
}

/// State of an active inline AI turn.
pub struct AiBlock {
    pub prompt: String,
    pub response: String,
    pub pending_approval: Option<PendingApprovalPrompt>,
}

impl AiBlock {
    /// Creates a new `AiBlock` for `prompt`.
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            response: String::new(),
            pending_approval: None,
        }
    }

    /// Applies an `AgentEvent` to this turn's state.
    pub fn apply_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Token(delta) => {
                self.response.push_str(&delta);
            }
            AgentEvent::ApprovalRequired { respond, .. } => {
                self.pending_approval = Some(PendingApprovalPrompt { respond });
            }
            AgentEvent::ToolStarted { .. }
            | AgentEvent::ToolFinished { .. }
            | AgentEvent::Done
            | AgentEvent::Error(_) => {}
        }
    }

    /// Resolves the pending approval prompt with `approved`.
    pub fn resolve_approval(&mut self, approved: bool) {
        if let Some(pending) = self.pending_approval.take() {
            let _ = pending.respond.send(approved);
        }
    }
}

//! `AiBlockView`: inline streaming AI response card, unified diff reviewer, and tool approval.

use ace_agent::{AgentEvent, PendingApproval};
use ace_vt::RgbColor;
use gpui_kit::*;
use tokio::sync::oneshot;

/// Theme colors passed from `TerminalCanvasView` to `AiBlockView`.
#[derive(Debug, Clone, Copy)]
pub struct AiBlockColors {
    pub bg: RgbColor,
    pub surface: RgbColor,
    pub border: RgbColor,
    pub fg: RgbColor,
    pub muted: RgbColor,
    pub accent: RgbColor,
}

fn hsla(c: RgbColor, a: f32) -> Hsla {
    Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a,
    }
    .into()
}

/// Active pending tool confirmation held by an `AiBlock`.
pub struct PendingApprovalPrompt {
    pub action: PendingApproval,
    pub respond: oneshot::Sender<bool>,
}

/// State of an inline AI conversation block.
pub struct AiBlock {
    pub prompt: String,
    pub response: String,
    pub tool_status: Option<String>,
    pub pending_approval: Option<PendingApprovalPrompt>,
    pub error: Option<String>,
    pub done: bool,
}

impl AiBlock {
    /// Creates a new streaming `AiBlock` for `prompt`.
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            response: String::new(),
            tool_status: None,
            pending_approval: None,
            error: None,
            done: false,
        }
    }

    /// Applies an `AgentEvent` to this block's state.
    pub fn apply_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Token(delta) => {
                self.response.push_str(&delta);
            }
            AgentEvent::ToolStarted { name, summary } => {
                self.tool_status = Some(format!("Running {name}: {summary}"));
            }
            AgentEvent::ApprovalRequired { action, respond } => {
                self.tool_status = None;
                self.pending_approval = Some(PendingApprovalPrompt { action, respond });
            }
            AgentEvent::ToolFinished { name, .. } => {
                self.tool_status = Some(format!("Completed {name}"));
            }
            AgentEvent::Done => {
                self.tool_status = None;
                self.done = true;
            }
            AgentEvent::Error(err) => {
                self.tool_status = None;
                self.error = Some(err);
                self.done = true;
            }
        }
    }

    /// Resolves the pending approval prompt with `approved`.
    pub fn resolve_approval(&mut self, approved: bool) {
        if let Some(pending) = self.pending_approval.take() {
            let _ = pending.respond.send(approved);
        }
    }
}

/// Renders an `AiBlock` as an inline card above the status bar.
pub fn render_ai_block<FClose, FApprove, FReject>(
    block: &AiBlock,
    colors: AiBlockColors,
    on_close: FClose,
    on_approve: FApprove,
    on_reject: FReject,
) -> impl IntoElement
where
    FClose: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    FApprove: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    FReject: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let green = RgbColor::new(0xa6, 0xe3, 0xa1);
    let red = RgbColor::new(0xf3, 0x8b, 0xa8);
    let cyan = RgbColor::new(0x94, 0xe2, 0xd5);

    let mut card = div()
        .w_full()
        .max_h(px(340.0))
        .p(px(12.0))
        .bg(hsla(colors.surface, 0.98))
        .border_t_1()
        .border_color(hsla(colors.accent, 0.45))
        .flex()
        .flex_col()
        .gap(px(8.0))
        // Header row
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(hsla(colors.accent, 0.2))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hsla(colors.accent, 1.0))
                                .child(if block.done { "AI" } else { "AI..." }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(hsla(colors.fg, 1.0))
                                .child(block.prompt.clone()),
                        ),
                )
                .child(
                    div()
                        .id("ai-block-close")
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .border_1()
                        .border_color(hsla(colors.border, 1.0))
                        .text_size(px(11.0))
                        .text_color(hsla(colors.muted, 1.0))
                        .child("Dismiss")
                        .on_click(on_close),
                ),
        );

    if !block.response.is_empty() {
        card = card.child(
            div()
                .w_full()
                .max_h(px(160.0))
                .overflow_hidden()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(hsla(colors.bg, 0.7))
                .text_size(px(12.0))
                .text_color(hsla(colors.fg, 1.0))
                .child(block.response.clone()),
        );
    }

    if let Some(status) = &block.tool_status {
        card = card.child(
            div()
                .text_size(px(11.0))
                .text_color(hsla(colors.muted, 1.0))
                .child(status.clone()),
        );
    }

    if let Some(pending) = &block.pending_approval {
        match &pending.action {
            PendingApproval::WriteFile {
                display_path, diff, ..
            } => {
                let mut diff_box = div()
                    .w_full()
                    .max_h(px(140.0))
                    .overflow_hidden()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(hsla(colors.bg, 0.9))
                    .border_1()
                    .border_color(hsla(colors.border, 1.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(colors.accent, 1.0))
                            .child(format!("Proposed edit: {display_path}")),
                    );

                for line in diff.lines().take(24) {
                    let color = if line.starts_with('+') && !line.starts_with("+++") {
                        green
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        red
                    } else if line.starts_with("@@") {
                        cyan
                    } else {
                        colors.muted
                    };
                    diff_box = diff_box.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(color, 1.0))
                            .child(line.to_string()),
                    );
                }

                card = card.child(diff_box).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(action_btn(
                            "ai-accept-diff",
                            "Accept",
                            green,
                            colors.bg,
                            on_approve,
                        ))
                        .child(action_btn(
                            "ai-reject-diff",
                            "Reject",
                            red,
                            colors.bg,
                            on_reject,
                        )),
                );
            }
            PendingApproval::RunShell { command, .. } => {
                card = card.child(
                    div()
                        .p(px(8.0))
                        .rounded(px(6.0))
                        .bg(hsla(colors.bg, 0.9))
                        .border_1()
                        .border_color(hsla(colors.border, 1.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(hsla(colors.fg, 1.0))
                                .child(format!("Run shell command: {command}")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(action_btn(
                                    "ai-approve-shell",
                                    "Approve",
                                    green,
                                    colors.bg,
                                    on_approve,
                                ))
                                .child(action_btn(
                                    "ai-deny-shell",
                                    "Deny",
                                    red,
                                    colors.bg,
                                    on_reject,
                                )),
                        ),
                );
            }
        }
    }

    if let Some(err) = &block.error {
        card = card.child(
            div()
                .text_size(px(12.0))
                .text_color(hsla(red, 1.0))
                .child(err.clone()),
        );
    }

    card
}

fn action_btn<F: Fn(&ClickEvent, &mut Window, &mut App) + 'static>(
    id: &'static str,
    label: &'static str,
    bg: RgbColor,
    fg: RgbColor,
    on_click: F,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .bg(hsla(bg, 1.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hsla(fg, 1.0))
        .child(label)
        .on_click(on_click)
}

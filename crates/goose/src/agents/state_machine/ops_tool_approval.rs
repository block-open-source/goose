//! Resolves approval requirements for pending tool requests.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use async_trait::async_trait;

use crate::agents::state_machine::ops_toolcalling::request_was_advertised;
use crate::agents::state_machine::{
    applied, messages_since_kickoff, not_applicable, ConversationEffect, Emitter, GooseEffect,
    Operation, OperationResult,
};
use crate::config::permission::PermissionLevel;
use crate::config::GooseMode;
use crate::conversation::message::{ActionRequiredData, Message, MessageContent, ToolRequest};
use crate::conversation::Conversation;
use crate::permission::Permission;
use crate::session::Session;
use crate::tool_inspection::{
    get_security_finding_id_from_results, InspectionAction, ToolInspectionManager,
};
use tokio::sync::Mutex;

pub const TOOL_EXECUTABLE_KEY: &str = "goose.executable";

pub struct ToolApprovalOperation<'a> {
    goose_mode: &'a Mutex<GooseMode>,
    tool_inspection_manager: &'a ToolInspectionManager,
}

impl<'a> ToolApprovalOperation<'a> {
    pub fn new(
        goose_mode: &'a Mutex<GooseMode>,
        tool_inspection_manager: &'a ToolInspectionManager,
    ) -> Self {
        Self {
            goose_mode,
            tool_inspection_manager,
        }
    }
}

#[async_trait]
impl Operation<Session, GooseEffect> for ToolApprovalOperation<'_> {
    fn name(&self) -> &'static str {
        "tool_approval"
    }

    async fn run(
        &self,
        session: &Session,
        conversation: &Conversation,
        _emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let goose_mode = *self.goose_mode.lock().await;
        if goose_mode == GooseMode::Chat {
            return not_applicable();
        }

        let state = ApprovalState::from_messages(messages_since_kickoff(conversation)?);
        let mut effects = Vec::new();

        for pending in state.responses() {
            let executable = permission_allows(&pending.permission);

            if let Some(tool_name) = pending.tool_name {
                if pending.permission == Permission::AlwaysAllow && pending.executable != Some(true)
                {
                    self.tool_inspection_manager
                        .update_permission_manager(&tool_name, PermissionLevel::AlwaysAllow)
                        .await;
                } else if pending.permission == Permission::AlwaysDeny {
                    self.tool_inspection_manager
                        .update_permission_manager(&tool_name, PermissionLevel::NeverAllow)
                        .await;
                }
            }

            if pending.executable != Some(executable) {
                effects.push(mark_executable(&pending.tool_call_id, executable));
            }
        }

        let pending_requests = state.pending_requests();
        if !pending_requests.is_empty() {
            let inspection_results = self
                .tool_inspection_manager
                .inspect_tools(
                    &session.id,
                    &pending_requests,
                    conversation.messages(),
                    goose_mode,
                )
                .await?;
            let permission_check_result = self
                .tool_inspection_manager
                .process_inspection_results_with_permission_inspector(
                    &pending_requests,
                    &inspection_results,
                )
                .unwrap_or_else(
                    || crate::permission::permission_judge::PermissionCheckResult {
                        approved: Vec::new(),
                        needs_approval: Vec::new(),
                        denied: Vec::new(),
                    },
                );

            for request in permission_check_result.denied {
                effects.push(mark_executable(&request.id, false));
            }

            for request in permission_check_result.needs_approval {
                let tool_call = request.tool_call.clone()?;
                effects.push(mark_executable(&request.id, false));

                let security_message = inspection_results
                    .iter()
                    .find(|result| result.tool_request_id == request.id)
                    .and_then(|result| match &result.action {
                        InspectionAction::RequireApproval(Some(message)) => Some(message.clone()),
                        _ => None,
                    });

                let action_required = Message::assistant()
                    .with_action_required(
                        request.id.clone(),
                        tool_call.name.to_string(),
                        tool_call.arguments.clone().unwrap_or_default(),
                        security_message,
                    )
                    .user_only();
                effects.push(action_required.into());

                if let Some(finding_id) =
                    get_security_finding_id_from_results(&request.id, &inspection_results)
                {
                    tracing::info!(
                        monotonic_counter.goose.prompt_injection_user_decisions = 1,
                        security.event_type = "approval_request",
                        security.finding_id = %finding_id,
                        tool.request_id = %request.id,
                        "security finding: approval requested"
                    );
                }
            }
        }

        if effects.is_empty() {
            not_applicable()
        } else {
            applied(effects)
        }
    }
}

struct PendingResponse {
    tool_call_id: String,
    tool_name: Option<String>,
    permission: Permission,
    executable: Option<bool>,
}

pub(super) struct ApprovalState {
    answered: HashSet<String>,
    approval_requests: HashSet<String>,
    approval_responses: HashMap<String, Permission>,
    tool_requests: Vec<ToolRequest>,
}

impl ApprovalState {
    pub(super) fn from_messages(messages: &[Message]) -> Self {
        let mut answered = HashSet::new();
        let mut approval_requests = HashSet::new();
        let mut approval_responses = HashMap::new();
        let mut tool_requests = Vec::new();

        for message in messages {
            for content in &message.content {
                match content {
                    MessageContent::ToolResponse(response) => {
                        answered.insert(response.id.clone());
                    }
                    MessageContent::ToolRequest(request)
                        if request_was_advertised(messages, request) =>
                    {
                        tool_requests.push(request.clone());
                    }
                    MessageContent::ActionRequired(action) => match &action.data {
                        ActionRequiredData::ToolConfirmation { id, .. } => {
                            approval_requests.insert(id.clone());
                        }
                        ActionRequiredData::ToolConfirmationResponse { id, permission } => {
                            approval_responses.insert(id.clone(), permission.clone());
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        Self {
            answered,
            approval_requests,
            approval_responses,
            tool_requests,
        }
    }

    fn responses(&self) -> Vec<PendingResponse> {
        self.tool_requests
            .iter()
            .filter_map(|request| {
                if self.answered.contains(&request.id) {
                    return None;
                }
                let permission = self.approval_responses.get(&request.id)?.clone();
                let tool_name = request
                    .tool_call
                    .as_ref()
                    .ok()
                    .map(|tool_call| tool_call.name.to_string());
                Some(PendingResponse {
                    tool_call_id: request.id.clone(),
                    tool_name,
                    permission,
                    executable: request_executable(request),
                })
            })
            .collect()
    }

    fn pending_requests(&self) -> Vec<ToolRequest> {
        self.tool_requests
            .iter()
            .filter_map(|request| {
                if self.answered.contains(&request.id)
                    || self.approval_requests.contains(&request.id)
                    || self.approval_responses.contains_key(&request.id)
                    || request.was_executed_externally()
                    || request_executable(request) == Some(false)
                    || request.tool_call.is_err()
                {
                    return None;
                }
                Some(request.clone())
            })
            .collect()
    }

    pub(super) fn has_confirmation_request(&self, request_id: &str) -> bool {
        self.approval_requests.contains(request_id)
            && self
                .tool_requests
                .iter()
                .any(|request| request.id == request_id)
    }

    pub(super) fn has_unapplied_confirmation_response(&self) -> bool {
        self.tool_requests.iter().any(|request| {
            !self.answered.contains(&request.id)
                && self.approval_requests.contains(&request.id)
                && self.approval_responses.contains_key(&request.id)
        })
    }

    pub(super) fn confirmation_response(&self, request_id: &str) -> Option<&Permission> {
        self.approval_responses.get(request_id)
    }

    pub(super) fn has_tool_response(&self, request_id: &str) -> bool {
        self.answered.contains(request_id)
    }
}

pub fn request_executable(request: &ToolRequest) -> Option<bool> {
    request
        .tool_meta
        .as_ref()
        .and_then(|meta| meta.get(TOOL_EXECUTABLE_KEY))
        .and_then(|value| value.as_bool())
}

fn permission_allows(permission: &Permission) -> bool {
    matches!(permission, Permission::AllowOnce | Permission::AlwaysAllow)
}

fn mark_executable(tool_call_id: &str, executable: bool) -> GooseEffect {
    ConversationEffect::PatchToolRequestMeta {
        tool_call_id: tool_call_id.to_string(),
        patch: serde_json::json!({ TOOL_EXECUTABLE_KEY: executable }),
    }
    .into()
}

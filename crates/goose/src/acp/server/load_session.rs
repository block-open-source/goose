use super::message_meta::{
    content_chunk_for_message, merge_message_meta, populate_output_token_limit_content,
};
use super::tool_calls::conversion::{
    build_initial_tool_call_with_message_meta, tool_call_update_fields_from_response,
    trusted_update_meta,
};
use super::tool_calls::enrichment::tool_chain_summary;
use super::*;
use agent_client_protocol::schema::v1::ToolCall;

fn replay_audience_annotations(audience: &[Role]) -> Annotations {
    Annotations::new().audience(
        audience
            .iter()
            .map(|role| match role {
                Role::Assistant => agent_client_protocol::schema::v1::Role::Assistant,
                Role::User => agent_client_protocol::schema::v1::Role::User,
            })
            .collect::<Vec<_>>(),
    )
}

fn messages_for_acp_replay(conversation: &Conversation) -> Vec<Message> {
    conversation
        .messages()
        .iter()
        .filter(|message| message.is_user_visible())
        .map(Message::user_visible_content)
        .map(|mut message| {
            populate_output_token_limit_content(&mut message);
            message
        })
        .filter(|message| !message.content.is_empty())
        .collect()
}

fn send_replay_content_chunk(
    cx: &ConnectionTo<Client>,
    session_id: &SessionId,
    message: &Message,
    content: ContentBlock,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let chunk = content_chunk_for_message(message, content);
    let update = match message.role {
        Role::User => SessionUpdate::UserMessageChunk(chunk),
        Role::Assistant => SessionUpdate::AgentMessageChunk(chunk),
    };
    cx.send_notification(SessionNotification::new(session_id.clone(), update))
}

fn build_replayed_tool_call(
    tool_request: &ToolRequest,
    message: &Message,
    client_requests_tool_call_label_enrichment: bool,
) -> ToolCall {
    let mut tool_call = build_initial_tool_call_with_message_meta(
        tool_request,
        message,
        client_requests_tool_call_label_enrichment,
    );

    if !client_requests_tool_call_label_enrichment {
        return tool_call;
    }

    let Some(chain_summary) = tool_request.generated_chain_summary() else {
        return tool_call;
    };
    let goose_meta = tool_call
        .meta
        .get_or_insert_default()
        .entry("goose".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !goose_meta.is_object() {
        *goose_meta = serde_json::Value::Object(serde_json::Map::new());
    }
    goose_meta
        .as_object_mut()
        .expect("goose metadata was initialized as an object")
        .extend([tool_chain_summary(&chain_summary)]);

    tool_call
}

/// Where to start replaying so that at most roughly `tail` trailing messages
/// are sent without splitting a turn: walk backwards from `len - tail` to the
/// nearest turn boundary (a visible user message that is not a tool response),
/// so tool request/response pairs are never separated. Returns 0 (full
/// replay) when the history is short enough or no boundary exists.
fn replay_start_index(messages: &[Message], tail: usize) -> usize {
    if tail == 0 || messages.len() <= tail {
        return 0;
    }
    let candidate = messages.len() - tail;
    messages[..=candidate]
        .iter()
        .rposition(|message| message.role == Role::User && !message.is_tool_response())
        .unwrap_or(0)
}

fn replay_tail_from_meta(meta: Option<&Meta>) -> Option<usize> {
    meta.and_then(|m| m.get("replayTail"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
}

fn replay_conversation_to_client(
    cx: &ConnectionTo<Client>,
    session: &Session,
    supports_goose_custom_notifications: bool,
    client_requests_tool_call_label_enrichment: bool,
    replay_tail: Option<usize>,
) -> Result<usize, agent_client_protocol::Error> {
    let session_id = SessionId::new(session.id.clone());
    let tool_call_notifier = ToolCallNotifier::new(cx, &session_id);

    let messages = session
        .conversation
        .as_ref()
        .map(messages_for_acp_replay)
        .unwrap_or_default();
    let skipped = replay_tail
        .map(|tail| replay_start_index(&messages, tail))
        .unwrap_or(0);
    let messages = &messages[skipped..];

    let mut replay_tool_requests = HashMap::new();

    for message in messages {
        for content_item in &message.content {
            match content_item {
                MessageContent::Text(text) => {
                    let mut tc = TextContent::new(text.text.clone());
                    if let Some(audience) =
                        text.annotations.as_ref().and_then(|a| a.audience.as_ref())
                    {
                        tc = tc.annotations(replay_audience_annotations(audience));
                    }
                    send_replay_content_chunk(cx, &session_id, message, ContentBlock::Text(tc))?;
                }
                MessageContent::Image(image) => {
                    let mut image_content =
                        ImageContent::new(image.data.clone(), image.mime_type.clone());
                    if let Some(audience) =
                        image.annotations.as_ref().and_then(|a| a.audience.as_ref())
                    {
                        image_content =
                            image_content.annotations(replay_audience_annotations(audience));
                    }
                    send_replay_content_chunk(
                        cx,
                        &session_id,
                        message,
                        ContentBlock::Image(image_content),
                    )?;
                }
                MessageContent::ToolRequest(tool_request) => {
                    replay_tool_requests.insert(tool_request.id.clone(), tool_request.clone());

                    let tool_call = build_replayed_tool_call(
                        tool_request,
                        message,
                        client_requests_tool_call_label_enrichment,
                    );

                    tool_call_notifier.send_initial(tool_call)?;
                }
                MessageContent::ToolResponse(tool_response) => {
                    let fields = tool_call_update_fields_from_response(
                        tool_response,
                        replay_tool_requests.get(&tool_response.id),
                        true,
                    );
                    let meta = trusted_update_meta(tool_response).unwrap_or_default();

                    let update =
                        ToolCallUpdate::new(ToolCallId::new(tool_response.id.clone()), fields)
                            .meta(merge_message_meta(meta, message));
                    tool_call_notifier.send_update(update)?;
                }
                MessageContent::Thinking(thinking) => {
                    cx.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentThoughtChunk(content_chunk_for_message(
                            message,
                            ContentBlock::Text(TextContent::new(thinking.thinking.clone())),
                        )),
                    ))?;
                }
                MessageContent::Error(error) => {
                    send_replay_content_chunk(
                        cx,
                        &session_id,
                        message,
                        ContentBlock::Text(TextContent::new(error.message.clone())),
                    )?;
                }
                MessageContent::SystemNotification(_) => {}
                _ => {}
            }
        }

        if supports_goose_custom_notifications {
            if let Some(usage) = &message.metadata.usage {
                cx.send_notification(GooseSessionNotification {
                    session_id: session.id.clone(),
                    update: GooseSessionUpdate::MessageUsage(message_usage_update(
                        message.id.clone(),
                        usage,
                    )),
                })?;
            }
        }
    }

    Ok(skipped)
}

impl GooseAcpAgent {
    fn resend_pending_tool_permissions(
        &self,
        cx: &ConnectionTo<Client>,
        agent: &Arc<Agent>,
        session_id: &str,
        requests: &[ToolConfirmationRequest],
        cancel_token: Option<CancellationToken>,
    ) -> Result<(), agent_client_protocol::Error> {
        let acp_session_id = SessionId::new(session_id.to_string());
        for request in requests {
            self.handle_tool_permission_request(
                cx,
                &acp_session_id,
                PendingToolPermission {
                    request_id: request.id.clone(),
                    tool_name: request.tool_name.clone(),
                    arguments: request.arguments.clone(),
                    prompt: request.prompt.clone(),
                },
                SessionAgentTarget {
                    agent: agent.clone(),
                    session_id: session_id.to_string(),
                    cancel_token: cancel_token.clone(),
                },
            )?;
        }

        Ok(())
    }

    async fn start_resumed_state_machine_turn(
        self: &Arc<Self>,
        cx: &ConnectionTo<Client>,
        agent: &Arc<Agent>,
        session_id: &str,
        requests: &[ToolConfirmationRequest],
    ) -> Result<(), agent_client_protocol::Error> {
        let run_id = format!("resume_{}", Uuid::new_v4());
        let cancel_token = CancellationToken::new();
        self.start_active_run(
            session_id,
            run_id.clone(),
            cancel_token.clone(),
            agent.clone(),
        )
        .await?;

        let acp_session_id = SessionId::new(session_id.to_string());
        if let Err(error) = Self::send_active_run_update(cx, &acp_session_id, Some(&run_id)) {
            self.clear_active_run(session_id, &run_id).await;
            return Err(error);
        }

        let session_config = SessionConfig {
            id: session_id.to_string(),
            schedule_id: None,
            max_turns: None,
            retry_config: None,
        };
        let stream = match agent
            .resume_state_machine_turn(session_config, cancel_token.clone())
            .await
        {
            Ok(Some(stream)) => stream,
            Ok(None) => {
                self.clear_active_run(session_id, &run_id).await;
                Self::send_active_run_update(cx, &acp_session_id, None)?;
                return Ok(());
            }
            Err(error) => {
                self.clear_active_run(session_id, &run_id).await;
                let _ = Self::send_active_run_update(cx, &acp_session_id, None);
                return Err(agent_client_protocol::Error::internal_error().data(format!(
                    "Failed to resume pending tool confirmation: {error}"
                )));
            }
        };

        let server = Arc::clone(self);
        let task_cx = cx.clone();
        let task_agent = agent.clone();
        let task_session_id = session_id.to_string();
        let task_run_id = run_id.clone();
        let task_cancel_token = cancel_token.clone();
        let task_acp_session_id = acp_session_id.clone();
        if let Err(error) = cx.spawn(async move {
            let _run_guard = ActiveRunDropGuard {
                registry: server.active_prompt_runs.clone(),
                session_id: task_session_id.clone(),
                run_id: task_run_id.clone(),
                cancel_token: task_cancel_token.clone(),
            };
            let result = server
                .forward_agent_stream(
                    &task_cx,
                    &task_acp_session_id,
                    &task_session_id,
                    &task_agent,
                    &task_cancel_token,
                    stream,
                )
                .await;
            if result.is_ok() {
                if let Err(error) = server
                    .send_session_usage_updates(
                        &task_cx,
                        &task_acp_session_id,
                        &task_session_id,
                        &task_agent,
                    )
                    .await
                {
                    warn!(
                        session_id = task_session_id,
                        ?error,
                        "Failed to update usage after resumed ACP turn"
                    );
                }
            }
            server
                .clear_active_run(&task_session_id, &task_run_id)
                .await;
            if let Err(error) = Self::send_active_run_update(&task_cx, &task_acp_session_id, None) {
                warn!(
                    session_id = task_session_id,
                    ?error,
                    "Failed to clear resumed ACP run status"
                );
            }
            if let Err(error) = result {
                warn!(
                    session_id = task_session_id,
                    ?error,
                    "Resumed ACP state-machine turn failed"
                );
            }
            Ok(())
        }) {
            cancel_token.cancel();
            self.clear_active_run(session_id, &run_id).await;
            let _ = Self::send_active_run_update(cx, &SessionId::new(session_id), None);
            return Err(error);
        }

        if let Err(error) = self.resend_pending_tool_permissions(
            cx,
            agent,
            session_id,
            requests,
            Some(cancel_token.clone()),
        ) {
            cancel_token.cancel();
            self.clear_active_run(session_id, &run_id).await;
            let _ = Self::send_active_run_update(cx, &acp_session_id, None);
            return Err(error);
        }

        Ok(())
    }

    pub(super) async fn handle_load_session(
        self: &Arc<Self>,
        cx: &ConnectionTo<Client>,
        args: LoadSessionRequest,
    ) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
        debug!(?args, "load session request");

        let session_id_str = args.session_id.0.to_string();

        let mut session = self
            .session_manager
            .get_session(&session_id_str, true)
            .await
            .map_err(|_| {
                agent_client_protocol::Error::resource_not_found(Some(session_id_str.clone()))
                    .data(format!("Session not found: {}", session_id_str))
            })?;

        let cwd = effective_session_cwd(self.session_cwd.as_deref(), &args.cwd);
        validate_absolute_cwd(&cwd)?;

        session = self
            .prepare_session_for_activation(session, cwd, args.mcp_servers, true)
            .await?;

        let replayed_from = replay_conversation_to_client(
            cx,
            &session,
            self.supports_goose_custom_notifications(),
            self.requests_tool_call_label_enrichment(),
            replay_tail_from_meta(args.meta.as_ref()),
        )?;
        let (agent, extension_results) = self.prepare_acp_session_agent(cx, &session).await?;
        self.apply_session_recipe(&agent, &session).await?;
        self.register_acp_session(session_id_str.clone(), agent.clone())
            .await;
        let provider = agent
            .provider()
            .await
            .internal_err_ctx("Failed to get provider while loading ACP session")?;
        resume_saved_provider_session(&provider, session.conversation.as_ref()).await;
        session = self
            .session_manager
            .get_session(&session_id_str, false)
            .await
            .internal_err_ctx("Failed to reload session")?;

        agent
            .extension_manager
            .update_working_dir(&session.working_dir)
            .await;

        let (mode_state, config_options) = build_session_setup_config(
            &self.provider_inventory,
            &session,
            &agent_thinking_effort_support(&agent).await,
        )
        .await?;

        let mut response = LoadSessionResponse::new().modes(mode_state);
        if let Some(co) = config_options {
            response = response.config_options(co);
        }

        let mut meta = session_response_meta(&session, &extension_results);
        if replayed_from > 0 {
            meta.insert(
                "replaySkipped".to_string(),
                serde_json::Value::Number(replayed_from.into()),
            );
        }
        response = response.meta(meta);

        let pending_confirmations = session
            .conversation
            .as_ref()
            .map(pending_tool_confirmations)
            .unwrap_or_default();
        let should_resume_state_machine = crate::agents::state_machine::enabled()
            && (!pending_confirmations.is_empty()
                || session
                    .conversation
                    .as_ref()
                    .is_some_and(has_unapplied_tool_confirmation_response));
        if should_resume_state_machine {
            self.start_resumed_state_machine_turn(
                cx,
                &agent,
                &session_id_str,
                &pending_confirmations,
            )
            .await?;
        } else {
            self.resend_pending_tool_permissions(
                cx,
                &agent,
                &session_id_str,
                &pending_confirmations,
                None,
            )?;
        }

        self.closed_session_ids.lock().await.remove(&session_id_str);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::InferenceMetadata;
    use goose_providers::thinking::{
        ThinkingEffortCapability, ThinkingEffortOption, ThinkingEffortSupport,
    };
    use rmcp::model::CallToolRequestParams;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Debug)]
    struct ResumeEffortProvider {
        resumed: AtomicBool,
    }

    #[async_trait::async_trait]
    impl Provider for ResumeEffortProvider {
        fn get_name(&self) -> &str {
            "claude-acp"
        }

        async fn resume(&self, session_id: &str) -> std::result::Result<(), ProviderError> {
            assert_eq!(session_id, "saved-inner-session");
            self.resumed.store(true, Ordering::Release);
            Ok(())
        }

        fn thinking_effort_support(&self) -> ThinkingEffortSupport {
            let value = if self.resumed.load(Ordering::Acquire) {
                "high"
            } else {
                "low"
            };
            ThinkingEffortSupport::Options(ThinkingEffortCapability {
                option_id: "effort".to_string(),
                values: vec![ThinkingEffortOption {
                    value: value.to_string(),
                    label: value.to_string(),
                }],
                current: Some(value.to_string()),
            })
        }

        async fn stream(
            &self,
            _model_config: &goose_providers::model::ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[rmcp::model::Tool],
        ) -> std::result::Result<crate::providers::base::MessageStream, ProviderError> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[tokio::test]
    async fn saved_provider_session_is_resumed_before_effort_snapshot() {
        let provider = Arc::new(ResumeEffortProvider {
            resumed: AtomicBool::new(false),
        });
        let conversation = Conversation::new_unvalidated([Message::assistant().with_inference(
            InferenceMetadata {
                provider: "claude-acp".to_string(),
                requested_model: "current".to_string(),
                resolved_model: None,
                provider_session_id: Some("saved-inner-session".to_string()),
            },
        )]);

        let provider_dyn: Arc<dyn Provider> = provider.clone();
        resume_saved_provider_session(&provider_dyn, Some(&conversation)).await;

        let ThinkingEffortSupport::Options(capability) = provider.thinking_effort_support() else {
            panic!("expected resumed effort capability");
        };
        assert_eq!(capability.current.as_deref(), Some("high"));
    }

    #[test]
    fn replay_start_index_short_history_replays_everything() {
        let messages = vec![
            Message::user().with_text("q1"),
            Message::assistant().with_text("a1"),
        ];
        assert_eq!(replay_start_index(&messages, 10), 0);
        assert_eq!(replay_start_index(&messages, 2), 0);
    }

    #[test]
    fn replay_start_index_zero_tail_replays_everything() {
        let messages = vec![
            Message::user().with_text("q1"),
            Message::assistant().with_text("a1"),
        ];
        assert_eq!(replay_start_index(&messages, 0), 0);
    }

    #[test]
    fn replay_start_index_starts_at_turn_boundary() {
        let messages = vec![
            Message::user().with_text("q1"),
            Message::assistant().with_text("a1"),
            Message::user().with_text("q2"),
            Message::assistant().with_text("a2"),
            Message::user().with_text("q3"),
            Message::assistant().with_text("a3"),
        ];
        // tail=3 → candidate index 3 (a2); nearest user boundary at or before is q2 (index 2)
        assert_eq!(replay_start_index(&messages, 3), 2);
        // tail=1 → candidate index 5 (a3); boundary is q3 (index 4)
        assert_eq!(replay_start_index(&messages, 1), 4);
    }

    #[test]
    fn replay_start_index_never_splits_tool_call_pairs() {
        let tool_request = Message::assistant()
            .with_tool_request("tool_1", Ok(CallToolRequestParams::new("developer__shell")));
        let tool_response = Message::user()
            .with_tool_response("tool_1", Ok(rmcp::model::CallToolResult::success(vec![])));
        let messages = vec![
            Message::user().with_text("q1"),
            tool_request,
            tool_response,
            Message::assistant().with_text("a1"),
        ];
        // tail=2 → candidate is the tool response; it is not a turn boundary,
        // so we walk back to q1 (index 0) rather than splitting the pair.
        assert_eq!(replay_start_index(&messages, 2), 0);
    }

    #[test]
    fn replay_start_index_no_boundary_replays_everything() {
        let messages = vec![
            Message::assistant().with_text("a1"),
            Message::assistant().with_text("a2"),
            Message::assistant().with_text("a3"),
        ];
        assert_eq!(replay_start_index(&messages, 1), 0);
    }

    #[test]
    fn acp_replay_populates_only_empty_marked_assistant_messages() {
        let visible_message = Message::assistant()
            .with_text("visible")
            .with_id("msg_visible");
        let empty_message = Message::assistant().with_id("msg_empty");

        let mut marked_message = Message::assistant().with_id("msg_limited");
        marked_message.metadata.output_token_limit_reached = true;

        let mut marked_user_message = Message::user().with_id("msg_user");
        marked_user_message.metadata.output_token_limit_reached = true;

        let mut hidden_marked_message = Message::assistant()
            .with_id("msg_hidden")
            .with_visibility(false, false);
        hidden_marked_message.metadata.output_token_limit_reached = true;

        let conversation = Conversation::new_unvalidated([
            visible_message,
            empty_message,
            marked_message,
            marked_user_message,
            hidden_marked_message,
        ]);

        let messages = messages_for_acp_replay(&conversation);

        assert_eq!(
            messages
                .iter()
                .filter_map(|message| message.id.as_deref())
                .collect::<Vec<_>>(),
            vec!["msg_visible", "msg_limited"]
        );
        assert_eq!(
            messages[1].as_concat_text(),
            "Response stopped because the model reached its output-token limit."
        );
        assert!(messages[1].metadata.output_token_limit_reached);
        assert!(conversation.messages()[2].content.is_empty());
    }

    fn persisted_enriched_tool_request() -> ToolRequest {
        ToolRequest {
            id: "req_first".to_string(),
            tool_call: Ok(CallToolRequestParams::new("developer__shell")),
            metadata: None,
            tool_meta: Some(serde_json::json!({
                (crate::conversation::message::TOOL_META_TITLE_KEY): "applied dark mode polish",
                (crate::conversation::message::TOOL_META_CHAIN_SUMMARY_KEY): {
                    "summary": "applied dark mode polish",
                    "count": 3,
                },
            })),
        }
    }

    #[test]
    fn replay_includes_persisted_enrichment_when_requested() {
        let mut message =
            Message::new(Role::Assistant, 1_700_000_000, vec![]).with_id("msg_replay");
        message.metadata.output_token_limit_reached = true;
        let tool_call =
            build_replayed_tool_call(&persisted_enriched_tool_request(), &message, true);
        let goose = tool_call
            .meta
            .as_ref()
            .and_then(|meta| meta.get("goose"))
            .expect("valid initial tool call should contain goose metadata");

        assert_eq!(tool_call.title, "applied dark mode polish");
        assert_eq!(
            goose,
            &serde_json::json!({
                "created": 1_700_000_000,
                "messageId": "msg_replay",
                "outputTokenLimitReached": true,
                "toolCall": {
                    "toolName": "developer__shell",
                    "extensionName": "developer",
                },
                "toolChainSummary": {
                    "summary": "applied dark mode polish",
                    "count": 3,
                },
            }),
        );
    }

    #[test]
    fn replay_omits_persisted_enrichment_when_not_requested() {
        let message = Message::new(Role::Assistant, 1_700_000_000, vec![]).with_id("msg_replay");
        let tool_call =
            build_replayed_tool_call(&persisted_enriched_tool_request(), &message, false);
        let goose = tool_call
            .meta
            .as_ref()
            .and_then(|meta| meta.get("goose"))
            .expect("valid initial tool call should contain goose metadata");

        assert_eq!(tool_call.title, "developer: shell");
        assert_eq!(
            goose,
            &serde_json::json!({
                "created": 1_700_000_000,
                "messageId": "msg_replay",
                "toolCall": {
                    "toolName": "developer__shell",
                    "extensionName": "developer",
                },
            }),
        );
    }

    #[test]
    fn pending_permissions_are_limited_to_the_active_turn() {
        let approval = |id: &str| {
            Message::assistant().with_action_required(
                id,
                "tool".to_string(),
                Default::default(),
                None,
            )
        };
        let conversation = Conversation::new_unvalidated([
            Message::user().with_text("old turn"),
            approval("old"),
            Message::user().with_text("current turn"),
            approval("current"),
        ]);

        let approval_ids = pending_tool_confirmations(&conversation)
            .into_iter()
            .map(|request| request.id)
            .collect::<Vec<_>>();
        assert_eq!(approval_ids, ["current"]);

        let no_kickoff = Conversation::new_unvalidated([approval("orphan")]);
        assert_eq!(pending_tool_confirmations(&no_kickoff).len(), 1);
    }
}

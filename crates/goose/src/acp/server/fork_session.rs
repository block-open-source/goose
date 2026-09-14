use super::*;

impl GooseAcpAgent {
    #[allow(dead_code)]
    pub(super) async fn handle_fork_session(
        &self,
        cx: &ConnectionTo<Client>,
        args: ForkSessionRequest,
    ) -> Result<ForkSessionResponse, agent_client_protocol::Error> {
        let conversation_before = conversation_before_from_meta(args.meta.as_ref())?;
        let source_session_id = &*args.session_id.0;

        let source = self
            .session_manager
            .get_session(source_session_id, false)
            .await
            .internal_err()?;

        // Resolve and validate the effective cwd before copying anything, so a
        // bad request cannot leave a stray "(copy)" session in the store.
        let cwd = effective_session_cwd(self.session_cwd.as_deref(), &args.cwd);
        validate_absolute_cwd(&cwd)?;

        let fork_name = if source.name.trim().is_empty() {
            "(copy)".to_string()
        } else {
            format!("{} (copy)", source.name)
        };

        let new_session = self
            .session_manager
            .copy_session(source_session_id, fork_name)
            .await
            .internal_err()?;
        let new_session_id = new_session.id.clone();

        if let Some(conversation_before) = conversation_before {
            self.session_manager
                .truncate_conversation(&new_session_id, conversation_before)
                .await
                .internal_err()?;
        }

        let new_session = self
            .session_manager
            .get_session(&new_session_id, true)
            .await
            .internal_err()?;

        let goose_session = self
            .prepare_session_for_activation(new_session.clone(), cwd, args.mcp_servers, true)
            .await?;

        let (agent, extension_results) = self.prepare_acp_session_agent(cx, &goose_session).await?;
        self.apply_session_recipe(&agent, &goose_session).await?;
        self.register_acp_session(goose_session.id.clone(), agent.clone())
            .await;
        let provider = agent
            .provider()
            .await
            .internal_err_ctx("Failed to get provider while forking ACP session")?;
        resume_saved_provider_session(&provider, goose_session.conversation.as_ref()).await;
        let effort_support = agent_thinking_effort_support(&agent).await;

        let acp_session_id = SessionId::new(new_session_id.clone());
        let mut meta = session_meta(&goose_session);
        if let Ok(v) = serde_json::to_value(&extension_results) {
            meta.insert("extensionResults".to_string(), v);
        }

        let (mode_state, config_options) =
            build_session_setup_config(&self.provider_inventory, &goose_session, &effort_support)
                .await?;

        let mut response = ForkSessionResponse::new(acp_session_id.clone())
            .modes(mode_state)
            .meta(meta);

        if let Some(co) = config_options {
            response = response.config_options(co);
        }
        Ok(response)
    }
}

fn conversation_before_from_meta(
    meta: Option<&Meta>,
) -> Result<Option<i64>, agent_client_protocol::Error> {
    let Some(value) = meta.and_then(|meta| meta.get("conversationBefore")) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    value.as_i64().map(Some).ok_or_else(|| {
        agent_client_protocol::Error::invalid_params()
            .data("conversationBefore must be an integer timestamp")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_with_conversation_before(value: serde_json::Value) -> Meta {
        let mut meta = Meta::new();
        meta.insert("conversationBefore".to_string(), value);
        meta
    }

    #[test]
    fn conversation_before_from_meta_returns_none_when_absent() {
        assert_eq!(conversation_before_from_meta(None).unwrap(), None);
        assert_eq!(
            conversation_before_from_meta(Some(&Meta::new())).unwrap(),
            None
        );
    }

    #[test]
    fn conversation_before_from_meta_treats_null_as_absent() {
        let meta = meta_with_conversation_before(serde_json::Value::Null);

        assert_eq!(conversation_before_from_meta(Some(&meta)).unwrap(), None);
    }

    #[test]
    fn conversation_before_from_meta_reads_integer_timestamp() {
        let meta = meta_with_conversation_before(serde_json::json!(1_718_000_000));

        assert_eq!(
            conversation_before_from_meta(Some(&meta)).unwrap(),
            Some(1_718_000_000)
        );
    }

    #[test]
    fn conversation_before_from_meta_rejects_non_integer_timestamp() {
        for value in [
            serde_json::json!("1718000000"),
            serde_json::json!(1718000000.5),
            serde_json::json!(true),
            serde_json::json!({ "created": 1718000000 }),
        ] {
            assert!(
                conversation_before_from_meta(Some(&meta_with_conversation_before(value))).is_err()
            );
        }
    }
}

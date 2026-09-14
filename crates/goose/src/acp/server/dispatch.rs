use super::*;
use crate::providers::inventory::ensure_refresh_identity_current;

impl HandleDispatchFrom<Client> for GooseAcpHandler {
    fn describe_chain(&self) -> impl std::fmt::Debug {
        "goose-acp"
    }

    fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<Client>,
    ) -> impl std::future::Future<Output = Result<Handled<Dispatch>, agent_client_protocol::Error>> + Send
    {
        let agent = self.agent.clone();

        // The MatchDispatchFrom chain produces an ~85KB async state machine.
        // Box::pin moves it to the heap so it doesn't overflow the tokio worker stack.
        Box::pin(async move {
            // Capture the connection handle so handlers can lazily activate
            // sessions that exist on disk but were never activated via
            // new_session/load_session on this connection. Set-once per
            // connection; the result is ignored on later requests.
            let _ = agent.client_cx.set(cx.clone());
            agent.start_thinking_effort_update_forwarder(&cx).await;

            // InitializeRequest runs inline: it sets connection-scoped state
            // (client fs/terminal capabilities) that later handlers read with
            // defaults, so a pipelined NewSessionRequest must not race ahead of it.
            MatchDispatchFrom::new(message, &cx)
                .if_request(
                    |req: InitializeRequest, responder: Responder<InitializeResponse>| async {
                        responder.respond_with_result(agent.on_initialize(req).await)
                    },
                )
                .await
                .if_request(
                    |_req: AuthenticateRequest, responder: Responder<AuthenticateResponse>| async {
                        responder.respond(AuthenticateResponse::new())
                    },
                )
                .await
                .if_request(
                    |req: NewSessionRequest, responder: Responder<NewSessionResponse>| async {
                        let agent = agent.clone();
                        let cx_clone = cx.clone();
                        cx.spawn(async move {
                            match agent.on_new_session(&cx_clone, req).await {
                                Ok(response) => {
                                    let session_id = response.session_id.0.to_string();
                                    responder.respond(response)?;
                                    let session_setup =
                                        agent.prepare_session_setup_by_id(&session_id).await;
                                    if let Err(error) = session_setup.and_then(|(session, totals, context_limit)| {
                                        send_session_setup_notifications(
                                            &cx_clone,
                                            &session,
                                            &totals,
                                            context_limit,
                                            agent.supports_goose_custom_notifications(),
                                        )
                                    }) {
                                        tracing::warn!(
                                            session_id = %session_id,
                                            error = ?error,
                                            "Failed to send ACP session setup notifications"
                                        );
                                    }
                                }
                                Err(error) => {
                                    responder.respond_with_error(error)?;
                                }
                            }
                            Ok(())
                        })?;
                        Ok(())
                    },
                )
                .await
                .if_request(
                    |req: LoadSessionRequest, responder: Responder<LoadSessionResponse>| async {
                        let agent = agent.clone();
                        let cx_clone = cx.clone();
                        cx.spawn(async move {
                            let session_id = req.session_id.0.to_string();
                            match agent.on_load_session(&cx_clone, req).await {
                                Ok(response) => {
                                    responder.respond(response)?;
                                    let session_setup =
                                        agent.prepare_session_setup_by_id(&session_id).await;
                                    if let Err(error) = session_setup.and_then(
                                        |(session, totals, context_limit)| {
                                            send_session_setup_notifications(
                                                &cx_clone,
                                                &session,
                                                &totals,
                                                context_limit,
                                                agent.supports_goose_custom_notifications(),
                                            )
                                        },
                                    ) {
                                        tracing::warn!(
                                            session_id = %session_id,
                                            error = ?error,
                                            "Failed to send ACP session setup notifications"
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        session_id = %session_id,
                                        error = ?e,
                                        "ACP load_session failed"
                                    );
                                    responder.respond_with_error(e)?;
                                }
                            }
                            Ok(())
                        })?;
                        Ok(())
                    },
                )
                .await
                .if_request(
                    |req: PromptRequest, responder: Responder<PromptResponse>| async {
                        let agent = agent.clone();
                        let cx_clone = cx.clone();
                        cx.spawn(async move {
                            match agent.on_prompt(&cx_clone, req).await {
                                Ok(response) => {
                                    responder.respond(response)?;
                                }
                                Err(e) => {
                                    responder.respond_with_error(e)?;
                                }
                            }
                            Ok(())
                        })?;
                        Ok(())
                    },
                )
                .await
                .if_notification(|notif: CancelNotification| async {
                    let agent = agent.clone();
                    agent.on_cancel(notif).await?;
                    Ok(())
                })
                .await
                // set_config_option (SACP 11) and set_mode; custom _goose/* in otherwise.
                .if_request({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |req: SetSessionConfigOptionRequest, responder: Responder<SetSessionConfigOptionResponse>| async move {
                        let cx_spawn = cx.clone();
                        cx.spawn(async move {
                            let cx = cx_spawn;
                            let value_id = match req.value.as_value_id() {
                                Some(value_id) => value_id.clone(),
                                None => {
                                    responder.respond_with_error(
                                        agent_client_protocol::Error::invalid_params().data("Expected a value ID")
                                    )?;
                                    return Ok(());
                                }
                            };
                            let session_id = req.session_id.clone();
                            let config_id = req.config_id.0.to_string();
                            match config_id.as_ref() {
                                "provider" => {
                                    Config::global().invalidate_secrets_cache();
                                    match agent.update_provider(&session_id.0, &value_id.0, None, None, None).await {
                                        Ok(_) => {}
                                        Err(e) => { responder.respond_with_error(e)?; return Ok(()); }
                                    }
                                }
                                "mode" => {
                                    match agent.on_set_mode(&session_id.0, &value_id.0).await {
                                        Ok(_) => {}
                                        Err(e) => { responder.respond_with_error(e)?; return Ok(()); }
                                    }
                                }
                                "model" => {
                                    match agent.on_set_model(&session_id.0, &value_id.0).await {
                                        Ok(_) => {}
                                        Err(e) => { responder.respond_with_error(e)?; return Ok(()); }
                                    }
                                }
                                "thinking_effort" => {
                                    match agent.on_set_thinking_effort(&session_id.0, &value_id.0).await {
                                        Ok(_) => {}
                                        Err(e) => { responder.respond_with_error(e)?; return Ok(()); }
                                    }
                                }
                                other => {
                                    responder.respond_with_error(
                                        agent_client_protocol::Error::invalid_params().data(format!("Unsupported config option: {}", other))
                                    )?;
                                    return Ok(());
                                }
                            }
                            // Respond immediately using the current provider inventory snapshot.
                            let (notification, config_options) = match agent.build_config_update(&session_id).await {
                                Ok(update) => update,
                                Err(e) => {
                                    warn!(
                                        session_id = %session_id.0,
                                        config_id = %config_id,
                                        error = ?e,
                                        "failed to build config update after config change"
                                    );
                                    responder.respond_with_error(e)?;
                                    return Ok(());
                                }
                            };
                            cx.send_notification(notification)?;
                            responder.respond(SetSessionConfigOptionResponse::new(config_options))?;

                            let maybe_refresh = if config_id == "provider" {
                                let provider_id = value_id.0.to_string();
                                agent
                                    .provider_inventory
                                    .plan_refresh_jobs(std::slice::from_ref(&provider_id))
                                    .await
                                    .ok()
                                    .and_then(|plan| {
                                        plan.started
                                            .into_iter()
                                            .find(|job| job.provider_id == provider_id)
                                    })
                            } else {
                                None
                            };
                            if let Some(refresh_job) = maybe_refresh {
                                let agent_bg = agent.clone();
                                let cx_bg = cx.clone();
                                let session_id_bg = session_id.clone();
                                tokio::spawn(async move {
                                    let refresh_identity = refresh_job.identity;
                                    let refresh_provider_id = refresh_job.provider_id;
                                    let mut refresh_guard =
                                        agent_bg.provider_inventory.refresh_guard(&refresh_identity);
                                    let provider_result: Result<Arc<dyn Provider>> =
                                        AssertUnwindSafe(async {
                                            let session_agent =
                                                agent_bg.get_session_agent(&session_id_bg.0).await?;
                                            let provider = session_agent
                                                .provider()
                                                .await
                                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                            let provider_name = provider.get_name().to_string();
                                            if provider_name != refresh_provider_id {
                                                return Err(anyhow::anyhow!(
                                                    "provider changed before inventory refresh completed"
                                                ));
                                            }
                                            Ok(provider)
                                        })
                                        .catch_unwind()
                                .await
                                .map_err(|_| {
                                    anyhow::anyhow!("provider inventory refresh task panicked")
                                })
                                .and_then(|result| result);

                                let fetch_result = match provider_result {
                                    Ok(provider) => {
                                        match ensure_refresh_identity_current(
                                            &refresh_provider_id,
                                            &refresh_identity,
                                        )
                                        .await
                                        {
                                            Ok(()) => match AssertUnwindSafe(
                                                provider.fetch_recommended_models(
                                                    crate::model_config::global_toolshim(),
                                                ),
                                            )
                                            .catch_unwind()
                                            .await
                                            {
                                                Ok(Ok(models)) => Ok(models),
                                                Ok(Err(error)) => {
                                                    Err(anyhow::anyhow!(error.to_string()))
                                                }
                                                Err(_) => Err(anyhow::anyhow!(
                                                    "provider inventory refresh task panicked"
                                                )),
                                            },
                                            Err(error) => Err(error),
                                        }
                                    }
                                    Err(error) => Err(error),
                                };

                                match fetch_result {
                                    Ok(models) => match agent_bg
                                        .provider_inventory
                                        .store_refreshed_models_for_identity(
                                            &refresh_identity,
                                            &models,
                                        )
                                        .await
                                    {
                                        Ok(()) => {
                                            refresh_guard.complete();
                                            match agent_bg.build_config_update(&session_id_bg).await
                                            {
                                                Ok((fresh_notification, _)) => {
                                                    let _ = cx_bg
                                                        .send_notification(fresh_notification);
                                                }
                                                Err(error) => warn!(
                                                    provider = %refresh_provider_id,
                                                    error = %error,
                                                    "failed to build config update after provider inventory refresh"
                                                ),
                                            }
                                        }
                                        Err(error) => warn!(
                                            provider = %refresh_provider_id,
                                            error = %error,
                                            "failed to store refreshed provider inventory after config change"
                                        ),
                                    },
                                    Err(error) => {
                                        let error_message = error.to_string();
                                        match agent_bg
                                            .provider_inventory
                                            .store_refresh_error_for_identity(
                                                &refresh_identity,
                                                error_message.clone(),
                                            )
                                            .await
                                        {
                                            Ok(()) => refresh_guard.complete(),
                                            Err(store_error) => warn!(
                                                provider = %refresh_provider_id,
                                                error = %store_error,
                                                refresh_error = %error_message,
                                                "failed to store provider inventory refresh error after config change"
                                            ),
                                        }
                                        warn!(
                                            provider = %refresh_provider_id,
                                            error = %error_message,
                                            "provider inventory refresh failed after config change"
                                        );
                                    }
                                }
                                });
                            }

                            Ok(())
                        })?;
                        Ok(())
                    }
                })
                .await
                .if_request({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |req: SetSessionModeRequest, responder: Responder<SetSessionModeResponse>| async move {
                        let cx_spawn = cx.clone();
                        cx.spawn(async move {
                            let cx = cx_spawn;
                            let session_id = req.session_id.clone();
                            let mode_id = req.mode_id.clone();
                            match agent.on_set_mode(&session_id.0, &mode_id.0).await {
                                Ok(resp) => {
                                    // Notify before responding so clients see the mode update before block_task unblocks.
                                    cx.send_notification(SessionNotification::new(
                                        session_id,
                                        SessionUpdate::CurrentModeUpdate(
                                            CurrentModeUpdate::new(mode_id),
                                        ),
                                    ))?;
                                    responder.respond(resp)?;
                                }
                                Err(e) => {
                                    responder.respond_with_error(e)?;
                                }
                            }
                            Ok(())
                        })?;
                        Ok(())
                    }
                })
                .await
                .if_request({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |req: ListSessionsRequest, responder: Responder<ListSessionsResponse>| async move {
                        cx.spawn(async move {
                            match agent.on_list_sessions(req).await {
                                Ok(response) => responder.respond(response)?,
                                Err(e) => responder.respond_with_error(e)?,
                            }
                            Ok(())
                        })?;
                        Ok(())
                    }
                })
                .await
                .if_request({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |req: DeleteSessionRequest, responder: Responder<DeleteSessionResponse>| async move {
                        cx.spawn(async move {
                            match agent.on_delete_session(req).await {
                                Ok(response) => responder.respond(response)?,
                                Err(e) => responder.respond_with_error(e)?,
                            }
                            Ok(())
                        })?;
                        Ok(())
                    }
                })
                .await
                .if_request({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |req: CloseSessionRequest, responder: Responder<CloseSessionResponse>| async move {
                        cx.spawn(async move {
                            match agent.on_close_session(&req.session_id.0).await {
                                Ok(response) => responder.respond(response)?,
                                Err(e) => responder.respond_with_error(e)?,
                            }
                            Ok(())
                        })?;
                        Ok(())
                    }
                })
                .await
                .if_request({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |req: ForkSessionRequest, responder: Responder<ForkSessionResponse>| async move {
                        let cx_spawn = cx.clone();
                        cx.spawn(async move {
                            match agent.on_fork_session(&cx_spawn, req).await {
                                Ok(response) => {
                                    let session_id = response.session_id.0.to_string();
                                    responder.respond(response)?;
                                    let session_setup =
                                        agent.prepare_session_setup_by_id(&session_id).await;
                                    if let Err(error) = session_setup.and_then(|(session, totals, context_limit)| {
                                        send_session_setup_notifications(
                                            &cx_spawn,
                                            &session,
                                            &totals,
                                            context_limit,
                                            agent.supports_goose_custom_notifications(),
                                        )
                                    }) {
                                        tracing::warn!(
                                            session_id = %session_id,
                                            error = ?error,
                                            "Failed to send ACP forked session setup notifications"
                                        );
                                    }
                                }
                                Err(error) => {
                                    responder.respond_with_error(error)?;
                                }
                            }
                            Ok(())
                        })?;
                        Ok(())
                    }
                })
                .await
                .otherwise({
                    let agent = agent.clone();
                    let cx = cx.clone();
                    |message: Dispatch| async move {
                        match message {
                            Dispatch::Request(req, responder) => {
                                cx.spawn(async move {
                                    match agent.dispatch_custom_request(&req.method, req.params).await {
                                        Ok(json) => responder.respond(json)?,
                                        Err(e) => responder.respond_with_error(e)?,
                                    }
                                    Ok(())
                                })?;
                                Ok(())
                            }
                            Dispatch::Response(result, router) => {
                                debug!(method = %router.method(), id = %router.id(), ok = result.is_ok(), "routing response");
                                router.route_with_result(result)?;
                                Ok(())
                            }
                            Dispatch::Notification(notif) => {
                                debug!(method = %notif.method, "unhandled notification");
                                Ok(())
                            }
                        }
                    }
                })
                .await
                .map(|()| Handled::Yes)
        })
    }
}

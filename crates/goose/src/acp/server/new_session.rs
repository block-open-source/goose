use crate::acp::custom_requests::GooseExtension;
use crate::acp::server::{meta_string, validate_absolute_cwd, ResultExt};
use crate::agents::ExtensionLoadResult;
use crate::config::{Config, GooseMode};
use crate::recipe::{Recipe, Settings};
use crate::session::{ExtensionData, Session, SessionType};

use super::GooseAcpAgent;
use agent_client_protocol::schema::v1::{Meta, NewSessionRequest, NewSessionResponse, SessionId};
use agent_client_protocol::{Client, ConnectionTo};
use goose_providers::model::ModelConfig;
use goose_providers::thinking::ThinkingEffortSupport;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::warn;

struct InitialSessionConfig {
    provider: String,
    model_config: ModelConfig,
    extension_data: ExtensionData,
    recipe: Option<Recipe>,
    user_recipe_values: Option<HashMap<String, String>>,
    meta: NewSessionMetaFields,
}

/// Session fields read from `_meta` on `session/new` that are applied to the
/// session row after it is created.
struct NewSessionMetaFields {
    project_id: Option<String>,
    /// Client-supplied title, recorded as user-set so goose's own name
    /// generation leaves it alone. `None` when a recipe title took precedence.
    client_title: Option<String>,
}

impl GooseAcpAgent {
    pub(super) async fn handle_new_session(
        &self,
        cx: &ConnectionTo<Client>,
        mut args: NewSessionRequest,
    ) -> Result<NewSessionResponse, agent_client_protocol::Error> {
        // When the host imposes a working directory (e.g. roaming, where the
        // connector's absolute path is meaningless on this machine), ignore the
        // cwd the client sent and use the host-controlled one instead.
        if let Some(host_cwd) = &self.session_cwd {
            args.cwd = host_cwd.clone();
        }
        validate_absolute_cwd(&args.cwd)?;
        let config = Config::global();
        let session_type = session_type_from_meta(args.meta.as_ref())?;
        let current_mode: GooseMode = config.get_goose_mode().unwrap_or_default();
        let recipe = self.resolve_recipe_from_meta(args.meta.as_ref()).await?;
        let meta = new_session_meta_fields(args.meta.as_ref(), recipe.as_ref())?;
        let session_name = recipe_title(recipe.as_ref())
            .map(str::to_string)
            .or_else(|| meta.client_title.clone())
            .unwrap_or_else(|| "New Chat".to_string());

        let session = self
            .session_manager
            .create_session(args.cwd.clone(), session_name, session_type, current_mode)
            .await
            .internal_err_ctx("Failed to create session")?;
        match self
            .finish_new_session_setup(cx, config, &session, args, recipe, meta)
            .await
        {
            Ok(response) => Ok(response),
            Err(error) => {
                self.cleanup_failed_new_session(&session.id).await;
                Err(error)
            }
        }
    }

    async fn finish_new_session_setup(
        &self,
        cx: &ConnectionTo<Client>,
        config: &Config,
        session: &Session,
        args: NewSessionRequest,
        recipe: Option<(Recipe, PathBuf)>,
        meta: NewSessionMetaFields,
    ) -> Result<NewSessionResponse, agent_client_protocol::Error> {
        let rendered_recipe = self
            .configure_new_session(cx, config, session, args, recipe, meta)
            .await?;

        let reloaded_session = self.reload_session(&session.id).await?;
        let (agent, extension_results) = self.activate_acp_session(cx, &reloaded_session).await?;
        if let Some(recipe) = &rendered_recipe {
            self.apply_recipe(&agent, recipe).await?;
        }

        let reloaded_session = self.reload_session(&session.id).await?;
        let response = self
            .build_new_session_response(
                &reloaded_session,
                &extension_results,
                &super::agent_thinking_effort_support(&agent).await,
            )
            .await?;
        Ok(response)
    }

    async fn cleanup_failed_new_session(&self, session_id: &str) {
        if let Err(error) = self.session_manager.delete_session(session_id).await {
            warn!(
                session_id,
                %error,
                "Failed to delete session during new-session cleanup"
            );
        }
        self.sessions.lock().await.remove(session_id);
        if let Err(error) = self
            .agent_manager
            .remove_session_if_loaded(session_id)
            .await
        {
            warn!(
                session_id,
                %error,
                "Failed to remove in-memory agent during new-session cleanup"
            );
        }
    }

    async fn configure_new_session(
        &self,
        cx: &ConnectionTo<Client>,
        config: &Config,
        session: &Session,
        args: NewSessionRequest,
        recipe: Option<(Recipe, PathBuf)>,
        meta: NewSessionMetaFields,
    ) -> Result<Option<Recipe>, agent_client_protocol::Error> {
        let recipe_parameter_scope_id = meta_string(args.meta.as_ref(), "recipeParameterScopeId")?;
        let (rendered, user_recipe_values) = self
            .render_recipe_for_session(
                cx,
                &session.id,
                recipe.as_ref(),
                recipe_parameter_scope_id.as_deref(),
            )
            .await?;

        let recipe_settings = rendered.as_ref().and_then(|r| r.settings.as_ref());
        let (provider, model_config) = self
            .resolve_provider_and_model(config, args.meta.as_ref(), recipe_settings)
            .await?;

        let goose_extensions = meta_goose_extensions(args.meta.as_ref())?;
        let recipe_extensions = rendered.as_ref().and_then(|r| r.extensions.as_deref());
        let extension_data = self.build_enabled_extensions_data(
            config,
            session,
            args.mcp_servers,
            goose_extensions,
            recipe_extensions,
        )?;

        self.apply_initial_session_config(
            &session.id,
            InitialSessionConfig {
                provider,
                model_config,
                extension_data,
                recipe: recipe.map(|(recipe, _)| recipe),
                user_recipe_values,
                meta,
            },
        )
        .await?;

        Ok(rendered)
    }

    async fn reload_session(
        &self,
        session_id: &str,
    ) -> Result<Session, agent_client_protocol::Error> {
        self.session_manager
            .get_session(session_id, false)
            .await
            .internal_err_ctx("Failed to reload session")
    }

    async fn resolve_provider_and_model(
        &self,
        config: &Config,
        meta: Option<&Meta>,
        recipe_settings: Option<&Settings>,
    ) -> Result<(String, ModelConfig), agent_client_protocol::Error> {
        let recipe_provider = recipe_settings.and_then(|s| s.goose_provider.clone());
        let recipe_model = recipe_settings.and_then(|s| s.goose_model.clone());

        let provider = match recipe_provider {
            Some(provider) => provider,
            None => match meta_string(meta, "provider")? {
                Some(provider) => provider,
                None => {
                    if let Some(model) = recipe_model.as_deref() {
                        let provider = config.get_goose_provider().map_err(|error| {
                            agent_client_protocol::Error::internal_error()
                                .data(format!("Failed to resolve provider: {}", error))
                        })?;
                        let model_config = model_config_from_recipe_settings(&provider, model)?;
                        return Ok((provider, model_config));
                    }

                    return super::resolve_default_provider_model_config(config);
                }
            },
        };

        let model_config = match recipe_model {
            Some(model) => model_config_from_recipe_settings(&provider, &model)?,
            None => super::resolve_provider_default_model_config(&provider).await?,
        };

        Ok((provider, model_config))
    }

    async fn apply_initial_session_config(
        &self,
        session_id: &str,
        config: InitialSessionConfig,
    ) -> Result<(), agent_client_protocol::Error> {
        let mut builder = self
            .session_manager
            .update(session_id)
            .provider_name(config.provider)
            .model_config(config.model_config)
            .extension_data(config.extension_data);
        if let Some(recipe) = config.recipe {
            builder = builder.recipe(Some(recipe));
        }
        if config.user_recipe_values.is_some() {
            builder = builder.user_recipe_values(config.user_recipe_values);
        }
        if let Some(project_id) = config.meta.project_id {
            builder = builder.project_id(Some(project_id));
        }
        if let Some(client_title) = config.meta.client_title {
            builder = builder.user_provided_name(client_title);
        }
        builder
            .apply()
            .await
            .internal_err_ctx("Failed to update session")?;
        Ok(())
    }

    async fn build_new_session_response(
        &self,
        session: &Session,
        extension_results: &[ExtensionLoadResult],
        effort_support: &ThinkingEffortSupport,
    ) -> Result<NewSessionResponse, agent_client_protocol::Error> {
        let (mode_state, config_options) =
            super::build_session_setup_config(&self.provider_inventory, session, effort_support)
                .await?;

        let mut response =
            NewSessionResponse::new(SessionId::new(session.id.clone())).modes(mode_state);
        if let Some(co) = config_options {
            response = response.config_options(co);
        }
        response = response.meta(super::session_response_meta(session, extension_results));
        Ok(response)
    }
}

fn model_config_from_recipe_settings(
    provider: &str,
    model: &str,
) -> Result<ModelConfig, agent_client_protocol::Error> {
    crate::model_config::model_config_from_user_config(provider, model)
        .internal_err_ctx("Failed to build model config from recipe settings")
}

fn session_type_from_meta(
    meta: Option<&Meta>,
) -> Result<SessionType, agent_client_protocol::Error> {
    if meta_bool(meta, "hidden")? {
        return Ok(SessionType::Hidden);
    }
    Ok(match meta_string(meta, "client")? {
        Some(_) => SessionType::User,
        None => SessionType::Acp,
    })
}

fn meta_bool(meta: Option<&Meta>, key: &str) -> Result<bool, agent_client_protocol::Error> {
    let Some(value) = meta.and_then(|m| m.get(key)) else {
        return Ok(false);
    };
    if value.is_null() {
        return Ok(false);
    }
    value.as_bool().ok_or_else(|| {
        agent_client_protocol::Error::invalid_params().data(format!("{key} must be a boolean"))
    })
}

fn recipe_title(recipe: Option<&(Recipe, PathBuf)>) -> Option<&str> {
    recipe
        .map(|(recipe, _)| recipe.title.trim())
        .filter(|title| !title.is_empty())
}

fn new_session_meta_fields(
    meta: Option<&Meta>,
    recipe: Option<&(Recipe, PathBuf)>,
) -> Result<NewSessionMetaFields, agent_client_protocol::Error> {
    let session_title = meta_string(meta, "sessionTitle")?
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty());
    Ok(NewSessionMetaFields {
        project_id: meta_string(meta, "projectId")?,
        // A recipe title is a server-side declaration, so it keeps the
        // precedence it has today and a client title only replaces the
        // "New Chat" fallback.
        client_title: session_title.filter(|_| recipe_title(recipe).is_none()),
    })
}

fn meta_goose_extensions(
    meta: Option<&Meta>,
) -> Result<Option<Vec<GooseExtension>>, agent_client_protocol::Error> {
    let Some(value) = meta.and_then(|m| m.get("enabledExtensions")) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|e| {
            agent_client_protocol::Error::invalid_params().data(format!("enabledExtensions: {e}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(value: serde_json::Value) -> Meta {
        match value {
            serde_json::Value::Object(map) => map,
            other => panic!("expected object, got {other}"),
        }
    }

    #[test]
    fn hidden_meta_yields_hidden_session() {
        let meta = meta(json!({ "hidden": true }));
        assert_eq!(
            session_type_from_meta(Some(&meta)).unwrap(),
            SessionType::Hidden
        );
    }

    #[test]
    fn hidden_overrides_client() {
        let meta = meta(json!({ "hidden": true, "client": "desktop" }));
        assert_eq!(
            session_type_from_meta(Some(&meta)).unwrap(),
            SessionType::Hidden
        );
    }

    #[test]
    fn absent_hidden_preserves_acp() {
        assert_eq!(session_type_from_meta(None).unwrap(), SessionType::Acp);
        let meta = meta(json!({ "hidden": false }));
        assert_eq!(
            session_type_from_meta(Some(&meta)).unwrap(),
            SessionType::Acp
        );
    }

    #[test]
    fn non_bool_hidden_is_rejected() {
        let meta = meta(json!({ "hidden": "yes", "client": "desktop" }));
        assert!(session_type_from_meta(Some(&meta)).is_err());
    }

    #[test]
    fn client_meta_yields_user_session() {
        let meta = meta(json!({ "client": "desktop" }));
        assert_eq!(
            session_type_from_meta(Some(&meta)).unwrap(),
            SessionType::User
        );
    }
}

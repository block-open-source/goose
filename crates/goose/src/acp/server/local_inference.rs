use super::*;

#[cfg(not(feature = "local-inference"))]
fn local_inference_unavailable() -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data("Local inference not enabled")
}

impl GooseAcpAgent {
    pub(super) async fn on_local_inference_models_list(
        &self,
        _req: LocalInferenceModelsListRequest,
    ) -> Result<LocalInferenceModelsListResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::list_models()
                .await
                .internal_err()
        }

        #[cfg(not(feature = "local-inference"))]
        Err(local_inference_unavailable())
    }

    pub(super) async fn on_local_inference_model_download(
        &self,
        req: LocalInferenceModelDownloadRequest,
    ) -> Result<LocalInferenceModelDownloadResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::download_model(req)
                .await
                .invalid_params_err()
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_model_download_progress(
        &self,
        req: LocalInferenceModelDownloadProgressRequest,
    ) -> Result<LocalInferenceModelDownloadProgressResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::download_progress(&req.model_id)
                .map(|progress| LocalInferenceModelDownloadProgressResponse { progress })
                .internal_err()
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_model_download_cancel(
        &self,
        req: LocalInferenceModelDownloadCancelRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::cancel_download(&req.model_id)
                .internal_err()?;
            Ok(EmptyResponse {})
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_model_delete(
        &self,
        req: LocalInferenceModelDeleteRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::delete_model(&req.model_id)
                .await
                .invalid_params_err()?;
            Ok(EmptyResponse {})
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_model_evict(
        &self,
        req: LocalInferenceModelEvictRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::evict_model(&req.model_id)
                .await
                .invalid_params_err()?;
            Ok(EmptyResponse {})
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_model_settings_read(
        &self,
        req: LocalInferenceModelSettingsReadRequest,
    ) -> Result<LocalInferenceModelSettingsReadResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::get_model_settings(&req.model_id)
                .invalid_params_err()
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_model_settings_update(
        &self,
        req: LocalInferenceModelSettingsUpdateRequest,
    ) -> Result<LocalInferenceModelSettingsUpdateResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::update_model_settings(
                &req.model_id,
                req.settings,
            )
            .invalid_params_err()
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_huggingface_search(
        &self,
        req: LocalInferenceHuggingFaceSearchRequest,
    ) -> Result<LocalInferenceHuggingFaceSearchResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::search_huggingface_models(
                req.query, req.limit,
            )
            .await
            .internal_err()
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_huggingface_repo_variants(
        &self,
        req: LocalInferenceHuggingFaceRepoVariantsRequest,
    ) -> Result<LocalInferenceHuggingFaceRepoVariantsResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            crate::providers::local_inference::management::huggingface_repo_variants(req.repo_id)
                .await
                .internal_err()
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(local_inference_unavailable())
        }
    }

    pub(super) async fn on_local_inference_builtin_chat_templates_list(
        &self,
        _req: LocalInferenceBuiltinChatTemplatesListRequest,
    ) -> Result<LocalInferenceBuiltinChatTemplatesListResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            crate::providers::local_inference::configure_huggingface_auth();
            Ok(crate::providers::local_inference::management::list_builtin_chat_templates())
        }

        #[cfg(not(feature = "local-inference"))]
        Err(local_inference_unavailable())
    }
}

use super::*;
use goose_acp_macros::custom_methods;

#[custom_methods]
impl GooseAcpAgent {
    pub async fn dispatch_custom_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, agent_client_protocol::Error> {
        let result = async {
            if <SaveRecipeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(method)
            {
                let req = recipe::deserialize_save_recipe_request(params)?;
                let result = self.on_save_recipe(req).await?;
                return serde_json::to_value(&result).map_err(|e| {
                    agent_client_protocol::Error::internal_error().data(e.to_string())
                });
            }

            self.handle_custom_request(method, params).await
        }
        .await;

        if let Err(error) = &result {
            tracing::error!(method, error = ?error, "ACP custom request failed");
        }

        result
    }

    #[custom_method(AddSessionExtensionRequest)]
    async fn dispatch_add_session_extension(
        &self,
        req: AddSessionExtensionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_add_session_extension(req).await
    }

    #[custom_method(RemoveSessionExtensionRequest)]
    async fn dispatch_remove_session_extension(
        &self,
        req: RemoveSessionExtensionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_remove_session_extension(req).await
    }

    #[custom_method(GetToolsRequest)]
    async fn dispatch_get_tools(
        &self,
        req: GetToolsRequest,
    ) -> Result<GetToolsResponse, agent_client_protocol::Error> {
        self.on_get_tools(req).await
    }

    #[custom_method(SetToolPermissionsRequest)]
    async fn dispatch_set_tool_permissions(
        &self,
        req: SetToolPermissionsRequest,
    ) -> Result<SetToolPermissionsResponse, agent_client_protocol::Error> {
        self.on_set_tool_permissions(req).await
    }

    #[custom_method(GooseToolCallRequest)]
    async fn dispatch_call_tool(
        &self,
        req: GooseToolCallRequest,
    ) -> Result<GooseToolCallResponse, agent_client_protocol::Error> {
        self.on_call_tool(req).await
    }

    #[custom_method(ReadResourceRequest)]
    async fn dispatch_read_resource(
        &self,
        req: ReadResourceRequest,
    ) -> Result<ReadResourceResponse, agent_client_protocol::Error> {
        self.on_read_resource(req).await
    }

    #[custom_method(AppsListRequest)]
    async fn dispatch_list_apps(
        &self,
        req: AppsListRequest,
    ) -> Result<AppsListResponse, agent_client_protocol::Error> {
        self.on_list_apps(req).await
    }

    #[custom_method(AppsExportRequest)]
    async fn dispatch_export_app(
        &self,
        req: AppsExportRequest,
    ) -> Result<AppsExportResponse, agent_client_protocol::Error> {
        self.on_export_app(req).await
    }

    #[custom_method(AppsImportRequest)]
    async fn dispatch_import_app(
        &self,
        req: AppsImportRequest,
    ) -> Result<AppsImportResponse, agent_client_protocol::Error> {
        self.on_import_app(req).await
    }

    #[custom_method(AppsDeleteRequest)]
    async fn dispatch_delete_app(
        &self,
        req: AppsDeleteRequest,
    ) -> Result<AppsDeleteResponse, agent_client_protocol::Error> {
        self.on_delete_app(req).await
    }

    #[custom_method(UpdateWorkingDirRequest)]
    async fn dispatch_update_working_dir(
        &self,
        req: UpdateWorkingDirRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_update_working_dir(req).await
    }

    #[custom_method(SetSessionSystemPromptRequest)]
    async fn dispatch_set_session_system_prompt(
        &self,
        req: SetSessionSystemPromptRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_set_session_system_prompt(req).await
    }

    #[custom_method(SteerSessionRequest)]
    async fn dispatch_steer_session(
        &self,
        req: SteerSessionRequest,
    ) -> Result<SteerSessionResponse, agent_client_protocol::Error> {
        self.on_steer_session(req).await
    }

    #[custom_method(DiagnosticsGetRequest)]
    async fn dispatch_get_diagnostics(
        &self,
        req: DiagnosticsGetRequest,
    ) -> Result<DiagnosticsGetResponse, agent_client_protocol::Error> {
        self.on_get_diagnostics(req).await
    }

    #[custom_method(ListPromptsRequest)]
    async fn dispatch_list_prompts(
        &self,
        req: ListPromptsRequest,
    ) -> Result<ListPromptsResponse, agent_client_protocol::Error> {
        self.on_list_prompts(req).await
    }

    #[custom_method(GetPromptRequest)]
    async fn dispatch_get_prompt(
        &self,
        req: GetPromptRequest,
    ) -> Result<GetPromptResponse, agent_client_protocol::Error> {
        self.on_get_prompt(req).await
    }

    #[custom_method(SavePromptRequest)]
    async fn dispatch_save_prompt(
        &self,
        req: SavePromptRequest,
    ) -> Result<PromptOperationResponse, agent_client_protocol::Error> {
        self.on_save_prompt(req).await
    }

    #[custom_method(ResetPromptRequest)]
    async fn dispatch_reset_prompt(
        &self,
        req: ResetPromptRequest,
    ) -> Result<PromptOperationResponse, agent_client_protocol::Error> {
        self.on_reset_prompt(req).await
    }

    #[custom_method(GetConfigExtensionsRequest)]
    async fn dispatch_get_config_extensions(
        &self,
    ) -> Result<GetConfigExtensionsResponse, agent_client_protocol::Error> {
        self.on_get_config_extensions().await
    }

    #[custom_method(AddConfigExtensionRequest)]
    async fn dispatch_add_config_extension(
        &self,
        req: AddConfigExtensionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_add_config_extension(req).await
    }

    #[custom_method(RemoveConfigExtensionRequest)]
    async fn dispatch_remove_config_extension(
        &self,
        req: RemoveConfigExtensionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_remove_config_extension(req).await
    }

    #[custom_method(SetConfigExtensionEnabledRequest)]
    async fn dispatch_set_config_extension_enabled(
        &self,
        req: SetConfigExtensionEnabledRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_set_config_extension_enabled(req).await
    }

    #[custom_method(GetSessionExtensionsRequest)]
    async fn dispatch_get_session_extensions(
        &self,
        req: GetSessionExtensionsRequest,
    ) -> Result<GetSessionExtensionsResponse, agent_client_protocol::Error> {
        self.on_get_session_extensions(req).await
    }

    #[custom_method(ListProvidersRequest)]
    async fn dispatch_list_providers(
        &self,
        req: ListProvidersRequest,
    ) -> Result<ListProvidersResponse, agent_client_protocol::Error> {
        self.on_list_providers(req).await
    }

    #[custom_method(ProviderSupportedModelsListRequest)]
    async fn dispatch_list_provider_supported_models(
        &self,
        req: ProviderSupportedModelsListRequest,
    ) -> Result<ProviderSupportedModelsListResponse, agent_client_protocol::Error> {
        self.on_list_provider_supported_models(req).await
    }

    #[custom_method(ProviderCatalogListRequest)]
    async fn dispatch_list_provider_catalog(
        &self,
        req: ProviderCatalogListRequest,
    ) -> Result<ProviderCatalogListResponse, agent_client_protocol::Error> {
        self.on_list_provider_catalog(req).await
    }

    #[custom_method(ProviderSetupCatalogListRequest)]
    async fn dispatch_list_provider_setup_catalog(
        &self,
        req: ProviderSetupCatalogListRequest,
    ) -> Result<ProviderSetupCatalogListResponse, agent_client_protocol::Error> {
        self.on_list_provider_setup_catalog(req).await
    }

    #[custom_method(ProviderCatalogTemplateRequest)]
    async fn dispatch_get_provider_catalog_template(
        &self,
        req: ProviderCatalogTemplateRequest,
    ) -> Result<ProviderCatalogTemplateResponse, agent_client_protocol::Error> {
        self.on_get_provider_catalog_template(req).await
    }

    #[custom_method(CustomProviderCreateRequest)]
    async fn dispatch_create_custom_provider(
        &self,
        req: CustomProviderCreateRequest,
    ) -> Result<CustomProviderCreateResponse, agent_client_protocol::Error> {
        self.on_create_custom_provider(req).await
    }

    #[custom_method(CustomProviderReadRequest)]
    async fn dispatch_read_custom_provider(
        &self,
        req: CustomProviderReadRequest,
    ) -> Result<CustomProviderReadResponse, agent_client_protocol::Error> {
        self.on_read_custom_provider(req).await
    }

    #[custom_method(CustomProviderUpdateRequest)]
    async fn dispatch_update_custom_provider(
        &self,
        req: CustomProviderUpdateRequest,
    ) -> Result<CustomProviderUpdateResponse, agent_client_protocol::Error> {
        self.on_update_custom_provider(req).await
    }

    #[custom_method(CustomProviderDeleteRequest)]
    async fn dispatch_delete_custom_provider(
        &self,
        req: CustomProviderDeleteRequest,
    ) -> Result<CustomProviderDeleteResponse, agent_client_protocol::Error> {
        self.on_delete_custom_provider(req).await
    }

    #[custom_method(RefreshProviderInventoryRequest)]
    async fn dispatch_refresh_provider_inventory(
        &self,
        req: RefreshProviderInventoryRequest,
    ) -> Result<RefreshProviderInventoryResponse, agent_client_protocol::Error> {
        self.on_refresh_provider_inventory(req).await
    }

    #[custom_method(ProviderReadinessCheckRequest)]
    async fn dispatch_check_provider_readiness(
        &self,
        req: ProviderReadinessCheckRequest,
    ) -> Result<ProviderReadinessCheckResponse, agent_client_protocol::Error> {
        self.on_check_provider_readiness(req).await
    }

    #[custom_method(ProviderConfigReadRequest)]
    async fn dispatch_read_provider_config(
        &self,
        req: ProviderConfigReadRequest,
    ) -> Result<ProviderConfigReadResponse, agent_client_protocol::Error> {
        self.on_read_provider_config(req).await
    }

    #[custom_method(ProviderConfigStatusRequest)]
    async fn dispatch_provider_config_status(
        &self,
        req: ProviderConfigStatusRequest,
    ) -> Result<ProviderConfigStatusResponse, agent_client_protocol::Error> {
        self.on_provider_config_status(req).await
    }

    #[custom_method(ProviderConfigSaveRequest)]
    async fn dispatch_save_provider_config(
        &self,
        req: ProviderConfigSaveRequest,
    ) -> Result<ProviderConfigChangeResponse, agent_client_protocol::Error> {
        self.on_save_provider_config(req).await
    }

    #[custom_method(ProviderConfigDeleteRequest)]
    async fn dispatch_delete_provider_config(
        &self,
        req: ProviderConfigDeleteRequest,
    ) -> Result<ProviderConfigChangeResponse, agent_client_protocol::Error> {
        self.on_delete_provider_config(req).await
    }

    #[custom_method(ProviderConfigAuthenticateRequest)]
    async fn dispatch_authenticate_provider_config(
        &self,
        req: ProviderConfigAuthenticateRequest,
    ) -> Result<ProviderConfigChangeResponse, agent_client_protocol::Error> {
        self.on_authenticate_provider_config(req).await
    }

    #[custom_method(ProviderSecretsListRequest)]
    async fn dispatch_list_provider_secrets(
        &self,
        req: ProviderSecretsListRequest,
    ) -> Result<ProviderSecretsListResponse, agent_client_protocol::Error> {
        self.on_list_provider_secrets(req).await
    }

    #[custom_method(ProviderSecretDeleteRequest)]
    async fn dispatch_delete_provider_secret(
        &self,
        req: ProviderSecretDeleteRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_delete_provider_secret(req).await
    }

    #[custom_method(CanonicalModelInfoRequest)]
    async fn dispatch_canonical_model_info(
        &self,
        req: CanonicalModelInfoRequest,
    ) -> Result<CanonicalModelInfoResponse, agent_client_protocol::Error> {
        self.on_canonical_model_info(req).await
    }

    #[custom_method(PreferencesReadRequest)]
    async fn dispatch_preferences_read(
        &self,
        req: PreferencesReadRequest,
    ) -> Result<PreferencesReadResponse, agent_client_protocol::Error> {
        self.on_preferences_read(req).await
    }

    #[custom_method(PreferencesSaveRequest)]
    async fn dispatch_preferences_save(
        &self,
        req: PreferencesSaveRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_preferences_save(req).await
    }

    #[custom_method(ConfigReadRequest)]
    async fn dispatch_config_read(
        &self,
        req: ConfigReadRequest,
    ) -> Result<ConfigReadResponse, agent_client_protocol::Error> {
        self.on_config_read(req).await
    }

    #[custom_method(ConfigUpsertRequest)]
    async fn dispatch_config_upsert(
        &self,
        req: ConfigUpsertRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_config_upsert(req).await
    }

    #[custom_method(ConfigRemoveRequest)]
    async fn dispatch_config_remove(
        &self,
        req: ConfigRemoveRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_config_remove(req).await
    }

    #[custom_method(ConfigReadAllRequest)]
    async fn dispatch_config_read_all(
        &self,
        req: ConfigReadAllRequest,
    ) -> Result<ConfigReadAllResponse, agent_client_protocol::Error> {
        self.on_config_read_all(req).await
    }

    #[custom_method(DefaultsReadRequest)]
    async fn dispatch_defaults_read(
        &self,
        req: DefaultsReadRequest,
    ) -> Result<DefaultsReadResponse, agent_client_protocol::Error> {
        self.on_defaults_read(req).await
    }

    #[custom_method(DefaultsSaveRequest)]
    async fn dispatch_defaults_save(
        &self,
        req: DefaultsSaveRequest,
    ) -> Result<DefaultsReadResponse, agent_client_protocol::Error> {
        self.on_defaults_save(req).await
    }

    #[custom_method(DefaultsClearRequest)]
    async fn dispatch_defaults_clear(
        &self,
        req: DefaultsClearRequest,
    ) -> Result<DefaultsReadResponse, agent_client_protocol::Error> {
        self.on_defaults_clear(req).await
    }

    #[custom_method(OnboardingImportScanRequest)]
    async fn dispatch_onboarding_import_scan(
        &self,
        req: OnboardingImportScanRequest,
    ) -> Result<OnboardingImportScanResponse, agent_client_protocol::Error> {
        self.on_onboarding_import_scan(req).await
    }

    #[custom_method(OnboardingImportApplyRequest)]
    async fn dispatch_onboarding_import_apply(
        &self,
        req: OnboardingImportApplyRequest,
    ) -> Result<OnboardingImportApplyResponse, agent_client_protocol::Error> {
        self.on_onboarding_import_apply(req).await
    }

    #[custom_method(ExportSessionRequest)]
    async fn dispatch_export_session(
        &self,
        req: ExportSessionRequest,
    ) -> Result<ExportSessionResponse, agent_client_protocol::Error> {
        self.on_export_session(req).await
    }

    #[custom_method(ImportSessionRequest)]
    async fn dispatch_import_session(
        &self,
        req: ImportSessionRequest,
    ) -> Result<ImportSessionResponse, agent_client_protocol::Error> {
        self.on_import_session(req).await
    }

    #[custom_method(ShareSessionNostrRequest)]
    async fn dispatch_share_session_nostr(
        &self,
        req: ShareSessionNostrRequest,
    ) -> Result<ShareSessionNostrResponse, agent_client_protocol::Error> {
        self.on_share_session_nostr(req).await
    }

    #[custom_method(EncodeRecipeRequest)]
    async fn dispatch_encode_recipe(
        &self,
        req: EncodeRecipeRequest,
    ) -> Result<EncodeRecipeResponse, agent_client_protocol::Error> {
        self.on_encode_recipe(req).await
    }

    #[custom_method(DecodeRecipeRequest)]
    async fn dispatch_decode_recipe(
        &self,
        req: DecodeRecipeRequest,
    ) -> Result<DecodeRecipeResponse, agent_client_protocol::Error> {
        self.on_decode_recipe(req).await
    }

    #[custom_method(ScanRecipeRequest)]
    async fn dispatch_scan_recipe(
        &self,
        req: ScanRecipeRequest,
    ) -> Result<ScanRecipeResponse, agent_client_protocol::Error> {
        self.on_scan_recipe(req).await
    }

    #[custom_method(ListRecipesRequest)]
    async fn dispatch_list_recipes(
        &self,
        req: ListRecipesRequest,
    ) -> Result<ListRecipesResponse, agent_client_protocol::Error> {
        self.on_list_recipes(req).await
    }

    #[custom_method(DeleteRecipeRequest)]
    async fn dispatch_delete_recipe(
        &self,
        req: DeleteRecipeRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_delete_recipe(req).await
    }

    #[custom_method(ScheduleRecipeRequest)]
    async fn dispatch_schedule_recipe(
        &self,
        req: ScheduleRecipeRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_schedule_recipe(req).await
    }

    #[custom_method(SetRecipeSlashCommandRequest)]
    async fn dispatch_set_recipe_slash_command(
        &self,
        req: SetRecipeSlashCommandRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_set_recipe_slash_command(req).await
    }

    #[custom_method(SaveRecipeRequest)]
    async fn dispatch_save_recipe(
        &self,
        req: SaveRecipeRequest,
    ) -> Result<SaveRecipeResponse, agent_client_protocol::Error> {
        self.on_save_recipe(req).await
    }

    #[custom_method(ParseRecipeRequest)]
    async fn dispatch_parse_recipe(
        &self,
        req: ParseRecipeRequest,
    ) -> Result<ParseRecipeResponse, agent_client_protocol::Error> {
        self.on_parse_recipe(req).await
    }

    #[custom_method(RecipeToYamlRequest)]
    async fn dispatch_recipe_to_yaml(
        &self,
        req: RecipeToYamlRequest,
    ) -> Result<RecipeToYamlResponse, agent_client_protocol::Error> {
        self.on_recipe_to_yaml(req).await
    }

    #[custom_method(ListSchedulesRequest)]
    async fn dispatch_list_schedules(
        &self,
        req: ListSchedulesRequest,
    ) -> Result<ListSchedulesResponse, agent_client_protocol::Error> {
        self.on_list_schedules(req).await
    }

    #[custom_method(ListScheduleSessionsRequest)]
    async fn dispatch_list_schedule_sessions(
        &self,
        req: ListScheduleSessionsRequest,
    ) -> Result<ListScheduleSessionsResponse, agent_client_protocol::Error> {
        self.on_list_schedule_sessions(req).await
    }

    #[custom_method(CreateScheduleRequest)]
    async fn dispatch_create_schedule(
        &self,
        req: CreateScheduleRequest,
    ) -> Result<CreateScheduleResponse, agent_client_protocol::Error> {
        self.on_create_schedule(req).await
    }

    #[custom_method(DeleteScheduleRequest)]
    async fn dispatch_delete_schedule(
        &self,
        req: DeleteScheduleRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_delete_schedule(req).await
    }

    #[custom_method(PauseScheduleRequest)]
    async fn dispatch_pause_schedule(
        &self,
        req: PauseScheduleRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_pause_schedule(req).await
    }

    #[custom_method(UnpauseScheduleRequest)]
    async fn dispatch_unpause_schedule(
        &self,
        req: UnpauseScheduleRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_unpause_schedule(req).await
    }

    #[custom_method(UpdateScheduleRequest)]
    async fn dispatch_update_schedule(
        &self,
        req: UpdateScheduleRequest,
    ) -> Result<UpdateScheduleResponse, agent_client_protocol::Error> {
        self.on_update_schedule(req).await
    }

    #[custom_method(RunScheduleNowRequest)]
    async fn dispatch_run_schedule_now(
        &self,
        req: RunScheduleNowRequest,
    ) -> Result<RunScheduleNowResponse, agent_client_protocol::Error> {
        self.on_run_schedule_now(req).await
    }

    #[custom_method(KillRunningJobRequest)]
    async fn dispatch_kill_running_job(
        &self,
        req: KillRunningJobRequest,
    ) -> Result<KillRunningJobResponse, agent_client_protocol::Error> {
        self.on_kill_running_job(req).await
    }

    #[custom_method(InspectRunningJobRequest)]
    async fn dispatch_inspect_running_job(
        &self,
        req: InspectRunningJobRequest,
    ) -> Result<InspectRunningJobResponse, agent_client_protocol::Error> {
        self.on_inspect_running_job(req).await
    }

    #[custom_method(GetSessionInfoRequest)]
    async fn dispatch_get_session_info(
        &self,
        req: GetSessionInfoRequest,
    ) -> Result<GetSessionInfoResponse, agent_client_protocol::Error> {
        self.on_get_session_info(req).await
    }

    #[custom_method(TruncateSessionConversationRequest)]
    async fn dispatch_truncate_session_conversation(
        &self,
        req: TruncateSessionConversationRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_truncate_session_conversation(req).await
    }

    #[custom_method(UpdateSessionProjectRequest)]
    async fn dispatch_update_session_project(
        &self,
        req: UpdateSessionProjectRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_update_session_project(req).await
    }

    #[custom_method(RenameSessionRequest)]
    async fn dispatch_rename_session(
        &self,
        req: RenameSessionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_rename_session(req).await
    }

    #[custom_method(ArchiveSessionRequest)]
    async fn dispatch_archive_session(
        &self,
        req: ArchiveSessionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_archive_session(req).await
    }

    #[custom_method(UnarchiveSessionRequest)]
    async fn dispatch_unarchive_session(
        &self,
        req: UnarchiveSessionRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_unarchive_session(req).await
    }

    #[custom_method(CreateSourceRequest)]
    async fn dispatch_create_source(
        &self,
        req: CreateSourceRequest,
    ) -> Result<CreateSourceResponse, agent_client_protocol::Error> {
        self.on_create_source(req).await
    }

    #[custom_method(ListSourcesRequest)]
    async fn dispatch_list_sources(
        &self,
        req: ListSourcesRequest,
    ) -> Result<ListSourcesResponse, agent_client_protocol::Error> {
        self.on_list_sources(req).await
    }

    #[custom_method(ListAgentMentionsRequest)]
    async fn dispatch_list_agent_mentions(
        &self,
        req: ListAgentMentionsRequest,
    ) -> Result<ListAgentMentionsResponse, agent_client_protocol::Error> {
        self.on_list_agent_mentions(req).await
    }

    #[custom_method(ListSlashCommandsRequest)]
    async fn dispatch_list_slash_commands(
        &self,
        req: ListSlashCommandsRequest,
    ) -> Result<ListSlashCommandsResponse, agent_client_protocol::Error> {
        self.on_list_slash_commands(req).await
    }

    #[custom_method(UpdateSourceRequest)]
    async fn dispatch_update_source(
        &self,
        req: UpdateSourceRequest,
    ) -> Result<UpdateSourceResponse, agent_client_protocol::Error> {
        self.on_update_source(req).await
    }

    #[custom_method(DeleteSourceRequest)]
    async fn dispatch_delete_source(
        &self,
        req: DeleteSourceRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_delete_source(req).await
    }

    #[custom_method(ExportSourceRequest)]
    async fn dispatch_export_source(
        &self,
        req: ExportSourceRequest,
    ) -> Result<ExportSourceResponse, agent_client_protocol::Error> {
        self.on_export_source(req).await
    }

    #[custom_method(ImportSourcesRequest)]
    async fn dispatch_import_sources(
        &self,
        req: ImportSourcesRequest,
    ) -> Result<ImportSourcesResponse, agent_client_protocol::Error> {
        self.on_import_sources(req).await
    }

    #[custom_method(DictationTranscribeRequest)]
    async fn dispatch_dictation_transcribe(
        &self,
        req: DictationTranscribeRequest,
    ) -> Result<DictationTranscribeResponse, agent_client_protocol::Error> {
        self.on_dictation_transcribe(req).await
    }

    #[custom_method(DictationConfigRequest)]
    async fn dispatch_dictation_config(
        &self,
        _req: DictationConfigRequest,
    ) -> Result<DictationConfigResponse, agent_client_protocol::Error> {
        self.on_dictation_config(_req).await
    }

    #[custom_method(DictationModelsListRequest)]
    async fn dispatch_dictation_models_list(
        &self,
        _req: DictationModelsListRequest,
    ) -> Result<DictationModelsListResponse, agent_client_protocol::Error> {
        self.on_dictation_models_list(_req).await
    }

    #[custom_method(DictationModelDownloadRequest)]
    async fn dispatch_dictation_model_download(
        &self,
        _req: DictationModelDownloadRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_dictation_model_download(_req).await
    }

    #[custom_method(DictationModelDownloadProgressRequest)]
    async fn dispatch_dictation_model_download_progress(
        &self,
        _req: DictationModelDownloadProgressRequest,
    ) -> Result<DictationModelDownloadProgressResponse, agent_client_protocol::Error> {
        self.on_dictation_model_download_progress(_req).await
    }

    #[custom_method(DictationModelCancelRequest)]
    async fn dispatch_dictation_model_cancel(
        &self,
        _req: DictationModelCancelRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_dictation_model_cancel(_req).await
    }

    #[custom_method(DictationModelDeleteRequest)]
    async fn dispatch_dictation_model_delete(
        &self,
        _req: DictationModelDeleteRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_dictation_model_delete(_req).await
    }

    #[custom_method(LocalInferenceModelsListRequest)]
    async fn dispatch_local_inference_models_list(
        &self,
        req: LocalInferenceModelsListRequest,
    ) -> Result<LocalInferenceModelsListResponse, agent_client_protocol::Error> {
        self.on_local_inference_models_list(req).await
    }

    #[custom_method(LocalInferenceModelDownloadRequest)]
    async fn dispatch_local_inference_model_download(
        &self,
        req: LocalInferenceModelDownloadRequest,
    ) -> Result<LocalInferenceModelDownloadResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_download(req).await
    }

    #[custom_method(LocalInferenceModelDownloadProgressRequest)]
    async fn dispatch_local_inference_model_download_progress(
        &self,
        req: LocalInferenceModelDownloadProgressRequest,
    ) -> Result<LocalInferenceModelDownloadProgressResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_download_progress(req).await
    }

    #[custom_method(LocalInferenceModelDownloadCancelRequest)]
    async fn dispatch_local_inference_model_download_cancel(
        &self,
        req: LocalInferenceModelDownloadCancelRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_download_cancel(req).await
    }

    #[custom_method(LocalInferenceModelDeleteRequest)]
    async fn dispatch_local_inference_model_delete(
        &self,
        req: LocalInferenceModelDeleteRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_delete(req).await
    }

    #[custom_method(LocalInferenceModelEvictRequest)]
    async fn dispatch_local_inference_model_evict(
        &self,
        req: LocalInferenceModelEvictRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_evict(req).await
    }

    #[custom_method(LocalInferenceModelSettingsReadRequest)]
    async fn dispatch_local_inference_model_settings_read(
        &self,
        req: LocalInferenceModelSettingsReadRequest,
    ) -> Result<LocalInferenceModelSettingsReadResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_settings_read(req).await
    }

    #[custom_method(LocalInferenceModelSettingsUpdateRequest)]
    async fn dispatch_local_inference_model_settings_update(
        &self,
        req: LocalInferenceModelSettingsUpdateRequest,
    ) -> Result<LocalInferenceModelSettingsUpdateResponse, agent_client_protocol::Error> {
        self.on_local_inference_model_settings_update(req).await
    }

    #[custom_method(LocalInferenceHuggingFaceSearchRequest)]
    async fn dispatch_local_inference_huggingface_search(
        &self,
        req: LocalInferenceHuggingFaceSearchRequest,
    ) -> Result<LocalInferenceHuggingFaceSearchResponse, agent_client_protocol::Error> {
        self.on_local_inference_huggingface_search(req).await
    }

    #[custom_method(LocalInferenceHuggingFaceRepoVariantsRequest)]
    async fn dispatch_local_inference_huggingface_repo_variants(
        &self,
        req: LocalInferenceHuggingFaceRepoVariantsRequest,
    ) -> Result<LocalInferenceHuggingFaceRepoVariantsResponse, agent_client_protocol::Error> {
        self.on_local_inference_huggingface_repo_variants(req).await
    }

    #[custom_method(LocalInferenceBuiltinChatTemplatesListRequest)]
    async fn dispatch_local_inference_builtin_chat_templates_list(
        &self,
        req: LocalInferenceBuiltinChatTemplatesListRequest,
    ) -> Result<LocalInferenceBuiltinChatTemplatesListResponse, agent_client_protocol::Error> {
        self.on_local_inference_builtin_chat_templates_list(req)
            .await
    }
}

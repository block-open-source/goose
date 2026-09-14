// This file is auto-generated — do not edit manually.

import type { ClientContext } from "@agentclientprotocol/sdk";
import type {
  AddConfigExtensionRequest_unstable,
  AddSessionExtensionRequest_unstable,
  AppsDeleteRequest_unstable,
  AppsDeleteResponse_unstable,
  AppsExportRequest_unstable,
  AppsExportResponse_unstable,
  AppsImportRequest_unstable,
  AppsImportResponse_unstable,
  AppsListRequest_unstable,
  AppsListResponse_unstable,
  ArchiveSessionRequest_unstable,
  CanonicalModelInfoRequest_unstable,
  CanonicalModelInfoResponse_unstable,
  ConfigReadAllRequest_unstable,
  ConfigReadAllResponse_unstable,
  ConfigReadRequest_unstable,
  ConfigReadResponse_unstable,
  ConfigRemoveRequest_unstable,
  ConfigUpsertRequest_unstable,
  CreateScheduleRequest_unstable,
  CreateScheduleResponse_unstable,
  CreateSourceRequest_unstable,
  CreateSourceResponse_unstable,
  CustomProviderCreateRequest_unstable,
  CustomProviderCreateResponse_unstable,
  CustomProviderDeleteRequest_unstable,
  CustomProviderDeleteResponse_unstable,
  CustomProviderReadRequest_unstable,
  CustomProviderReadResponse_unstable,
  CustomProviderUpdateRequest_unstable,
  CustomProviderUpdateResponse_unstable,
  DecodeRecipeRequest_unstable,
  DecodeRecipeResponse_unstable,
  DefaultsClearRequest_unstable,
  DefaultsReadRequest_unstable,
  DefaultsReadResponse_unstable,
  DefaultsSaveRequest_unstable,
  DeleteRecipeRequest_unstable,
  DeleteScheduleRequest_unstable,
  DeleteSourceRequest_unstable,
  DiagnosticsGetRequest_unstable,
  DiagnosticsGetResponse_unstable,
  DictationConfigRequest_unstable,
  DictationConfigResponse_unstable,
  DictationModelCancelRequest_unstable,
  DictationModelDeleteRequest_unstable,
  DictationModelDownloadProgressRequest_unstable,
  DictationModelDownloadProgressResponse_unstable,
  DictationModelDownloadRequest_unstable,
  DictationModelsListRequest_unstable,
  DictationModelsListResponse_unstable,
  DictationTranscribeRequest_unstable,
  DictationTranscribeResponse_unstable,
  EncodeRecipeRequest_unstable,
  EncodeRecipeResponse_unstable,
  ExportSessionRequest_unstable,
  ExportSessionResponse_unstable,
  ExportSourceRequest_unstable,
  ExportSourceResponse_unstable,
  GetConfigExtensionsRequest_unstable,
  GetConfigExtensionsResponse_unstable,
  GetPromptRequest_unstable,
  GetPromptResponse_unstable,
  GetSessionExtensionsRequest_unstable,
  GetSessionExtensionsResponse_unstable,
  GetSessionInfoRequest_unstable,
  GetSessionInfoResponse_unstable,
  GetToolsRequest_unstable,
  GetToolsResponse_unstable,
  GooseToolCallRequest_unstable,
  GooseToolCallResponse_unstable,
  ImportSessionRequest_unstable,
  ImportSessionResponse_unstable,
  ImportSourcesRequest_unstable,
  ImportSourcesResponse_unstable,
  InspectRunningJobRequest_unstable,
  InspectRunningJobResponse_unstable,
  KillRunningJobRequest_unstable,
  KillRunningJobResponse_unstable,
  ListAgentMentionsRequest_unstable,
  ListAgentMentionsResponse_unstable,
  ListPromptsRequest_unstable,
  ListPromptsResponse_unstable,
  ListProvidersRequest_unstable,
  ListProvidersResponse_unstable,
  ListRecipesRequest_unstable,
  ListRecipesResponse_unstable,
  ListScheduleSessionsRequest_unstable,
  ListScheduleSessionsResponse_unstable,
  ListSchedulesRequest_unstable,
  ListSchedulesResponse_unstable,
  ListSlashCommandsRequest_unstable,
  ListSlashCommandsResponse_unstable,
  ListSourcesRequest_unstable,
  ListSourcesResponse_unstable,
  LocalInferenceBuiltinChatTemplatesListRequest_unstable,
  LocalInferenceBuiltinChatTemplatesListResponse_unstable,
  LocalInferenceHuggingFaceRepoVariantsRequest_unstable,
  LocalInferenceHuggingFaceRepoVariantsResponse_unstable,
  LocalInferenceHuggingFaceSearchRequest_unstable,
  LocalInferenceHuggingFaceSearchResponse_unstable,
  LocalInferenceModelDeleteRequest_unstable,
  LocalInferenceModelDownloadCancelRequest_unstable,
  LocalInferenceModelDownloadProgressRequest_unstable,
  LocalInferenceModelDownloadProgressResponse_unstable,
  LocalInferenceModelDownloadRequest_unstable,
  LocalInferenceModelDownloadResponse_unstable,
  LocalInferenceModelEvictRequest_unstable,
  LocalInferenceModelSettingsReadRequest_unstable,
  LocalInferenceModelSettingsReadResponse_unstable,
  LocalInferenceModelSettingsUpdateRequest_unstable,
  LocalInferenceModelSettingsUpdateResponse_unstable,
  LocalInferenceModelsListRequest_unstable,
  LocalInferenceModelsListResponse_unstable,
  OnboardingImportApplyRequest_unstable,
  OnboardingImportApplyResponse_unstable,
  OnboardingImportScanRequest_unstable,
  OnboardingImportScanResponse_unstable,
  ParseRecipeRequest_unstable,
  ParseRecipeResponse_unstable,
  PauseScheduleRequest_unstable,
  PreferencesReadRequest_unstable,
  PreferencesReadResponse_unstable,
  PreferencesSaveRequest_unstable,
  PromptOperationResponse_unstable,
  ProviderCatalogListRequest_unstable,
  ProviderCatalogListResponse_unstable,
  ProviderCatalogTemplateRequest_unstable,
  ProviderCatalogTemplateResponse_unstable,
  ProviderConfigAuthenticateRequest_unstable,
  ProviderConfigChangeResponse_unstable,
  ProviderConfigDeleteRequest_unstable,
  ProviderConfigReadRequest_unstable,
  ProviderConfigReadResponse_unstable,
  ProviderConfigSaveRequest_unstable,
  ProviderConfigStatusRequest_unstable,
  ProviderConfigStatusResponse_unstable,
  ProviderReadinessCheckRequest_unstable,
  ProviderReadinessCheckResponse_unstable,
  ProviderSecretDeleteRequest_unstable,
  ProviderSecretsListRequest_unstable,
  ProviderSecretsListResponse_unstable,
  ProviderSetupCatalogListRequest_unstable,
  ProviderSetupCatalogListResponse_unstable,
  ProviderSupportedModelsListRequest_unstable,
  ProviderSupportedModelsListResponse_unstable,
  ReadResourceRequest_unstable,
  ReadResourceResponse_unstable,
  RecipeToYamlRequest_unstable,
  RecipeToYamlResponse_unstable,
  RefreshProviderInventoryRequest_unstable,
  RefreshProviderInventoryResponse_unstable,
  RemoveConfigExtensionRequest_unstable,
  RemoveSessionExtensionRequest_unstable,
  RenameSessionRequest_unstable,
  ResetPromptRequest_unstable,
  RunScheduleNowRequest_unstable,
  RunScheduleNowResponse_unstable,
  SavePromptRequest_unstable,
  SaveRecipeRequest_unstable,
  SaveRecipeResponse_unstable,
  ScanRecipeRequest_unstable,
  ScanRecipeResponse_unstable,
  ScheduleRecipeRequest_unstable,
  SetConfigExtensionEnabledRequest_unstable,
  SetRecipeSlashCommandRequest_unstable,
  SetSessionSystemPromptRequest_unstable,
  SetToolPermissionsRequest_unstable,
  SetToolPermissionsResponse_unstable,
  ShareSessionNostrRequest_unstable,
  ShareSessionNostrResponse_unstable,
  SteerSessionRequest_unstable,
  SteerSessionResponse_unstable,
  TruncateSessionConversationRequest_unstable,
  UnarchiveSessionRequest_unstable,
  UnpauseScheduleRequest_unstable,
  UpdateScheduleRequest_unstable,
  UpdateScheduleResponse_unstable,
  UpdateSessionProjectRequest_unstable,
  UpdateSourceRequest_unstable,
  UpdateSourceResponse_unstable,
  UpdateWorkingDirRequest_unstable,
} from './types.gen.js';
import {
  zAppsDeleteResponse_unstable,
  zAppsExportResponse_unstable,
  zAppsImportResponse_unstable,
  zAppsListResponse_unstable,
  zCanonicalModelInfoResponse_unstable,
  zConfigReadAllResponse_unstable,
  zConfigReadResponse_unstable,
  zCreateScheduleResponse_unstable,
  zCreateSourceResponse_unstable,
  zCustomProviderCreateResponse_unstable,
  zCustomProviderDeleteResponse_unstable,
  zCustomProviderReadResponse_unstable,
  zCustomProviderUpdateResponse_unstable,
  zDecodeRecipeResponse_unstable,
  zDefaultsReadResponse_unstable,
  zDiagnosticsGetResponse_unstable,
  zDictationConfigResponse_unstable,
  zDictationModelDownloadProgressResponse_unstable,
  zDictationModelsListResponse_unstable,
  zDictationTranscribeResponse_unstable,
  zEncodeRecipeResponse_unstable,
  zExportSessionResponse_unstable,
  zExportSourceResponse_unstable,
  zGetConfigExtensionsResponse_unstable,
  zGetPromptResponse_unstable,
  zGetSessionExtensionsResponse_unstable,
  zGetSessionInfoResponse_unstable,
  zGetToolsResponse_unstable,
  zGooseToolCallResponse_unstable,
  zImportSessionResponse_unstable,
  zImportSourcesResponse_unstable,
  zInspectRunningJobResponse_unstable,
  zKillRunningJobResponse_unstable,
  zListAgentMentionsResponse_unstable,
  zListPromptsResponse_unstable,
  zListProvidersResponse_unstable,
  zListRecipesResponse_unstable,
  zListScheduleSessionsResponse_unstable,
  zListSchedulesResponse_unstable,
  zListSlashCommandsResponse_unstable,
  zListSourcesResponse_unstable,
  zLocalInferenceBuiltinChatTemplatesListResponse_unstable,
  zLocalInferenceHuggingFaceRepoVariantsResponse_unstable,
  zLocalInferenceHuggingFaceSearchResponse_unstable,
  zLocalInferenceModelDownloadProgressResponse_unstable,
  zLocalInferenceModelDownloadResponse_unstable,
  zLocalInferenceModelSettingsReadResponse_unstable,
  zLocalInferenceModelSettingsUpdateResponse_unstable,
  zLocalInferenceModelsListResponse_unstable,
  zOnboardingImportApplyResponse_unstable,
  zOnboardingImportScanResponse_unstable,
  zParseRecipeResponse_unstable,
  zPreferencesReadResponse_unstable,
  zPromptOperationResponse_unstable,
  zProviderCatalogListResponse_unstable,
  zProviderCatalogTemplateResponse_unstable,
  zProviderConfigChangeResponse_unstable,
  zProviderConfigReadResponse_unstable,
  zProviderConfigStatusResponse_unstable,
  zProviderReadinessCheckResponse_unstable,
  zProviderSecretsListResponse_unstable,
  zProviderSetupCatalogListResponse_unstable,
  zProviderSupportedModelsListResponse_unstable,
  zReadResourceResponse_unstable,
  zRecipeToYamlResponse_unstable,
  zRefreshProviderInventoryResponse_unstable,
  zRunScheduleNowResponse_unstable,
  zSaveRecipeResponse_unstable,
  zScanRecipeResponse_unstable,
  zSetToolPermissionsResponse_unstable,
  zShareSessionNostrResponse_unstable,
  zSteerSessionResponse_unstable,
  zUpdateScheduleResponse_unstable,
  zUpdateSourceResponse_unstable,
} from './zod.gen.js';

export class GooseExtClient {
  constructor(private conn: Pick<ClientContext, "request">) {}

  async sessionExtensionsAdd_unstable(
    params: AddSessionExtensionRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/session/extensions/add", params);
  }

  async sessionExtensionsRemove_unstable(
    params: RemoveSessionExtensionRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/session/extensions/remove",
      params,
    );
  }

  async toolsList_unstable(
    params: GetToolsRequest_unstable,
  ): Promise<GetToolsResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/tools/list", params);
    return zGetToolsResponse_unstable.parse(raw) as GetToolsResponse_unstable;
  }

  async toolsPermissionsSet_unstable(
    params: SetToolPermissionsRequest_unstable,
  ): Promise<SetToolPermissionsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/tools/permissions/set",
      params,
    );
    return zSetToolPermissionsResponse_unstable.parse(
      raw,
    ) as SetToolPermissionsResponse_unstable;
  }

  async toolsCall_unstable(
    params: GooseToolCallRequest_unstable,
  ): Promise<GooseToolCallResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/tools/call", params);
    return zGooseToolCallResponse_unstable.parse(
      raw,
    ) as GooseToolCallResponse_unstable;
  }

  async resourcesRead_unstable(
    params: ReadResourceRequest_unstable,
  ): Promise<ReadResourceResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/resources/read",
      params,
    );
    return zReadResourceResponse_unstable.parse(
      raw,
    ) as ReadResourceResponse_unstable;
  }

  async appsList_unstable(
    params: AppsListRequest_unstable,
  ): Promise<AppsListResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/apps/list", params);
    return zAppsListResponse_unstable.parse(raw) as AppsListResponse_unstable;
  }

  async appsExport_unstable(
    params: AppsExportRequest_unstable,
  ): Promise<AppsExportResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/apps/export", params);
    return zAppsExportResponse_unstable.parse(
      raw,
    ) as AppsExportResponse_unstable;
  }

  async appsImport_unstable(
    params: AppsImportRequest_unstable,
  ): Promise<AppsImportResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/apps/import", params);
    return zAppsImportResponse_unstable.parse(
      raw,
    ) as AppsImportResponse_unstable;
  }

  async appsDelete_unstable(
    params: AppsDeleteRequest_unstable,
  ): Promise<AppsDeleteResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/apps/delete", params);
    return zAppsDeleteResponse_unstable.parse(
      raw,
    ) as AppsDeleteResponse_unstable;
  }

  async sessionWorkingDirUpdate_unstable(
    params: UpdateWorkingDirRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/session/working-dir/update",
      params,
    );
  }

  async sessionSystemPromptSet_unstable(
    params: SetSessionSystemPromptRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/session/system-prompt/set",
      params,
    );
  }

  async sessionSteer_unstable(
    params: SteerSessionRequest_unstable,
  ): Promise<SteerSessionResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/session/steer",
      params,
    );
    return zSteerSessionResponse_unstable.parse(
      raw,
    ) as SteerSessionResponse_unstable;
  }

  async diagnosticsGet_unstable(
    params: DiagnosticsGetRequest_unstable,
  ): Promise<DiagnosticsGetResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/diagnostics/get",
      params,
    );
    return zDiagnosticsGetResponse_unstable.parse(
      raw,
    ) as DiagnosticsGetResponse_unstable;
  }

  async configPromptsList_unstable(
    params: ListPromptsRequest_unstable,
  ): Promise<ListPromptsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/config/prompts/list",
      params,
    );
    return zListPromptsResponse_unstable.parse(
      raw,
    ) as ListPromptsResponse_unstable;
  }

  async configPromptsGet_unstable(
    params: GetPromptRequest_unstable,
  ): Promise<GetPromptResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/config/prompts/get",
      params,
    );
    return zGetPromptResponse_unstable.parse(raw) as GetPromptResponse_unstable;
  }

  async configPromptsSave_unstable(
    params: SavePromptRequest_unstable,
  ): Promise<PromptOperationResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/config/prompts/save",
      params,
    );
    return zPromptOperationResponse_unstable.parse(
      raw,
    ) as PromptOperationResponse_unstable;
  }

  async configPromptsReset_unstable(
    params: ResetPromptRequest_unstable,
  ): Promise<PromptOperationResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/config/prompts/reset",
      params,
    );
    return zPromptOperationResponse_unstable.parse(
      raw,
    ) as PromptOperationResponse_unstable;
  }

  async configExtensionsList_unstable(
    params: GetConfigExtensionsRequest_unstable,
  ): Promise<GetConfigExtensionsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/config/extensions/list",
      params,
    );
    return zGetConfigExtensionsResponse_unstable.parse(
      raw,
    ) as GetConfigExtensionsResponse_unstable;
  }

  async configExtensionsAdd_unstable(
    params: AddConfigExtensionRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/config/extensions/add", params);
  }

  async configExtensionsRemove_unstable(
    params: RemoveConfigExtensionRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/config/extensions/remove", params);
  }

  async configExtensionsSetEnabled_unstable(
    params: SetConfigExtensionEnabledRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/config/extensions/set-enabled",
      params,
    );
  }

  async sessionExtensionsList_unstable(
    params: GetSessionExtensionsRequest_unstable,
  ): Promise<GetSessionExtensionsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/session/extensions/list",
      params,
    );
    return zGetSessionExtensionsResponse_unstable.parse(
      raw,
    ) as GetSessionExtensionsResponse_unstable;
  }

  async providersList_unstable(
    params: ListProvidersRequest_unstable,
  ): Promise<ListProvidersResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/list",
      params,
    );
    return zListProvidersResponse_unstable.parse(
      raw,
    ) as ListProvidersResponse_unstable;
  }

  async providersSupportedModelsList_unstable(
    params: ProviderSupportedModelsListRequest_unstable,
  ): Promise<ProviderSupportedModelsListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/supported-models/list",
      params,
    );
    return zProviderSupportedModelsListResponse_unstable.parse(
      raw,
    ) as ProviderSupportedModelsListResponse_unstable;
  }

  async providersCatalogList_unstable(
    params: ProviderCatalogListRequest_unstable,
  ): Promise<ProviderCatalogListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/catalog/list",
      params,
    );
    return zProviderCatalogListResponse_unstable.parse(
      raw,
    ) as ProviderCatalogListResponse_unstable;
  }

  async providersSetupCatalogList_unstable(
    params: ProviderSetupCatalogListRequest_unstable,
  ): Promise<ProviderSetupCatalogListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/setup/catalog/list",
      params,
    );
    return zProviderSetupCatalogListResponse_unstable.parse(
      raw,
    ) as ProviderSetupCatalogListResponse_unstable;
  }

  async providersCatalogTemplate_unstable(
    params: ProviderCatalogTemplateRequest_unstable,
  ): Promise<ProviderCatalogTemplateResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/catalog/template",
      params,
    );
    return zProviderCatalogTemplateResponse_unstable.parse(
      raw,
    ) as ProviderCatalogTemplateResponse_unstable;
  }

  async providersCustomCreate_unstable(
    params: CustomProviderCreateRequest_unstable,
  ): Promise<CustomProviderCreateResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/custom/create",
      params,
    );
    return zCustomProviderCreateResponse_unstable.parse(
      raw,
    ) as CustomProviderCreateResponse_unstable;
  }

  async providersCustomRead_unstable(
    params: CustomProviderReadRequest_unstable,
  ): Promise<CustomProviderReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/custom/read",
      params,
    );
    return zCustomProviderReadResponse_unstable.parse(
      raw,
    ) as CustomProviderReadResponse_unstable;
  }

  async providersCustomUpdate_unstable(
    params: CustomProviderUpdateRequest_unstable,
  ): Promise<CustomProviderUpdateResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/custom/update",
      params,
    );
    return zCustomProviderUpdateResponse_unstable.parse(
      raw,
    ) as CustomProviderUpdateResponse_unstable;
  }

  async providersCustomDelete_unstable(
    params: CustomProviderDeleteRequest_unstable,
  ): Promise<CustomProviderDeleteResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/custom/delete",
      params,
    );
    return zCustomProviderDeleteResponse_unstable.parse(
      raw,
    ) as CustomProviderDeleteResponse_unstable;
  }

  async providersInventoryRefresh_unstable(
    params: RefreshProviderInventoryRequest_unstable,
  ): Promise<RefreshProviderInventoryResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/inventory/refresh",
      params,
    );
    return zRefreshProviderInventoryResponse_unstable.parse(
      raw,
    ) as RefreshProviderInventoryResponse_unstable;
  }

  async providersReadinessCheck_unstable(
    params: ProviderReadinessCheckRequest_unstable,
  ): Promise<ProviderReadinessCheckResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/readiness/check",
      params,
    );
    return zProviderReadinessCheckResponse_unstable.parse(
      raw,
    ) as ProviderReadinessCheckResponse_unstable;
  }

  async providersConfigRead_unstable(
    params: ProviderConfigReadRequest_unstable,
  ): Promise<ProviderConfigReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/config/read",
      params,
    );
    return zProviderConfigReadResponse_unstable.parse(
      raw,
    ) as ProviderConfigReadResponse_unstable;
  }

  async providersConfigStatus_unstable(
    params: ProviderConfigStatusRequest_unstable,
  ): Promise<ProviderConfigStatusResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/config/status",
      params,
    );
    return zProviderConfigStatusResponse_unstable.parse(
      raw,
    ) as ProviderConfigStatusResponse_unstable;
  }

  async providersConfigSave_unstable(
    params: ProviderConfigSaveRequest_unstable,
  ): Promise<ProviderConfigChangeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/config/save",
      params,
    );
    return zProviderConfigChangeResponse_unstable.parse(
      raw,
    ) as ProviderConfigChangeResponse_unstable;
  }

  async providersConfigDelete_unstable(
    params: ProviderConfigDeleteRequest_unstable,
  ): Promise<ProviderConfigChangeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/config/delete",
      params,
    );
    return zProviderConfigChangeResponse_unstable.parse(
      raw,
    ) as ProviderConfigChangeResponse_unstable;
  }

  async providersConfigAuthenticate_unstable(
    params: ProviderConfigAuthenticateRequest_unstable,
  ): Promise<ProviderConfigChangeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/config/authenticate",
      params,
    );
    return zProviderConfigChangeResponse_unstable.parse(
      raw,
    ) as ProviderConfigChangeResponse_unstable;
  }

  async providersSecretsList_unstable(
    params: ProviderSecretsListRequest_unstable,
  ): Promise<ProviderSecretsListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/secrets/list",
      params,
    );
    return zProviderSecretsListResponse_unstable.parse(
      raw,
    ) as ProviderSecretsListResponse_unstable;
  }

  async providersSecretsDelete_unstable(
    params: ProviderSecretDeleteRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/providers/secrets/delete", params);
  }

  async providersCanonicalModelInfo_unstable(
    params: CanonicalModelInfoRequest_unstable,
  ): Promise<CanonicalModelInfoResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/providers/canonical-model-info",
      params,
    );
    return zCanonicalModelInfoResponse_unstable.parse(
      raw,
    ) as CanonicalModelInfoResponse_unstable;
  }

  async preferencesRead_unstable(
    params: PreferencesReadRequest_unstable,
  ): Promise<PreferencesReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/preferences/read",
      params,
    );
    return zPreferencesReadResponse_unstable.parse(
      raw,
    ) as PreferencesReadResponse_unstable;
  }

  async preferencesSave_unstable(
    params: PreferencesSaveRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/preferences/save", params);
  }

  async configRead_unstable(
    params: ConfigReadRequest_unstable,
  ): Promise<ConfigReadResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/config/read", params);
    return zConfigReadResponse_unstable.parse(
      raw,
    ) as ConfigReadResponse_unstable;
  }

  async configUpsert_unstable(
    params: ConfigUpsertRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/config/upsert", params);
  }

  async configRemove_unstable(
    params: ConfigRemoveRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/config/remove", params);
  }

  async configReadAll_unstable(
    params: ConfigReadAllRequest_unstable,
  ): Promise<ConfigReadAllResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/config/read-all",
      params,
    );
    return zConfigReadAllResponse_unstable.parse(
      raw,
    ) as ConfigReadAllResponse_unstable;
  }

  async defaultsRead_unstable(
    params: DefaultsReadRequest_unstable,
  ): Promise<DefaultsReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/defaults/read",
      params,
    );
    return zDefaultsReadResponse_unstable.parse(
      raw,
    ) as DefaultsReadResponse_unstable;
  }

  async defaultsSave_unstable(
    params: DefaultsSaveRequest_unstable,
  ): Promise<DefaultsReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/defaults/save",
      params,
    );
    return zDefaultsReadResponse_unstable.parse(
      raw,
    ) as DefaultsReadResponse_unstable;
  }

  async defaultsClear_unstable(
    params: DefaultsClearRequest_unstable,
  ): Promise<DefaultsReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/defaults/clear",
      params,
    );
    return zDefaultsReadResponse_unstable.parse(
      raw,
    ) as DefaultsReadResponse_unstable;
  }

  async onboardingImportScan_unstable(
    params: OnboardingImportScanRequest_unstable,
  ): Promise<OnboardingImportScanResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/onboarding/import/scan",
      params,
    );
    return zOnboardingImportScanResponse_unstable.parse(
      raw,
    ) as OnboardingImportScanResponse_unstable;
  }

  async onboardingImportApply_unstable(
    params: OnboardingImportApplyRequest_unstable,
  ): Promise<OnboardingImportApplyResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/onboarding/import/apply",
      params,
    );
    return zOnboardingImportApplyResponse_unstable.parse(
      raw,
    ) as OnboardingImportApplyResponse_unstable;
  }

  async sessionExport_unstable(
    params: ExportSessionRequest_unstable,
  ): Promise<ExportSessionResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/session/export",
      params,
    );
    return zExportSessionResponse_unstable.parse(
      raw,
    ) as ExportSessionResponse_unstable;
  }

  async sessionImport_unstable(
    params: ImportSessionRequest_unstable,
  ): Promise<ImportSessionResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/session/import",
      params,
    );
    return zImportSessionResponse_unstable.parse(
      raw,
    ) as ImportSessionResponse_unstable;
  }

  async sessionShareNostr_unstable(
    params: ShareSessionNostrRequest_unstable,
  ): Promise<ShareSessionNostrResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/session/share/nostr",
      params,
    );
    return zShareSessionNostrResponse_unstable.parse(
      raw,
    ) as ShareSessionNostrResponse_unstable;
  }

  async recipesEncode_unstable(
    params: EncodeRecipeRequest_unstable,
  ): Promise<EncodeRecipeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/recipes/encode",
      params,
    );
    return zEncodeRecipeResponse_unstable.parse(
      raw,
    ) as EncodeRecipeResponse_unstable;
  }

  async recipesDecode_unstable(
    params: DecodeRecipeRequest_unstable,
  ): Promise<DecodeRecipeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/recipes/decode",
      params,
    );
    return zDecodeRecipeResponse_unstable.parse(
      raw,
    ) as DecodeRecipeResponse_unstable;
  }

  async recipesScan_unstable(
    params: ScanRecipeRequest_unstable,
  ): Promise<ScanRecipeResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/recipes/scan", params);
    return zScanRecipeResponse_unstable.parse(
      raw,
    ) as ScanRecipeResponse_unstable;
  }

  async recipesList_unstable(
    params: ListRecipesRequest_unstable,
  ): Promise<ListRecipesResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/recipes/list", params);
    return zListRecipesResponse_unstable.parse(
      raw,
    ) as ListRecipesResponse_unstable;
  }

  async recipesDelete_unstable(
    params: DeleteRecipeRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/recipes/delete", params);
  }

  async recipesSchedule_unstable(
    params: ScheduleRecipeRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/recipes/schedule", params);
  }

  async recipesSlashCommand_unstable(
    params: SetRecipeSlashCommandRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/recipes/slash-command", params);
  }

  async recipesSave_unstable(
    params: SaveRecipeRequest_unstable,
  ): Promise<SaveRecipeResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/recipes/save", params);
    return zSaveRecipeResponse_unstable.parse(
      raw,
    ) as SaveRecipeResponse_unstable;
  }

  async recipesParse_unstable(
    params: ParseRecipeRequest_unstable,
  ): Promise<ParseRecipeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/recipes/parse",
      params,
    );
    return zParseRecipeResponse_unstable.parse(
      raw,
    ) as ParseRecipeResponse_unstable;
  }

  async recipesToYaml_unstable(
    params: RecipeToYamlRequest_unstable,
  ): Promise<RecipeToYamlResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/recipes/to-yaml",
      params,
    );
    return zRecipeToYamlResponse_unstable.parse(
      raw,
    ) as RecipeToYamlResponse_unstable;
  }

  async schedulesList_unstable(
    params: ListSchedulesRequest_unstable,
  ): Promise<ListSchedulesResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/list",
      params,
    );
    return zListSchedulesResponse_unstable.parse(
      raw,
    ) as ListSchedulesResponse_unstable;
  }

  async schedulesSessionsList_unstable(
    params: ListScheduleSessionsRequest_unstable,
  ): Promise<ListScheduleSessionsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/sessions/list",
      params,
    );
    return zListScheduleSessionsResponse_unstable.parse(
      raw,
    ) as ListScheduleSessionsResponse_unstable;
  }

  async schedulesCreate_unstable(
    params: CreateScheduleRequest_unstable,
  ): Promise<CreateScheduleResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/create",
      params,
    );
    return zCreateScheduleResponse_unstable.parse(
      raw,
    ) as CreateScheduleResponse_unstable;
  }

  async schedulesDelete_unstable(
    params: DeleteScheduleRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/schedules/delete", params);
  }

  async schedulesPause_unstable(
    params: PauseScheduleRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/schedules/pause", params);
  }

  async schedulesUnpause_unstable(
    params: UnpauseScheduleRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/schedules/unpause", params);
  }

  async schedulesUpdate_unstable(
    params: UpdateScheduleRequest_unstable,
  ): Promise<UpdateScheduleResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/update",
      params,
    );
    return zUpdateScheduleResponse_unstable.parse(
      raw,
    ) as UpdateScheduleResponse_unstable;
  }

  async schedulesRunNow_unstable(
    params: RunScheduleNowRequest_unstable,
  ): Promise<RunScheduleNowResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/run-now",
      params,
    );
    return zRunScheduleNowResponse_unstable.parse(
      raw,
    ) as RunScheduleNowResponse_unstable;
  }

  async schedulesRunningJobKill_unstable(
    params: KillRunningJobRequest_unstable,
  ): Promise<KillRunningJobResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/running-job/kill",
      params,
    );
    return zKillRunningJobResponse_unstable.parse(
      raw,
    ) as KillRunningJobResponse_unstable;
  }

  async schedulesRunningJobInspect_unstable(
    params: InspectRunningJobRequest_unstable,
  ): Promise<InspectRunningJobResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/schedules/running-job/inspect",
      params,
    );
    return zInspectRunningJobResponse_unstable.parse(
      raw,
    ) as InspectRunningJobResponse_unstable;
  }

  async sessionInfo_unstable(
    params: GetSessionInfoRequest_unstable,
  ): Promise<GetSessionInfoResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/session/info", params);
    return zGetSessionInfoResponse_unstable.parse(
      raw,
    ) as GetSessionInfoResponse_unstable;
  }

  async sessionConversationTruncate_unstable(
    params: TruncateSessionConversationRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/session/conversation/truncate",
      params,
    );
  }

  async sessionProjectUpdate_unstable(
    params: UpdateSessionProjectRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/session/project/update", params);
  }

  async sessionRename_unstable(
    params: RenameSessionRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/session/rename", params);
  }

  async sessionArchive_unstable(
    params: ArchiveSessionRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/session/archive", params);
  }

  async sessionUnarchive_unstable(
    params: UnarchiveSessionRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/session/unarchive", params);
  }

  async sourcesCreate_unstable(
    params: CreateSourceRequest_unstable,
  ): Promise<CreateSourceResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/sources/create",
      params,
    );
    return zCreateSourceResponse_unstable.parse(
      raw,
    ) as CreateSourceResponse_unstable;
  }

  async sourcesList_unstable(
    params: ListSourcesRequest_unstable,
  ): Promise<ListSourcesResponse_unstable> {
    const raw = await this.conn.request("_goose/unstable/sources/list", params);
    return zListSourcesResponse_unstable.parse(
      raw,
    ) as ListSourcesResponse_unstable;
  }

  async agentMentionsList_unstable(
    params: ListAgentMentionsRequest_unstable,
  ): Promise<ListAgentMentionsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/agent-mentions/list",
      params,
    );
    return zListAgentMentionsResponse_unstable.parse(
      raw,
    ) as ListAgentMentionsResponse_unstable;
  }

  async slashCommandsList_unstable(
    params: ListSlashCommandsRequest_unstable,
  ): Promise<ListSlashCommandsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/slash-commands/list",
      params,
    );
    return zListSlashCommandsResponse_unstable.parse(
      raw,
    ) as ListSlashCommandsResponse_unstable;
  }

  async sourcesUpdate_unstable(
    params: UpdateSourceRequest_unstable,
  ): Promise<UpdateSourceResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/sources/update",
      params,
    );
    return zUpdateSourceResponse_unstable.parse(
      raw,
    ) as UpdateSourceResponse_unstable;
  }

  async sourcesDelete_unstable(
    params: DeleteSourceRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/sources/delete", params);
  }

  async sourcesExport_unstable(
    params: ExportSourceRequest_unstable,
  ): Promise<ExportSourceResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/sources/export",
      params,
    );
    return zExportSourceResponse_unstable.parse(
      raw,
    ) as ExportSourceResponse_unstable;
  }

  async sourcesImport_unstable(
    params: ImportSourcesRequest_unstable,
  ): Promise<ImportSourcesResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/sources/import",
      params,
    );
    return zImportSourcesResponse_unstable.parse(
      raw,
    ) as ImportSourcesResponse_unstable;
  }

  async dictationTranscribe_unstable(
    params: DictationTranscribeRequest_unstable,
  ): Promise<DictationTranscribeResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/dictation/transcribe",
      params,
    );
    return zDictationTranscribeResponse_unstable.parse(
      raw,
    ) as DictationTranscribeResponse_unstable;
  }

  async dictationConfig_unstable(
    params: DictationConfigRequest_unstable,
  ): Promise<DictationConfigResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/dictation/config",
      params,
    );
    return zDictationConfigResponse_unstable.parse(
      raw,
    ) as DictationConfigResponse_unstable;
  }

  async dictationModelsList_unstable(
    params: DictationModelsListRequest_unstable,
  ): Promise<DictationModelsListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/dictation/models/list",
      params,
    );
    return zDictationModelsListResponse_unstable.parse(
      raw,
    ) as DictationModelsListResponse_unstable;
  }

  async dictationModelsDownload_unstable(
    params: DictationModelDownloadRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/dictation/models/download",
      params,
    );
  }

  async dictationModelsDownloadProgress_unstable(
    params: DictationModelDownloadProgressRequest_unstable,
  ): Promise<DictationModelDownloadProgressResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/dictation/models/download/progress",
      params,
    );
    return zDictationModelDownloadProgressResponse_unstable.parse(
      raw,
    ) as DictationModelDownloadProgressResponse_unstable;
  }

  async dictationModelsCancel_unstable(
    params: DictationModelCancelRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/dictation/models/cancel", params);
  }

  async dictationModelsDelete_unstable(
    params: DictationModelDeleteRequest_unstable,
  ): Promise<void> {
    await this.conn.request("_goose/unstable/dictation/models/delete", params);
  }

  async localInferenceModelsList_unstable(
    params: LocalInferenceModelsListRequest_unstable,
  ): Promise<LocalInferenceModelsListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/models/list",
      params,
    );
    return zLocalInferenceModelsListResponse_unstable.parse(
      raw,
    ) as LocalInferenceModelsListResponse_unstable;
  }

  async localInferenceModelsDownload_unstable(
    params: LocalInferenceModelDownloadRequest_unstable,
  ): Promise<LocalInferenceModelDownloadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/models/download",
      params,
    );
    return zLocalInferenceModelDownloadResponse_unstable.parse(
      raw,
    ) as LocalInferenceModelDownloadResponse_unstable;
  }

  async localInferenceModelsDownloadProgress_unstable(
    params: LocalInferenceModelDownloadProgressRequest_unstable,
  ): Promise<LocalInferenceModelDownloadProgressResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/models/download/progress",
      params,
    );
    return zLocalInferenceModelDownloadProgressResponse_unstable.parse(
      raw,
    ) as LocalInferenceModelDownloadProgressResponse_unstable;
  }

  async localInferenceModelsDownloadCancel_unstable(
    params: LocalInferenceModelDownloadCancelRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/local-inference/models/download/cancel",
      params,
    );
  }

  async localInferenceModelsDelete_unstable(
    params: LocalInferenceModelDeleteRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/local-inference/models/delete",
      params,
    );
  }

  async localInferenceModelsEvict_unstable(
    params: LocalInferenceModelEvictRequest_unstable,
  ): Promise<void> {
    await this.conn.request(
      "_goose/unstable/local-inference/models/evict",
      params,
    );
  }

  async localInferenceModelsSettingsRead_unstable(
    params: LocalInferenceModelSettingsReadRequest_unstable,
  ): Promise<LocalInferenceModelSettingsReadResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/models/settings/read",
      params,
    );
    return zLocalInferenceModelSettingsReadResponse_unstable.parse(
      raw,
    ) as LocalInferenceModelSettingsReadResponse_unstable;
  }

  async localInferenceModelsSettingsUpdate_unstable(
    params: LocalInferenceModelSettingsUpdateRequest_unstable,
  ): Promise<LocalInferenceModelSettingsUpdateResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/models/settings/update",
      params,
    );
    return zLocalInferenceModelSettingsUpdateResponse_unstable.parse(
      raw,
    ) as LocalInferenceModelSettingsUpdateResponse_unstable;
  }

  async localInferenceHuggingfaceSearch_unstable(
    params: LocalInferenceHuggingFaceSearchRequest_unstable,
  ): Promise<LocalInferenceHuggingFaceSearchResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/huggingface/search",
      params,
    );
    return zLocalInferenceHuggingFaceSearchResponse_unstable.parse(
      raw,
    ) as LocalInferenceHuggingFaceSearchResponse_unstable;
  }

  async localInferenceHuggingfaceRepoVariants_unstable(
    params: LocalInferenceHuggingFaceRepoVariantsRequest_unstable,
  ): Promise<LocalInferenceHuggingFaceRepoVariantsResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/huggingface/repo/variants",
      params,
    );
    return zLocalInferenceHuggingFaceRepoVariantsResponse_unstable.parse(
      raw,
    ) as LocalInferenceHuggingFaceRepoVariantsResponse_unstable;
  }

  async localInferenceChatTemplatesBuiltinList_unstable(
    params: LocalInferenceBuiltinChatTemplatesListRequest_unstable,
  ): Promise<LocalInferenceBuiltinChatTemplatesListResponse_unstable> {
    const raw = await this.conn.request(
      "_goose/unstable/local-inference/chat-templates/builtin/list",
      params,
    );
    return zLocalInferenceBuiltinChatTemplatesListResponse_unstable.parse(
      raw,
    ) as LocalInferenceBuiltinChatTemplatesListResponse_unstable;
  }
}

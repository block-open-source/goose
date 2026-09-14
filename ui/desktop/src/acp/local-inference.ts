import type {
  LocalInferenceDownloadProgressDto,
  LocalInferenceHfModelInfoDto,
  LocalInferenceHfModelVariantDto,
  LocalInferenceModelDownloadRequest_unstable,
  LocalInferenceModelDto,
  LocalInferenceModelSettingsDto,
} from '@aaif/goose-acp-client';
import { getAcpClient } from './acpConnection';

export type LocalModelResponse = LocalInferenceModelDto;
export type DownloadProgress = LocalInferenceDownloadProgressDto;
export type DownloadModelRequest = LocalInferenceModelDownloadRequest_unstable;
export type HfModelInfo = LocalInferenceHfModelInfoDto;
export type HfModelVariant = LocalInferenceHfModelVariantDto;
export type ModelSettings = LocalInferenceModelSettingsDto;
export type SamplingConfig = NonNullable<LocalInferenceModelSettingsDto['sampling']>;
export type ToolCallingMode = NonNullable<LocalInferenceModelSettingsDto['toolCalling']>;
export type ChatTemplate = NonNullable<LocalInferenceModelSettingsDto['chatTemplate']>;

export type RepoVariantsResponse = {
  variants: HfModelVariant[];
  recommendedIndex: number | null;
  availableMemoryBytes: number;
  downloadedQuants: string[];
  downloadedVariants: string[];
};

export async function listLocalModels(): Promise<LocalModelResponse[]> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceModelsList_unstable({});
  return response.models;
}

export async function downloadHfModel(request: DownloadModelRequest): Promise<string> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceModelsDownload_unstable(request);
  return response.modelId;
}

export async function getLocalModelDownloadProgress(
  modelId: string
): Promise<DownloadProgress | null> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceModelsDownloadProgress_unstable({ modelId });
  return response.progress ?? null;
}

export async function cancelLocalModelDownload(modelId: string): Promise<void> {
  const client = await getAcpClient();
  await client.goose.localInferenceModelsDownloadCancel_unstable({ modelId });
}

export async function deleteLocalModel(modelId: string): Promise<void> {
  const client = await getAcpClient();
  await client.goose.localInferenceModelsDelete_unstable({ modelId });
}

export async function evictLocalModel(modelId: string): Promise<void> {
  const client = await getAcpClient();
  await client.goose.localInferenceModelsEvict_unstable({ modelId });
}

export async function getModelSettings(modelId: string): Promise<ModelSettings> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceModelsSettingsRead_unstable({ modelId });
  return response.settings;
}

export async function updateModelSettings(
  modelId: string,
  settings: ModelSettings
): Promise<ModelSettings> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceModelsSettingsUpdate_unstable({
    modelId,
    settings,
  });
  return response.settings;
}

export async function searchHfModels(query: string, limit?: number): Promise<HfModelInfo[]> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceHuggingfaceSearch_unstable({ query, limit });
  return response.models;
}

export async function getRepoFiles(repoId: string): Promise<RepoVariantsResponse> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceHuggingfaceRepoVariants_unstable({ repoId });
  return {
    variants: response.variants,
    recommendedIndex: response.recommendedIndex ?? null,
    availableMemoryBytes: response.availableMemoryBytes,
    downloadedQuants: response.downloadedQuants,
    downloadedVariants: response.downloadedVariants,
  };
}

export async function listBuiltinChatTemplates(): Promise<string[]> {
  const client = await getAcpClient();
  const response = await client.goose.localInferenceChatTemplatesBuiltinList_unstable({});
  return response.templates;
}

import type { InitializeResponse } from '@agentclientprotocol/sdk';
import { getAcpInitializeResponse } from './acpConnection';

export interface AcpFeatureCapabilities {
  localInference: boolean;
  recipeParameterScopes: boolean;
  emptyExtensionSelection: boolean;
}

export async function getAcpFeatureCapabilities(): Promise<AcpFeatureCapabilities> {
  const initializeResponse = await getAcpInitializeResponse();

  return {
    localInference: hasLocalInferenceCapability(initializeResponse),
    recipeParameterScopes: hasRecipeParameterScopesCapability(initializeResponse),
    emptyExtensionSelection: hasEmptyExtensionSelectionCapability(initializeResponse),
  };
}

export function hasLocalInferenceCapability(
  initializeResponse: Pick<InitializeResponse, 'agentCapabilities'>
): boolean {
  const agentCapabilities = initializeResponse.agentCapabilities;
  if (!agentCapabilities) {
    return false;
  }

  const meta = agentCapabilities._meta;
  if (!isRecord(meta)) {
    return false;
  }

  const goose = meta.goose;
  if (!isRecord(goose)) {
    return false;
  }

  return 'localInference' in goose;
}

export function hasRecipeParameterScopesCapability(
  initializeResponse: Pick<InitializeResponse, 'agentCapabilities'>
): boolean {
  const agentCapabilities = initializeResponse.agentCapabilities;
  if (!agentCapabilities) {
    return false;
  }

  const meta = agentCapabilities._meta;
  if (!isRecord(meta)) {
    return false;
  }

  const goose = meta.goose;
  if (!isRecord(goose)) {
    return false;
  }

  return 'recipeParameterScopes' in goose;
}

export function hasEmptyExtensionSelectionCapability(
  initializeResponse: Pick<InitializeResponse, 'agentCapabilities'>
): boolean {
  const meta = initializeResponse.agentCapabilities?._meta;
  return isRecord(meta) && isRecord(meta.goose) && isRecord(meta.goose.emptyExtensionSelection);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

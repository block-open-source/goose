import type { InitializeResponse } from '@agentclientprotocol/sdk';
import { describe, expect, it } from 'vitest';
import {
  hasLocalInferenceCapability,
  hasRecipeParameterScopesCapability,
  hasEmptyExtensionSelectionCapability,
} from '../capabilities';

function initializeResponseWithMeta(meta?: unknown): Pick<InitializeResponse, 'agentCapabilities'> {
  return {
    agentCapabilities: {
      _meta: meta,
    },
  } as Pick<InitializeResponse, 'agentCapabilities'>;
}

describe('ACP capabilities', () => {
  it('requires advertised empty-selection support', () => {
    expect(
      hasEmptyExtensionSelectionCapability(
        initializeResponseWithMeta({
          goose: { emptyExtensionSelection: {} },
        })
      )
    ).toBe(true);
    for (const meta of [
      undefined,
      {},
      { goose: null },
      { goose: {} },
      { goose: { emptyExtensionSelection: false } },
      { goose: { emptyExtensionSelection: null } },
    ]) {
      expect(hasEmptyExtensionSelectionCapability(initializeResponseWithMeta(meta))).toBe(false);
    }
  });
  it('detects local inference support from Goose metadata', () => {
    expect(
      hasLocalInferenceCapability(
        initializeResponseWithMeta({
          goose: {
            localInference: {},
          },
        })
      )
    ).toBe(true);
  });

  it('detects scoped recipe-parameter support from Goose metadata', () => {
    expect(
      hasRecipeParameterScopesCapability(
        initializeResponseWithMeta({
          goose: {
            recipeParameterScopes: {},
          },
        })
      )
    ).toBe(true);
  });

  it('treats missing or malformed scoped recipe-parameter metadata as unsupported', () => {
    expect(hasRecipeParameterScopesCapability(initializeResponseWithMeta())).toBe(false);
    expect(hasRecipeParameterScopesCapability(initializeResponseWithMeta({}))).toBe(false);
    expect(hasRecipeParameterScopesCapability(initializeResponseWithMeta({ goose: {} }))).toBe(
      false
    );
    expect(hasRecipeParameterScopesCapability(initializeResponseWithMeta({ goose: true }))).toBe(
      false
    );
  });

  it('treats missing local inference metadata as unsupported', () => {
    expect(hasLocalInferenceCapability(initializeResponseWithMeta())).toBe(false);
    expect(hasLocalInferenceCapability(initializeResponseWithMeta({}))).toBe(false);
    expect(hasLocalInferenceCapability(initializeResponseWithMeta({ goose: {} }))).toBe(false);
  });

  it('ignores malformed Goose metadata', () => {
    expect(hasLocalInferenceCapability(initializeResponseWithMeta({ goose: true }))).toBe(false);
    expect(hasLocalInferenceCapability(initializeResponseWithMeta({ goose: null }))).toBe(false);
  });
});

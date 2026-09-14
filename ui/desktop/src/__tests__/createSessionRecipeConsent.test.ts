import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Recipe } from '../recipe';

const mocks = vi.hoisted(() => ({
  controllerCreateSession: vi.fn(),
  decodeRecipe: vi.fn(),
  scanRecipe: vi.fn(),
  listSavedRecipes: vi.fn(),
  requestRecipeConsent: vi.fn(),
  configuredRecipeParameters: vi.fn(),
  hasAcceptedRecipeBefore: vi.fn(),
  recordRecipeHash: vi.fn(),
}));

vi.mock('../acp/chatSessionController', () => ({
  acpChatSessionController: { createSession: mocks.controllerCreateSession },
}));
vi.mock('../acp/extensions', () => ({
  getConfiguredGooseExtensions: vi.fn(async () => []),
  gooseExtensionName: vi.fn(),
}));
vi.mock('../acp/capabilities', () => ({
  getAcpFeatureCapabilities: vi.fn(async () => ({ recipeParameterScopes: true })),
}));
vi.mock('../acp/recipeParamRequests', () => ({
  beginConfiguredRecipeParameterScope: vi.fn(() => undefined),
  configuredRecipeParameters: mocks.configuredRecipeParameters,
}));
vi.mock('../recipe', () => ({
  decodeRecipe: mocks.decodeRecipe,
  scanRecipe: mocks.scanRecipe,
}));
vi.mock('../recipe/recipe_management', () => ({
  listSavedRecipes: mocks.listSavedRecipes,
}));
vi.mock('../recipe/consent', () => ({
  requestRecipeConsent: mocks.requestRecipeConsent,
}));

import { createSession } from '../sessions';
import { isRecipeDeclined } from '../acp/errors';

const recipe: Recipe = {
  title: 'Helpful Data Analyzer',
  description: 'Shared recipe',
  extensions: [{ type: 'stdio', name: 'analyzer', cmd: 'sh', args: ['-c', 'id'] }],
};
const session = { id: 'session-1' };

describe('createSession recipe consent gate', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.assign(window.electron, {
      hasAcceptedRecipeBefore: mocks.hasAcceptedRecipeBefore,
      recordRecipeHash: mocks.recordRecipeHash,
    });
    mocks.controllerCreateSession.mockResolvedValue(session);
    mocks.decodeRecipe.mockResolvedValue(recipe);
    mocks.scanRecipe.mockResolvedValue({ has_security_warnings: false });
    mocks.listSavedRecipes.mockResolvedValue([{ id: 'saved-1', recipe }]);
    mocks.hasAcceptedRecipeBefore.mockResolvedValue(false);
    mocks.recordRecipeHash.mockResolvedValue(true);
    mocks.configuredRecipeParameters.mockReturnValue(undefined);
  });

  it('shows consent for a deeplink recipe before session/new and records acceptance', async () => {
    mocks.requestRecipeConsent.mockResolvedValue(true);
    mocks.configuredRecipeParameters.mockReturnValue({ script: 'ls' });

    const result = await createSession('/work', { recipeDeeplink: 'ENCODED' });

    expect(result).toBe(session);
    expect(mocks.decodeRecipe).toHaveBeenCalledWith('ENCODED');
    expect(mocks.requestRecipeConsent).toHaveBeenCalledWith(
      {
        recipe,
        hasSecurityWarnings: false,
        providedParameters: { script: 'ls' },
      },
      undefined
    );
    expect(mocks.hasAcceptedRecipeBefore).toHaveBeenCalledWith(recipe, { script: 'ls' });
    expect(mocks.recordRecipeHash).toHaveBeenCalledWith(recipe, { script: 'ls' });
    expect(mocks.controllerCreateSession).toHaveBeenCalledWith('/work', [], {
      recipeId: undefined,
      recipeDeeplink: 'ENCODED',
      recipeParameterScopeId: undefined,
    });
    expect(mocks.requestRecipeConsent.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.controllerCreateSession.mock.invocationCallOrder[0]
    );
  });

  it('never calls session/new when the user declines', async () => {
    mocks.requestRecipeConsent.mockResolvedValue(false);

    await expect(createSession('/work', { recipeDeeplink: 'ENCODED' })).rejects.toSatisfy(
      isRecipeDeclined
    );

    expect(mocks.controllerCreateSession).not.toHaveBeenCalled();
    expect(mocks.recordRecipeHash).not.toHaveBeenCalled();
  });

  it('skips the modal for a recipe accepted before', async () => {
    mocks.hasAcceptedRecipeBefore.mockResolvedValue(true);

    await createSession('/work', { recipeDeeplink: 'ENCODED' });

    expect(mocks.requestRecipeConsent).not.toHaveBeenCalled();
    expect(mocks.scanRecipe).not.toHaveBeenCalled();
    expect(mocks.controllerCreateSession).toHaveBeenCalledTimes(1);
  });

  it('re-prompts when a deeplink changes only a parameter on a trusted template', async () => {
    // Mirror the real trust store: acceptance is keyed on the recipe together with
    // the provided parameters, so a new parameter value is not covered by prior trust.
    const accepted = new Set<string>();
    const key = (r: unknown, params?: Record<string, string>) => JSON.stringify([r, params]);
    mocks.hasAcceptedRecipeBefore.mockImplementation(async (r, params) =>
      accepted.has(key(r, params))
    );
    mocks.recordRecipeHash.mockImplementation(async (r, params) => {
      accepted.add(key(r, params));
      return true;
    });
    mocks.requestRecipeConsent.mockResolvedValue(true);

    mocks.configuredRecipeParameters.mockReturnValue({ script: 'echo safe' });
    await createSession('/work', { recipeDeeplink: 'ENCODED' });
    expect(mocks.requestRecipeConsent).toHaveBeenCalledTimes(1);

    // Same template, same parameter: trusted, no prompt.
    await createSession('/work', { recipeDeeplink: 'ENCODED' });
    expect(mocks.requestRecipeConsent).toHaveBeenCalledTimes(1);

    // Same template, malicious parameter: must prompt again.
    mocks.configuredRecipeParameters.mockReturnValue({ script: 'rm -rf ~' });
    await createSession('/work', { recipeDeeplink: 'ENCODED' });
    expect(mocks.requestRecipeConsent).toHaveBeenCalledTimes(2);
  });

  it('resolves library recipes by id and passes hidden character warnings through', async () => {
    mocks.scanRecipe.mockResolvedValue({ has_security_warnings: true });
    mocks.requestRecipeConsent.mockResolvedValue(true);

    await createSession('/work', { recipeId: 'saved-1' });

    expect(mocks.decodeRecipe).not.toHaveBeenCalled();
    expect(mocks.requestRecipeConsent).toHaveBeenCalledWith(
      {
        recipe,
        hasSecurityWarnings: true,
        providedParameters: undefined,
      },
      undefined
    );
    expect(mocks.controllerCreateSession).toHaveBeenCalledWith(
      '/work',
      [],
      expect.objectContaining({ recipeId: 'saved-1' })
    );
  });

  it('fails before session/new when a library recipe id is unknown', async () => {
    await expect(createSession('/work', { recipeId: 'missing' })).rejects.toThrow(/not found/);
    expect(mocks.controllerCreateSession).not.toHaveBeenCalled();
  });

  it('does not touch recipe machinery for plain sessions', async () => {
    await createSession('/work');

    expect(mocks.decodeRecipe).not.toHaveBeenCalled();
    expect(mocks.listSavedRecipes).not.toHaveBeenCalled();
    expect(mocks.requestRecipeConsent).not.toHaveBeenCalled();
    expect(mocks.controllerCreateSession).toHaveBeenCalledTimes(1);
  });
});

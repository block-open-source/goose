import type { Session } from './types/session';
import type { ExtensionConfig } from './types/extensions';
import type { setViewType } from './hooks/useNavigation';
import type { FixedExtensionEntry } from './components/ConfigContext';
import { AppEvents } from './constants/events';
import { acpChatSessionController } from './acp/chatSessionController';
import { getConfiguredGooseExtensions, gooseExtensionName } from './acp/extensions';
import {
  beginConfiguredRecipeParameterScope,
  configuredRecipeParameters,
} from './acp/recipeParamRequests';
import { getAcpFeatureCapabilities } from './acp/capabilities';
import { RecipeParameterScopesUnsupportedError } from './acp/errors';
import { decodeRecipe, type Recipe } from './recipe';
import { listSavedRecipes } from './recipe/recipe_management';
import { ensureRecipeConsent } from './recipe/consentGate';

export function getSessionDisplayName(session: Session): string {
  if (session.user_set_name) {
    return session.name;
  }
  if (session.recipe?.title) {
    return session.recipe.title;
  }
  return session.name;
}

interface CreateSessionOptions {
  recipeDeeplink?: string;
  recipeId?: string;
  extensionConfigs?: ExtensionConfig[];
  allExtensions?: FixedExtensionEntry[];
}

function selectedExtensionConfigs(options?: CreateSessionOptions): ExtensionConfig[] {
  if (options?.extensionConfigs && options.extensionConfigs.length > 0) {
    return options.extensionConfigs;
  }
  if (options?.allExtensions) {
    return options.allExtensions
      .filter((extension) => extension.enabled)
      .map((extension) => {
        const { enabled: _enabled, ...config } = extension;
        return config as ExtensionConfig;
      });
  }
  return [];
}

async function resolveRecipe(options?: CreateSessionOptions): Promise<Recipe | undefined> {
  if (options?.recipeId) {
    const entry = (await listSavedRecipes()).find((manifest) => manifest.id === options.recipeId);
    if (!entry) {
      throw new Error(`Recipe ${options.recipeId} was not found in the recipe library`);
    }
    return entry.recipe;
  }
  if (options?.recipeDeeplink) {
    return decodeRecipe(options.recipeDeeplink);
  }
  return undefined;
}

async function createAcpSession(
  workingDir: string,
  options?: CreateSessionOptions
): Promise<Session> {
  // Recipes can declare commands, endpoints, and shell checks that run as soon as the
  // session exists, so consent has to be settled before session/new is ever sent.
  const recipe = await resolveRecipe(options);
  if (recipe) {
    await ensureRecipeConsent(
      recipe,
      options?.recipeDeeplink ? configuredRecipeParameters() : undefined
    );
  }

  const configuredParameterScope = options?.recipeDeeplink
    ? beginConfiguredRecipeParameterScope()
    : undefined;
  try {
    if (configuredParameterScope) {
      const capabilities = await getAcpFeatureCapabilities();
      if (!capabilities.recipeParameterScopes) {
        throw new RecipeParameterScopesUnsupportedError();
      }
    }
    const selectedNames = new Set(selectedExtensionConfigs(options).map((config) => config.name));
    const gooseExtensions =
      selectedNames.size > 0
        ? (await getConfiguredGooseExtensions())
            .filter((entry) => selectedNames.has(gooseExtensionName(entry.extension)))
            .map((entry) => entry.extension)
        : [];
    return await acpChatSessionController.createSession(workingDir, gooseExtensions, {
      recipeId: options?.recipeId,
      recipeDeeplink: options?.recipeDeeplink,
      recipeParameterScopeId: configuredParameterScope?.id,
    });
  } finally {
    configuredParameterScope?.finish();
  }
}

export async function createSession(
  workingDir: string,
  options?: CreateSessionOptions
): Promise<Session> {
  return createAcpSession(workingDir, options);
}

export async function startNewSession(
  initialText: string | undefined,
  setView: setViewType,
  workingDir: string,
  options?: {
    recipeDeeplink?: string;
    recipeId?: string;
    allExtensions?: FixedExtensionEntry[];
  }
): Promise<Session> {
  const session = await createSession(workingDir, options);
  window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED, { detail: { session } }));

  const initialMessage = initialText ? { msg: initialText, images: [] } : undefined;

  const eventDetail = {
    sessionId: session.id,
    initialMessage,
  };

  window.dispatchEvent(
    new CustomEvent(AppEvents.ADD_ACTIVE_SESSION, {
      detail: eventDetail,
    })
  );

  setView('pair', {
    disableAnimation: true,
    initialMessage,
    resumeSessionId: session.id,
  });
  return session;
}

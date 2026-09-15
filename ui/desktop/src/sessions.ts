import type { Session } from './types/session';
import type { ExtensionConfig } from './types/extensions';
import type { GooseExtension } from '@aaif/goose-acp-client';
import type { setViewType } from './hooks/useNavigation';
import type { FixedExtensionEntry } from './components/ConfigContext';
import { AppEvents } from './constants/events';
import { acpChatSessionController } from './acp/chatSessionController';
import { getConfiguredGooseExtensions, gooseExtensionName } from './acp/extensions';
import { beginConfiguredRecipeParameterScope } from './acp/recipeParamRequests';
import { getAcpFeatureCapabilities } from './acp/capabilities';
import { RecipeParameterScopesUnsupportedError } from './acp/errors';

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

/**
 * Three-valued on purpose. `undefined` means the caller is not naming a set and the
 * backend should use the configured one; `[]` means the user asked for a session with
 * no extensions at all. Collapsing the two is what made an all-off selection come back
 * with the default extensions.
 */
function selectedExtensionConfigs(options?: CreateSessionOptions): ExtensionConfig[] | undefined {
  if (options?.extensionConfigs) {
    return options.extensionConfigs;
  }
  if (options?.allExtensions) {
    const enabled = options.allExtensions
      .filter((extension) => extension.enabled)
      .map((extension) => {
        const { enabled: _enabled, ...config } = extension;
        return config as ExtensionConfig;
      });
    // An empty configured list is also what this looks like before the config
    // finishes loading, so it stays "not specified" rather than becoming an
    // explicit empty selection. Only `extensionConfigs` can express that.
    return enabled.length > 0 ? enabled : undefined;
  }
  return undefined;
}

async function resolveGooseExtensions(
  selected: ExtensionConfig[] | undefined
): Promise<GooseExtension[] | undefined> {
  if (selected === undefined) {
    return undefined;
  }
  if (selected.length === 0) {
    return [];
  }
  const selectedNames = new Set(selected.map((config) => config.name));
  return (await getConfiguredGooseExtensions())
    .filter((entry) => selectedNames.has(gooseExtensionName(entry.extension)))
    .map((entry) => entry.extension);
}

async function createAcpSession(
  workingDir: string,
  options?: CreateSessionOptions
): Promise<Session> {
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
    const gooseExtensions = await resolveGooseExtensions(selectedExtensionConfigs(options));
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

import { RecipeDeclinedError } from '../acp/errors';
import { scanRecipe, type Recipe } from '.';
import { requestRecipeConsent } from './consent';

// Recipes can declare commands, endpoints, and shell checks that run as soon as the
// session exists or a schedule fires, so consent has to be settled before either is
// created. Throws RecipeDeclinedError when the user declines and RecipeConsentAbortedError
// when the caller aborts (e.g. navigates away); resolves when the recipe is already
// trusted or freshly accepted.
//
// Trust is keyed on the recipe together with the parameter values that will be
// substituted into it, so changing a deeplink parameter re-prompts even when the
// template is unchanged.
export async function ensureRecipeConsent(
  recipe: Recipe,
  providedParameters?: Record<string, string>,
  signal?: globalThis.AbortSignal
): Promise<void> {
  if (await window.electron.hasAcceptedRecipeBefore(recipe, providedParameters)) {
    return;
  }

  const scan = await scanRecipe(recipe);
  const accepted = await requestRecipeConsent(
    {
      recipe,
      hasSecurityWarnings: scan.has_security_warnings,
      providedParameters,
    },
    signal
  );
  if (!accepted) {
    throw new RecipeDeclinedError();
  }
  await window.electron.recordRecipeHash(recipe, providedParameters);
}

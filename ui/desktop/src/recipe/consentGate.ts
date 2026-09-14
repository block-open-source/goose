import { RecipeDeclinedError } from '../acp/errors';
import { scanRecipe, type Recipe } from '.';
import { requestRecipeConsent } from './consent';

// Recipes can declare commands, endpoints, and shell checks that run as soon as the
// session exists or a schedule fires, so consent has to be settled before either is
// created. Throws RecipeDeclinedError when the user declines; resolves when the recipe
// is already trusted or freshly accepted.
export async function ensureRecipeConsent(
  recipe: Recipe,
  providedParameters?: Record<string, string>
): Promise<void> {
  if (await window.electron.hasAcceptedRecipeBefore(recipe)) {
    return;
  }

  const scan = await scanRecipe(recipe);
  const accepted = await requestRecipeConsent({
    recipe,
    hasSecurityWarnings: scan.has_security_warnings,
    providedParameters,
  });
  if (!accepted) {
    throw new RecipeDeclinedError();
  }
  await window.electron.recordRecipeHash(recipe);
}

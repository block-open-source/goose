import type {
  RecipeDto,
  RecipeExtensionDto,
  RecipeListEntryDto,
  RecipeParameterDto,
  RecipeSettingsDto,
} from '@aaif/goose-acp-client';
import {
  decodeRecipe as acpDecodeRecipe,
  encodeRecipe as acpEncodeRecipe,
  parseRecipe as acpParseRecipe,
  scanRecipe as acpScanRecipe,
} from '../acp/recipe';

export type Parameter = RecipeParameterDto;
export type RecipeExtension = RecipeExtensionDto;
export type RecipeSettings = RecipeSettingsDto;
export type Recipe = RecipeDto & {
  // TODO: Separate these from the raw recipe type
  // Properties added for scheduled execution
  scheduledJobId?: string;
  isScheduledExecution?: boolean;
};
export type RecipeManifest = Omit<RecipeListEntryDto, 'recipe'> & {
  recipe: Recipe;
};

export async function encodeRecipe(recipe: Recipe): Promise<string> {
  try {
    return await acpEncodeRecipe(recipe);
  } catch (error) {
    console.error('Failed to encode recipe:', error);
    throw error;
  }
}

export async function decodeRecipe(deeplink: string): Promise<Recipe> {
  try {
    return stripEmptyExtensions(await acpDecodeRecipe(deeplink));
  } catch (error) {
    console.error('Failed to decode deeplink:', error);
    throw error;
  }
}

export async function scanRecipe(recipe: Recipe): Promise<{ has_security_warnings: boolean }> {
  try {
    return await acpScanRecipe(recipe);
  } catch (error) {
    console.error('Failed to scan recipe:', error);
    throw error;
  }
}

export async function generateDeepLink(recipe: Recipe): Promise<string> {
  const encoded = await encodeRecipe(recipe);
  return `goose://recipe?config=${encoded}`;
}

/**
 * Strips empty extensions arrays from recipes before passing to the backend.
 *
 * This is a backwards compatibility workaround for the desktop app. Previously,
 * the UI was saving recipes with an empty `extensions: []` array, which the
 * backend interprets as "use no extensions" rather than "use user's default
 * extensions". By removing the empty array, the backend will fall back to
 * loading the user's configured default extensions.
 *
 * This can be removed once we have the ability to manage recipe extensions
 * directly in the UI, allowing users to explicitly choose which extensions
 * a recipe should use.
 */
export function stripEmptyExtensions(recipe: Recipe): Recipe {
  if (Array.isArray(recipe.extensions) && recipe.extensions.length === 0) {
    const { extensions: _, ...rest } = recipe;
    return rest as Recipe;
  }
  return recipe;
}

export async function parseRecipeFromFile(fileContent: string): Promise<Recipe> {
  try {
    return await acpParseRecipe(fileContent);
  } catch (error) {
    let errorMessage = 'unknown error';
    if (typeof error === 'object' && error !== null && 'message' in error) {
      errorMessage = error.message as string;
    }
    throw new Error(errorMessage, { cause: error });
  }
}

export async function parseDeeplink(deeplink: string): Promise<Recipe | null> {
  try {
    const cleanLink = deeplink.trim();

    if (!cleanLink.startsWith('goose://recipe?config=')) {
      throw new Error('Invalid deeplink format. Expected: goose://recipe?config=...');
    }

    const recipeEncoded = cleanLink.replace('goose://recipe?config=', '');

    if (!recipeEncoded) {
      throw new Error('No recipe configuration found in deeplink');
    }
    const recipe = await decodeRecipe(recipeEncoded);

    if (!recipe.title || !recipe.description) {
      throw new Error('Recipe is missing required fields (title, description)');
    }

    if (!recipe.instructions && !recipe.prompt) {
      throw new Error('Recipe must have either instructions or prompt');
    }

    return recipe;
  } catch (error) {
    console.error('Failed to parse deeplink:', error);
    return null;
  }
}

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RecipeDto } from '@aaif/goose-acp-client';
import { getAcpClient } from '../acpConnection';
import { encodeRecipe, listRecipes, parseRecipe, saveRecipe } from '../recipe';

vi.mock('../acpConnection', () => ({
  getAcpClient: vi.fn(),
}));

const recipe = {
  title: 'Test Recipe',
  description: 'A recipe used by ACP tests',
  instructions: 'Follow these test instructions',
} as RecipeDto;

function createClient() {
  return {
    goose: {
      recipesEncode_unstable: vi.fn(),
      recipesList_unstable: vi.fn(),
      recipesParse_unstable: vi.fn(),
      recipesSave_unstable: vi.fn(),
    },
  };
}

describe('ACP recipe helpers', () => {
  let client: ReturnType<typeof createClient>;

  beforeEach(() => {
    vi.clearAllMocks();
    client = createClient();
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );
  });

  it('surfaces ACP JSON-RPC error messages', async () => {
    client.goose.recipesEncode_unstable.mockRejectedValue({
      error: { message: 'recipe is invalid' },
    });

    await expect(encodeRecipe(recipe)).rejects.toThrow('recipe is invalid');
  });

  it('prefers ACP JSON-RPC error data over generic messages', async () => {
    client.goose.recipesSave_unstable.mockRejectedValue({
      error: {
        message: 'Invalid params',
        data: 'save recipe validation failed at recipe.extensions[0]: missing field `cmd`',
      },
    });

    await expect(saveRecipe(recipe)).rejects.toThrow(
      'save recipe validation failed at recipe.extensions[0]: missing field `cmd`'
    );
  });

  it('prefers ACP JSON-RPC error data from Error instances', async () => {
    client.goose.recipesParse_unstable.mockRejectedValue(
      Object.assign(new Error('Invalid params'), {
        error: {
          message: 'Invalid params',
          data: 'recipe: missing field `title`',
        },
      })
    );

    await expect(parseRecipe('description: Missing title')).rejects.toThrow(
      'recipe: missing field `title`'
    );
  });

  it('shares concurrent recipe list requests', async () => {
    const recipes = [
      {
        id: 'recipe-1',
        recipe,
      },
    ];
    client.goose.recipesList_unstable.mockResolvedValue({ recipes });

    const [first, second] = await Promise.all([listRecipes(), listRecipes()]);

    expect(client.goose.recipesList_unstable).toHaveBeenCalledTimes(1);
    expect(first).toBe(recipes);
    expect(second).toBe(recipes);
  });

  it('fetches recipes again after a list request settles', async () => {
    client.goose.recipesList_unstable.mockResolvedValue({ recipes: [] });

    await listRecipes();
    await listRecipes();

    expect(client.goose.recipesList_unstable).toHaveBeenCalledTimes(2);
  });
});

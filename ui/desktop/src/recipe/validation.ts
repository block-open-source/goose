import { zRecipeDto } from '@aaif/goose-acp-client';
import { z } from 'zod';

type JsonSchema = Record<string, unknown>;

const recipeDescription =
  'A Recipe represents a reusable agent configuration with instructions, optional prompt, parameters, supported extensions, settings, and subrecipes.';

let recipeJsonSchema: JsonSchema | null = null;

export function getRecipeJsonSchema(): JsonSchema {
  if (!recipeJsonSchema) {
    recipeJsonSchema = {
      ...(z.toJSONSchema(zRecipeDto, { target: 'draft-07', reused: 'inline' }) as JsonSchema),
      title: 'Recipe',
      description: recipeDescription,
    };
  }

  return recipeJsonSchema;
}

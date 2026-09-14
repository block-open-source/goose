import { Recipe } from '../recipe';
import type { Message } from './message';

export type TokenState = {
  accumulatedCacheReadTokens?: number;
  accumulatedCacheWriteTokens?: number;
  accumulatedCost?: number | null;
  accumulatedInputTokens: number;
  accumulatedOutputTokens: number;
  accumulatedTotalTokens: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  contextLimit?: number;
};

export interface ChatType {
  sessionId: string;
  name: string;
  messages: Message[];
  recipe?: Recipe | null; // Add recipe configuration to chat state
  resolvedRecipe?: Recipe | null; // Add resolved recipe with parameter values rendered to chat state
  recipeParameterValues?: Record<string, string> | null; // Add recipe parameters to chat state
}

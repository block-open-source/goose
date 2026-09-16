import { describe, expect, it } from 'vitest';
import type { Recipe } from '.';
import {
  getRecipeConsentRequestsSnapshot,
  requestRecipeConsent,
  resolveRecipeConsent,
  subscribeRecipeConsentRequests,
} from './consent';

const recipe: Recipe = { title: 'Analyzer', description: 'Shared recipe' };

describe('recipe consent store', () => {
  it('publishes a pending request until it is resolved, then settles the promise', async () => {
    const notified: number[] = [];
    const unsubscribe = subscribeRecipeConsentRequests(() =>
      notified.push(getRecipeConsentRequestsSnapshot().length)
    );

    const decision = requestRecipeConsent({ recipe, hasSecurityWarnings: false });
    const [pending] = getRecipeConsentRequestsSnapshot();
    expect(pending.recipe).toBe(recipe);

    resolveRecipeConsent(pending.id, true);
    await expect(decision).resolves.toBe(true);
    expect(getRecipeConsentRequestsSnapshot()).toEqual([]);
    expect(notified).toEqual([1, 0]);
    unsubscribe();
  });
});

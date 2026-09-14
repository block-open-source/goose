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
  it('exposes a pending request until it is resolved, then settles the promise', async () => {
    const notified: number[] = [];
    const unsubscribe = subscribeRecipeConsentRequests(() =>
      notified.push(getRecipeConsentRequestsSnapshot().length)
    );

    const decision = requestRecipeConsent({ recipe, hasSecurityWarnings: false });
    const [pending] = getRecipeConsentRequestsSnapshot();
    expect(pending.recipe).toBe(recipe);

    expect(resolveRecipeConsent(pending.id, true)).toBe(true);
    await expect(decision).resolves.toBe(true);
    expect(getRecipeConsentRequestsSnapshot()).toEqual([]);
    expect(notified).toEqual([1, 0]);
    unsubscribe();
  });

  it('resolves false when the user declines', async () => {
    const decision = requestRecipeConsent({ recipe, hasSecurityWarnings: true });
    const [pending] = getRecipeConsentRequestsSnapshot();

    resolveRecipeConsent(pending.id, false);
    await expect(decision).resolves.toBe(false);
  });

  it('ignores unknown or already-resolved ids', () => {
    expect(resolveRecipeConsent('missing', true)).toBe(false);
  });
});

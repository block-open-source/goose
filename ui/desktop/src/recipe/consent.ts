import { v7 as uuidv7 } from 'uuid';
import type { Recipe } from '.';

export interface RecipeConsentRequest {
  id: string;
  recipe: Recipe;
  hasSecurityWarnings: boolean;
}

interface PendingConsent {
  request: RecipeConsentRequest;
  resolve: (accepted: boolean) => void;
}

const pendingRequests = new Map<string, PendingConsent>();
const listeners = new Set<() => void>();
let snapshot: RecipeConsentRequest[] = [];

function emit(): void {
  snapshot = Array.from(pendingRequests.values(), (pending) => pending.request);
  for (const listener of listeners) {
    listener();
  }
}

export function subscribeRecipeConsentRequests(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getRecipeConsentRequestsSnapshot(): RecipeConsentRequest[] {
  return snapshot;
}

export function requestRecipeConsent(
  input: Omit<RecipeConsentRequest, 'id'>
): Promise<boolean> {
  const request: RecipeConsentRequest = { id: `recipe_consent_${uuidv7()}`, ...input };
  return new Promise<boolean>((resolve) => {
    pendingRequests.set(request.id, { request, resolve });
    emit();
  });
}

export function resolveRecipeConsent(id: string, accepted: boolean): void {
  const pending = pendingRequests.get(id);
  if (!pending) {
    return;
  }
  pendingRequests.delete(id);
  emit();
  pending.resolve(accepted);
}

import { v7 as uuidv7 } from 'uuid';
import type { Recipe } from '.';
import { RecipeConsentAbortedError } from '../acp/errors';

export interface RecipeConsentRequest {
  id: string;
  recipe: Recipe;
  hasSecurityWarnings: boolean;
  providedParameters?: Record<string, string>;
}

interface PendingConsent {
  request: RecipeConsentRequest;
  settle: (accepted: boolean) => void;
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
  input: Omit<RecipeConsentRequest, 'id'>,
  signal?: globalThis.AbortSignal
): Promise<boolean> {
  const request: RecipeConsentRequest = { id: `recipe_consent_${uuidv7()}`, ...input };
  return new Promise<boolean>((resolve, reject) => {
    if (signal?.aborted) {
      reject(new RecipeConsentAbortedError());
      return;
    }

    const onAbort = () => {
      if (pendingRequests.delete(request.id)) {
        emit();
        reject(new RecipeConsentAbortedError());
      }
    };

    const settle = (accepted: boolean) => {
      signal?.removeEventListener('abort', onAbort);
      resolve(accepted);
    };

    pendingRequests.set(request.id, { request, settle });
    signal?.addEventListener('abort', onAbort);
    emit();
  });
}

export function resolveRecipeConsent(id: string, accepted: boolean): boolean {
  const pending = pendingRequests.get(id);
  if (!pending) {
    return false;
  }
  pendingRequests.delete(id);
  emit();
  pending.settle(accepted);
  return true;
}

import { useSyncExternalStore } from 'react';
import {
  getRecipeConsentRequestsSnapshot,
  resolveRecipeConsent,
  subscribeRecipeConsentRequests,
} from '../recipe/consent';
import { RecipeWarningModal } from './ui/RecipeWarningModal';

export default function RecipeConsentModalContainer() {
  const requests = useSyncExternalStore(
    subscribeRecipeConsentRequests,
    getRecipeConsentRequestsSnapshot
  );
  const request = requests[0];
  if (!request) {
    return null;
  }

  return (
    <RecipeWarningModal
      key={request.id}
      isOpen
      recipeDetails={{
        title: request.recipe.title,
        description: request.recipe.description,
        instructions: request.recipe.instructions || undefined,
      }}
      hasSecurityWarnings={request.hasSecurityWarnings}
      onConfirm={() => resolveRecipeConsent(request.id, true)}
      onCancel={() => resolveRecipeConsent(request.id, false)}
    />
  );
}

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
      recipe={request.recipe}
      hasSecurityWarnings={request.hasSecurityWarnings}
      providedParameters={request.providedParameters}
      onConfirm={() => resolveRecipeConsent(request.id, true)}
      onCancel={() => resolveRecipeConsent(request.id, false)}
    />
  );
}

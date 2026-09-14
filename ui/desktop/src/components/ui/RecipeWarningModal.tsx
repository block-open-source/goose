import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogPortal,
  DialogOverlay,
  DialogPointerEventsRecovery,
} from './dialog';
import * as DialogPrimitive from '@radix-ui/react-dialog';
import { Button } from './button';
import MarkdownContent from '../MarkdownContent';
import { cn } from '../../utils';
import { defineMessages, useIntl } from '../../i18n';

const i18n = defineMessages({
  securityWarningTitle: {
    id: 'recipeWarningModal.securityWarningTitle',
    defaultMessage: '⚠️ Security Warning',
  },
  newRecipeWarningTitle: {
    id: 'recipeWarningModal.newRecipeWarningTitle',
    defaultMessage: '⚠️ New Recipe Warning',
  },
  firstTimeDescription: {
    id: 'recipeWarningModal.firstTimeDescription',
    defaultMessage: "You are about to execute a recipe that you haven't run before. ",
  },
  trustSource: {
    id: 'recipeWarningModal.trustSource',
    defaultMessage: 'Only proceed if you trust the source of this recipe.',
  },
  hiddenCharsWarning: {
    id: 'recipeWarningModal.hiddenCharsWarning',
    defaultMessage:
      'This recipe contains hidden characters that will be ignored for your safety, as they could be used for malicious purposes.',
  },
  recipePreview: {
    id: 'recipeWarningModal.recipePreview',
    defaultMessage: 'Recipe Preview:',
  },
  titleLabel: {
    id: 'recipeWarningModal.titleLabel',
    defaultMessage: 'Title:',
  },
  descriptionLabel: {
    id: 'recipeWarningModal.descriptionLabel',
    defaultMessage: 'Description:',
  },
  instructionsLabel: {
    id: 'recipeWarningModal.instructionsLabel',
    defaultMessage: 'Instructions:',
  },
  cancel: {
    id: 'recipeWarningModal.cancel',
    defaultMessage: 'Cancel',
  },
  trustAndExecute: {
    id: 'recipeWarningModal.trustAndExecute',
    defaultMessage: 'Trust and Execute',
  },
});

interface RecipeWarningModalProps {
  isOpen: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  recipeDetails: {
    title?: string;
    description?: string;
    instructions?: string;
  };
  hasSecurityWarnings?: boolean;
}

export function RecipeWarningModal({
  isOpen,
  onConfirm,
  onCancel,
  recipeDetails,
  hasSecurityWarnings = false,
}: RecipeWarningModalProps) {
  const intl = useIntl();

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onCancel()}>
      <DialogPortal>
        <DialogOverlay />
        <DialogPrimitive.Content
          className={cn(
            'bg-background-primary data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 fixed top-[50%] left-[50%] z-50 grid w-full max-w-[calc(100%-2rem)] translate-x-[-50%] translate-y-[-50%] gap-4 rounded-lg border p-6 shadow-lg duration-200 sm:max-w-[80vw] max-h-[80vh] flex flex-col p-0'
          )}
          onPointerDownOutside={(e) => e.preventDefault()}
          onEscapeKeyDown={(e) => e.preventDefault()}
        >
          <DialogPointerEventsRecovery />
          <DialogHeader className="flex-shrink-0 p-6 pb-0">
            <DialogTitle>
              {hasSecurityWarnings
                ? intl.formatMessage(i18n.securityWarningTitle)
                : intl.formatMessage(i18n.newRecipeWarningTitle)}
            </DialogTitle>
            <DialogDescription>
              {!hasSecurityWarnings && intl.formatMessage(i18n.firstTimeDescription)}
              {intl.formatMessage(i18n.trustSource)}
            </DialogDescription>
          </DialogHeader>

          {hasSecurityWarnings && (
            <div className="px-6">
              <div className="bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-lg p-4">
                <div className="flex items-start">
                  <div className="ml-3">
                    <div className="mt-2 text-sm text-yellow-700 dark:text-yellow-300">
                      <p>{intl.formatMessage(i18n.hiddenCharsWarning)}</p>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          )}

          <div className="flex-1 overflow-y-auto p-6 pt-4">
            <div className="bg-background-secondary p-4 rounded-lg">
              <h3 className="font-medium mb-3 text-text-primary">
                {intl.formatMessage(i18n.recipePreview)}
              </h3>
              <div className="space-y-4">
                {recipeDetails.title && (
                  <p className="text-text-primary">
                    <strong>{intl.formatMessage(i18n.titleLabel)}</strong> {recipeDetails.title}
                  </p>
                )}
                {recipeDetails.description && (
                  <p className="text-text-primary">
                    <strong>{intl.formatMessage(i18n.descriptionLabel)}</strong>{' '}
                    {recipeDetails.description}
                  </p>
                )}
                {recipeDetails.instructions && (
                  <div>
                    <h4 className="font-medium text-text-primary mb-1">
                      {intl.formatMessage(i18n.instructionsLabel)}
                    </h4>
                    <MarkdownContent content={recipeDetails.instructions} className="text-sm" />
                  </div>
                )}
              </div>
            </div>
          </div>

          <DialogFooter className="flex-shrink-0 p-6 pt-0">
            <Button variant="outline" onClick={onCancel}>
              {intl.formatMessage(i18n.cancel)}
            </Button>
            <Button onClick={onConfirm}>{intl.formatMessage(i18n.trustAndExecute)}</Button>
          </DialogFooter>
        </DialogPrimitive.Content>
      </DialogPortal>
    </Dialog>
  );
}

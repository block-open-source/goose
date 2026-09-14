import { useState, useCallback } from 'react';
import { Button } from '../../../ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../../../ui/dialog';
import { ExtensionFormData } from '../utils';
import EnvVarsSection from './EnvVarsSection';
import HeadersSection from './HeadersSection';
import ExtensionConfigFields from './ExtensionConfigFields';
import { PlusIcon, Edit, Trash2, AlertTriangle, Info } from 'lucide-react';
import ExtensionInfoFields from './ExtensionInfoFields';
import ExtensionTimeoutField from './ExtensionTimeoutField';
import { acpUpsertConfig } from '../../../../acp/config';
import { ConfirmationModal } from '../../../ui/ConfirmationModal';
import { defineMessages, useIntl } from '../../../../i18n';

const i18n = defineMessages({
  deleteExtensionTitle: {
    id: 'extensionModal.deleteExtensionTitle',
    defaultMessage: 'Delete Extension "{name}"',
  },
  deleteDescription: {
    id: 'extensionModal.deleteDescription',
    defaultMessage: 'This will permanently remove this extension and all of its settings.',
  },
  installationNotes: {
    id: 'extensionModal.installationNotes',
    defaultMessage: 'Installation Notes',
  },
  cancel: {
    id: 'extensionModal.cancel',
    defaultMessage: 'Cancel',
  },
  confirmRemoval: {
    id: 'extensionModal.confirmRemoval',
    defaultMessage: 'Confirm removal',
  },
  removeExtension: {
    id: 'extensionModal.removeExtension',
    defaultMessage: 'Remove extension',
  },
  unsavedChangesTitle: {
    id: 'extensionModal.unsavedChangesTitle',
    defaultMessage: 'Unsaved Changes',
  },
  unsavedChangesMessage: {
    id: 'extensionModal.unsavedChangesMessage',
    defaultMessage:
      'You have unsaved changes to the extension configuration. Are you sure you want to close without saving?',
  },
  closeWithoutSaving: {
    id: 'extensionModal.closeWithoutSaving',
    defaultMessage: 'Close Without Saving',
  },
});

interface ExtensionModalProps {
  title: string;
  initialData: ExtensionFormData;
  onClose: () => void;
  onSubmit: (formData: ExtensionFormData) => void;
  onDelete?: (name: string) => void;
  submitLabel: string;
  modalType: 'add' | 'edit';
}

export default function ExtensionModal({
  title,
  initialData,
  onClose,
  onSubmit,
  onDelete,
  submitLabel,
  modalType,
}: ExtensionModalProps) {
  const intl = useIntl();
  const [formData, setFormData] = useState<ExtensionFormData>(initialData);
  const [showDeleteConfirmation, setShowDeleteConfirmation] = useState(false);
  const [submitAttempted, setSubmitAttempted] = useState(false);
  const [showCloseConfirmation, setShowCloseConfirmation] = useState(false);
  const [hasPendingEnvVars, setHasPendingEnvVars] = useState(false);
  const [pendingEnvVar, setPendingEnvVar] = useState<{ key: string; value: string } | null>(null);
  const [hasPendingHeaders, setHasPendingHeaders] = useState(false);
  const [pendingHeader, setPendingHeader] = useState<{ key: string; value: string } | null>(null);

  // Function to check if form has been modified
  const hasFormChanges = (): boolean => {
    // Check basic fields
    const nameChanged = formData.name !== initialData.name;
    const descriptionChanged = formData.description !== initialData.description;
    const typeChanged = formData.type !== initialData.type;
    const timeoutChanged = formData.timeout !== initialData.timeout;

    // Check if command/endpoint has changed
    const commandChanged =
      (formData.type === 'stdio' && formData.cmd !== initialData.cmd) ||
      (formData.type === 'streamable_http' && formData.endpoint !== initialData.endpoint);

    // Check if headers have changed
    const headersEdited = formData.headers.some((header) => header.isEdited === true);
    const headersAdded = formData.headers.length > initialData.headers.length;
    const headersRemoved = formData.headers.length < initialData.headers.length;

    // Check if any environment variables have been modified
    const envVarsChanged = formData.envVars.some((envVar) => envVar.isEdited === true);
    const envVarsAdded = formData.envVars.length > initialData.envVars.length;
    const envVarsRemoved = formData.envVars.length < initialData.envVars.length;

    // Check if there are pending environment variables or headers being typed
    const hasPendingInput = hasPendingEnvVars || hasPendingHeaders;

    return (
      nameChanged ||
      descriptionChanged ||
      typeChanged ||
      timeoutChanged ||
      commandChanged ||
      headersEdited ||
      headersAdded ||
      headersRemoved ||
      envVarsChanged ||
      envVarsAdded ||
      envVarsRemoved ||
      hasPendingInput
    );
  };

  // Handle backdrop close with confirmation if needed
  const handleClose = () => {
    if (hasFormChanges()) {
      setShowCloseConfirmation(true);
    } else {
      onClose();
    }
  };

  // Handle confirmed close
  const handleConfirmClose = () => {
    setShowCloseConfirmation(false);
    onClose();
  };

  // Handle cancel close confirmation
  const handleCancelClose = () => {
    setShowCloseConfirmation(false);
  };

  const handleAddEnvVar = (key: string, value: string) => {
    setFormData({
      ...formData,
      envVars: [...formData.envVars, { key, value, isEdited: true }],
    });
  };

  const handleRemoveEnvVar = (index: number) => {
    const newEnvVars = [...formData.envVars];
    newEnvVars.splice(index, 1);
    setFormData({
      ...formData,
      envVars: newEnvVars,
    });
  };

  const handleEnvVarChange = (index: number, field: 'key' | 'value', value: string) => {
    const newEnvVars = [...formData.envVars];
    newEnvVars[index][field] = value;

    // Mark as edited if it's a value change
    if (field === 'value') {
      newEnvVars[index].isEdited = true;
    }

    setFormData({
      ...formData,
      envVars: newEnvVars,
    });
  };

  const handleAddHeader = (key: string, value: string) => {
    setFormData({
      ...formData,
      headers: [...formData.headers, { key, value, isEdited: true }],
    });
  };

  const handleRemoveHeader = (index: number) => {
    const newHeaders = [...formData.headers];
    newHeaders.splice(index, 1);
    setFormData({
      ...formData,
      headers: newHeaders,
    });
  };

  const handleHeaderChange = (index: number, field: 'key' | 'value', value: string) => {
    if (field === 'key') {
      if (value.includes(' ')) {
        return;
      }
      const trimmedNewKey = value.trim();
      const normalizedNewKey = trimmedNewKey.toLowerCase();
      const isDuplicate = formData.headers.some(
        (h, i) => i !== index && h.key.trim().toLowerCase() === normalizedNewKey
      );
      if (isDuplicate && trimmedNewKey !== '') {
        return;
      }
    }
    const newHeaders = [...formData.headers];
    newHeaders[index][field] = value;

    // Mark as edited if it's a value change
    if (field === 'value') {
      newHeaders[index].isEdited = true;
    }

    setFormData({
      ...formData,
      headers: newHeaders,
    });
  };

  const handlePendingHeaderChange = useCallback(
    (hasPending: boolean, header: { key: string; value: string } | null) => {
      setHasPendingHeaders(hasPending);
      setPendingHeader(header);
    },
    []
  );

  const handlePendingEnvVarChange = useCallback(
    (hasPending: boolean, envVar: { key: string; value: string } | null) => {
      setHasPendingEnvVars(hasPending);
      setPendingEnvVar(envVar);
    },
    []
  );

  // Function to store a secret value
  const storeSecret = async (key: string, value: string) => {
    try {
      await acpUpsertConfig(key, value, true);
      return true;
    } catch (error) {
      console.error('Failed to store secret:', error);
      return false;
    }
  };

  // Function to determine which icon to display with proper styling
  const getModalIcon = () => {
    if (showDeleteConfirmation) {
      return <AlertTriangle className="text-red-500" size={24} />;
    }
    return modalType === 'add' ? (
      <PlusIcon className="text-iconStandard" size={24} />
    ) : (
      <Edit className="text-iconStandard" size={24} />
    );
  };

  const isNameValid = () => {
    return formData.name.trim() !== '';
  };

  const isConfigValid = () => {
    return (
      (formData.type === 'stdio' && !!formData.cmd && formData.cmd.trim() !== '') ||
      (formData.type === 'streamable_http' &&
        !!formData.endpoint &&
        formData.endpoint.trim() !== '')
    );
  };

  const isEnvVarsValid = () => {
    return formData.envVars.every(
      ({ key, value }) => (key === '' && value === '') || (key !== '' && value !== '')
    );
  };

  const getFinalHeaders = () => {
    const finalHeaders = [...formData.headers];
    if (pendingHeader && pendingHeader.key.trim() !== '' && pendingHeader.value.trim() !== '') {
      finalHeaders.push({ ...pendingHeader, isEdited: true });
    }
    return finalHeaders;
  };

  const getFinalEnvVars = () => {
    const finalEnvVars = [...formData.envVars];
    if (
      pendingEnvVar &&
      pendingEnvVar.key.trim() !== '' &&
      pendingEnvVar.value.trim() !== '' &&
      !pendingEnvVar.key.includes(' ')
    ) {
      finalEnvVars.push({ ...pendingEnvVar, isEdited: true });
    }
    return finalEnvVars;
  };

  const isHeadersValid = () => {
    return getFinalHeaders().every(
      ({ key, value }) => (key === '' && value === '') || (key !== '' && value !== '')
    );
  };

  const isTimeoutValid = () => {
    // Check if timeout is not undefined, null, or empty string
    if (formData.timeout === undefined || formData.timeout === null) {
      return false;
    }

    // Convert to number if it's a string
    const timeoutValue =
      typeof formData.timeout === 'string' ? Number(formData.timeout) : formData.timeout;

    // Check if it's a valid number (not NaN) and is a positive number
    return !isNaN(timeoutValue) && timeoutValue > 0;
  };

  // Form validation
  const isFormValid = () => {
    return (
      isNameValid() && isConfigValid() && isEnvVarsValid() && isHeadersValid() && isTimeoutValid()
    );
  };

  // Handle submit with validation and secret storage
  const handleSubmit = async () => {
    setSubmitAttempted(true);

    if (isFormValid()) {
      const finalFormData = {
        ...formData,
        envVars: getFinalEnvVars(),
        headers: getFinalHeaders(),
      };

      // Only store env vars that have been edited (which includes new)
      const secretPromises = finalFormData.envVars
        .filter((envVar) => envVar.isEdited)
        .map(({ key, value }) => storeSecret(key, value));

      try {
        // Wait for all secrets to be stored
        const results = await Promise.all(secretPromises);

        if (results.every((success) => success)) {
          // Convert timeout to number if needed
          const dataToSubmit = {
            ...finalFormData,
            timeout:
              typeof finalFormData.timeout === 'string'
                ? Number(finalFormData.timeout)
                : finalFormData.timeout,
          };
          onSubmit(dataToSubmit);
          onClose();
        } else {
          console.error('Failed to store one or more secrets');
        }
      } catch (error) {
        console.error('Error during submission:', error);
      }
    }
  };

  // Update title based on current state
  const modalTitle = showDeleteConfirmation
    ? intl.formatMessage(i18n.deleteExtensionTitle, { name: formData.name })
    : title;

  return (
    <>
      <Dialog open={true} onOpenChange={handleClose}>
        <DialogContent className="sm:max-w-[600px] max-h-[90vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              {getModalIcon()}
              {modalTitle}
            </DialogTitle>
            {showDeleteConfirmation && (
              <DialogDescription>{intl.formatMessage(i18n.deleteDescription)}</DialogDescription>
            )}
          </DialogHeader>

          {showDeleteConfirmation ? (
            <div className="py-4">
              <p className="text-text-primary">{intl.formatMessage(i18n.deleteDescription)}</p>
            </div>
          ) : (
            <div className="py-4 space-y-6">
              {formData.installation_notes && (
                <div className="bg-background-secondary border border-border-primary rounded-lg p-4">
                  <div className="flex items-start gap-2">
                    <Info className="h-5 w-5 text-blue-400 shrink-0 mt-0.5" />
                    <div>
                      <h4 className="text-sm font-medium text-text-primary mb-1">
                        {intl.formatMessage(i18n.installationNotes)}
                      </h4>
                      <p className="text-sm text-text-secondary">{formData.installation_notes}</p>
                    </div>
                  </div>
                </div>
              )}

              {/* Form Fields */}
              {/* Name and Type */}
              <ExtensionInfoFields
                name={formData.name}
                type={formData.type}
                description={formData.description}
                onChange={(key, value) => setFormData({ ...formData, [key]: value })}
                submitAttempted={submitAttempted}
              />

              <hr className="border-t border-border-primary" />

              {/* Command */}
              <div>
                <ExtensionConfigFields
                  type={formData.type}
                  full_cmd={formData.cmd || ''}
                  endpoint={formData.endpoint || ''}
                  onChange={(key, value) => setFormData({ ...formData, [key]: value })}
                  submitAttempted={submitAttempted}
                  isValid={isConfigValid()}
                />
                <div className="mb-4" />
                <ExtensionTimeoutField
                  timeout={formData.timeout || 300}
                  onChange={(key, value) => setFormData({ ...formData, [key]: value })}
                  submitAttempted={submitAttempted}
                />
              </div>

              {(formData.type === 'stdio' || formData.type === 'streamable_http') && (
                <>
                  <hr className="border-t border-border-primary" />

                  <div>
                    <EnvVarsSection
                      envVars={formData.envVars}
                      onAdd={handleAddEnvVar}
                      onRemove={handleRemoveEnvVar}
                      onChange={handleEnvVarChange}
                      submitAttempted={submitAttempted}
                      onPendingInputChange={handlePendingEnvVarChange}
                    />
                  </div>
                </>
              )}

              {formData.type === 'streamable_http' && (
                <>
                  {/* Divider */}
                  <hr className="border-t border-border-primary" />

                  <div>
                    <HeadersSection
                      headers={formData.headers}
                      onAdd={handleAddHeader}
                      onRemove={handleRemoveHeader}
                      onChange={handleHeaderChange}
                      submitAttempted={submitAttempted}
                      onPendingInputChange={handlePendingHeaderChange}
                    />
                  </div>
                </>
              )}
            </div>
          )}

          <DialogFooter className="pt-2">
            {showDeleteConfirmation ? (
              <>
                <Button variant="outline" onClick={() => setShowDeleteConfirmation(false)}>
                  {intl.formatMessage(i18n.cancel)}
                </Button>
                <Button
                  onClick={() => {
                    if (onDelete) {
                      onDelete(formData.name);
                      onClose();
                    }
                  }}
                  variant="destructive"
                >
                  <Trash2 className="h-4 w-4 mr-2" />
                  {intl.formatMessage(i18n.confirmRemoval)}
                </Button>
              </>
            ) : (
              <>
                {modalType === 'edit' && onDelete && (
                  <Button
                    onClick={() => setShowDeleteConfirmation(true)}
                    variant="outline"
                    className="text-red-500 hover:text-red-600"
                  >
                    <Trash2 className="h-4 w-4 mr-2" />
                    {intl.formatMessage(i18n.removeExtension)}
                  </Button>
                )}
                <Button variant="outline" onClick={handleClose}>
                  {intl.formatMessage(i18n.cancel)}
                </Button>
                <Button
                  data-testid="extension-submit-btn"
                  onClick={handleSubmit}
                  disabled={!isFormValid()}
                >
                  {submitLabel}
                </Button>
              </>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Close Confirmation Modal */}
      {showCloseConfirmation && (
        <ConfirmationModal
          isOpen={showCloseConfirmation}
          title={intl.formatMessage(i18n.unsavedChangesTitle)}
          message={intl.formatMessage(i18n.unsavedChangesMessage)}
          confirmLabel={intl.formatMessage(i18n.closeWithoutSaving)}
          onConfirm={handleConfirmClose}
          onCancel={handleCancelClose}
        />
      )}
    </>
  );
}

import React, { useState, useEffect, useRef } from 'react';
import { Input } from '../../../../../ui/input';
import { Select } from '../../../../../ui/Select';
import { Button } from '../../../../../ui/button';
import { SecureStorageNotice } from '../SecureStorageNotice';
import type { UpdateCustomProviderRequest } from '../../../../../../types/providers';
import type { ProviderTemplateDto } from '@aaif/goose-acp-client';
import { Plus, X, Trash2, AlertTriangle, ExternalLink, Search, Settings } from 'lucide-react';
import { cn } from '../../../../../../utils';
import ProviderCatalogPicker from '../ProviderCatalogPicker';
import { defineMessages, useIntl } from '../../../../../../i18n';

const i18n = defineMessages({
  chooseSetup: {
    id: 'customProviderForm.chooseSetup',
    defaultMessage: "Choose how you'd like to set up your provider.",
  },
  startFromTemplate: {
    id: 'customProviderForm.startFromTemplate',
    defaultMessage: 'Start from a provider template',
  },
  startFromTemplateDesc: {
    id: 'customProviderForm.startFromTemplateDesc',
    defaultMessage: "Pick a known provider and we'll auto-fill the configuration",
  },
  configureManually: {
    id: 'customProviderForm.configureManually',
    defaultMessage: 'Configure manually',
  },
  configureManuallyDesc: {
    id: 'customProviderForm.configureManuallyDesc',
    defaultMessage: 'Enter all provider details yourself',
  },
  cancel: {
    id: 'customProviderForm.cancel',
    defaultMessage: 'Cancel',
  },
  back: {
    id: 'customProviderForm.back',
    defaultMessage: '← Back',
  },
  usingTemplate: {
    id: 'customProviderForm.usingTemplate',
    defaultMessage: 'Using template: {name}',
  },
  docs: {
    id: 'customProviderForm.docs',
    defaultMessage: 'Docs',
  },
  clear: {
    id: 'customProviderForm.clear',
    defaultMessage: 'Clear',
  },
  providerType: {
    id: 'customProviderForm.providerType',
    defaultMessage: 'Provider Type',
  },
  openaiCompatible: {
    id: 'customProviderForm.openaiCompatible',
    defaultMessage: 'OpenAI Compatible',
  },
  anthropicCompatible: {
    id: 'customProviderForm.anthropicCompatible',
    defaultMessage: 'Anthropic Compatible',
  },
  ollamaCompatible: {
    id: 'customProviderForm.ollamaCompatible',
    defaultMessage: 'Ollama Compatible',
  },
  displayName: {
    id: 'customProviderForm.displayName',
    defaultMessage: 'Display Name',
  },
  displayNamePlaceholder: {
    id: 'customProviderForm.displayNamePlaceholder',
    defaultMessage: 'Your Provider Name',
  },
  apiUrl: {
    id: 'customProviderForm.apiUrl',
    defaultMessage: 'API URL',
  },
  apiUrlPlaceholder: {
    id: 'customProviderForm.apiUrlPlaceholder',
    defaultMessage: 'https://api.example.com',
  },
  apiBasePath: {
    id: 'customProviderForm.apiBasePath',
    defaultMessage: 'API Base Path (optional)',
  },
  apiBasePathPlaceholder: {
    id: 'customProviderForm.apiBasePathPlaceholder',
    defaultMessage: 'e.g., v1/chat/completions or project_id/v1',
  },
  apiBasePathHint: {
    id: 'customProviderForm.apiBasePathHint',
    defaultMessage:
      "Override the default API path. Leave blank to use the provider's default path.",
  },
  authentication: {
    id: 'customProviderForm.authentication',
    defaultMessage: 'Authentication',
  },
  authHint: {
    id: 'customProviderForm.authHint',
    defaultMessage: "Local LLMs like Ollama typically don't require an API key.",
  },
  requiresApiKey: {
    id: 'customProviderForm.requiresApiKey',
    defaultMessage: 'This provider requires an API key',
  },
  apiKey: {
    id: 'customProviderForm.apiKey',
    defaultMessage: 'API Key',
  },
  apiKeyPlaceholderExisting: {
    id: 'customProviderForm.apiKeyPlaceholderExisting',
    defaultMessage: 'Leave blank to keep existing key',
  },
  apiKeyPlaceholderNew: {
    id: 'customProviderForm.apiKeyPlaceholderNew',
    defaultMessage: 'Your API key',
  },
  availableModels: {
    id: 'customProviderForm.availableModels',
    defaultMessage: 'Available Models (comma-separated)',
  },
  modelsPlaceholder: {
    id: 'customProviderForm.modelsPlaceholder',
    defaultMessage: 'model-a, model-b, model-c',
  },
  toolCalling: {
    id: 'customProviderForm.toolCalling',
    defaultMessage: 'Tool calling',
  },
  reasoning: {
    id: 'customProviderForm.reasoning',
    defaultMessage: 'Reasoning',
  },
  attachments: {
    id: 'customProviderForm.attachments',
    defaultMessage: 'Attachments',
  },
  supportsStreaming: {
    id: 'customProviderForm.supportsStreaming',
    defaultMessage: 'Provider supports streaming responses',
  },
  alwaysUseToolshim: {
    id: 'customProviderForm.alwaysUseToolshim',
    defaultMessage: 'Always use Toolshim for this provider',
  },
  customHeaders: {
    id: 'customProviderForm.customHeaders',
    defaultMessage: 'Custom Headers',
  },
  customHeadersHint: {
    id: 'customProviderForm.customHeadersHint',
    defaultMessage:
      'Add custom HTTP headers to include in requests to the provider. Click the "+" button to add after filling both fields.',
  },
  headerNamePlaceholder: {
    id: 'customProviderForm.headerNamePlaceholder',
    defaultMessage: 'Header name',
  },
  valuePlaceholder: {
    id: 'customProviderForm.valuePlaceholder',
    defaultMessage: 'Value',
  },
  add: {
    id: 'customProviderForm.add',
    defaultMessage: 'Add',
  },
  headerBothRequired: {
    id: 'customProviderForm.headerBothRequired',
    defaultMessage: 'Both header name and value must be entered',
  },
  headerNoSpaces: {
    id: 'customProviderForm.headerNoSpaces',
    defaultMessage: 'Header name cannot contain spaces',
  },
  headerDuplicate: {
    id: 'customProviderForm.headerDuplicate',
    defaultMessage: 'A header with this name already exists',
  },
  displayNameRequired: {
    id: 'customProviderForm.displayNameRequired',
    defaultMessage: 'Display name is required',
  },
  apiUrlRequired: {
    id: 'customProviderForm.apiUrlRequired',
    defaultMessage: 'API URL is required',
  },
  apiKeyRequired: {
    id: 'customProviderForm.apiKeyRequired',
    defaultMessage: 'API key is required',
  },
  modelsRequired: {
    id: 'customProviderForm.modelsRequired',
    defaultMessage: 'At least one model is required',
  },
  submitError: {
    id: 'customProviderForm.submitError',
    defaultMessage: 'Failed to save provider. Please check your configuration and try again.',
  },
  cannotDeleteActive: {
    id: 'customProviderForm.cannotDeleteActive',
    defaultMessage:
      "You cannot delete this provider while it's currently in use. Please switch to a different model first.",
  },
  deleteConfirmation: {
    id: 'customProviderForm.deleteConfirmation',
    defaultMessage:
      'Are you sure you want to delete this custom provider? This will permanently remove the provider and its stored API key. This action cannot be undone.',
  },
  confirmDelete: {
    id: 'customProviderForm.confirmDelete',
    defaultMessage: 'Confirm Delete',
  },
  deleteProvider: {
    id: 'customProviderForm.deleteProvider',
    defaultMessage: 'Delete Provider',
  },
  updateProvider: {
    id: 'customProviderForm.updateProvider',
    defaultMessage: 'Update Provider',
  },
  createProvider: {
    id: 'customProviderForm.createProvider',
    defaultMessage: 'Create Provider',
  },
});

type Step = 'choice' | 'catalog' | 'form';

type ProviderEngine = 'openai_compatible' | 'anthropic_compatible' | 'ollama_compatible';

const ENGINE_ALIASES: Record<string, ProviderEngine> = {
  openai: 'openai_compatible',
  openai_compatible: 'openai_compatible',
  anthropic: 'anthropic_compatible',
  anthropic_compatible: 'anthropic_compatible',
  ollama: 'ollama_compatible',
  ollama_compatible: 'ollama_compatible',
};

const normalizeEngine = (engine: string): ProviderEngine =>
  ENGINE_ALIASES[engine.trim().toLowerCase()] ?? 'openai_compatible';

interface CustomProviderFormProps {
  onSubmit: (data: UpdateCustomProviderRequest) => void | Promise<void>;
  onCancel: () => void;
  onDelete?: () => Promise<void>;
  isActiveProvider?: boolean;
  initialData: UpdateCustomProviderRequest | null;
  isEditable?: boolean;
}

export default function CustomProviderForm({
  onSubmit,
  onCancel,
  onDelete,
  isActiveProvider = false,
  initialData,
  isEditable,
}: CustomProviderFormProps) {
  const intl = useIntl();
  const engineOptions: { value: ProviderEngine; label: string }[] = [
    { value: 'openai_compatible', label: intl.formatMessage(i18n.openaiCompatible) },
    { value: 'anthropic_compatible', label: intl.formatMessage(i18n.anthropicCompatible) },
    { value: 'ollama_compatible', label: intl.formatMessage(i18n.ollamaCompatible) },
  ];
  const [engine, setEngine] = useState<ProviderEngine>('openai_compatible');
  const [displayName, setDisplayName] = useState('');
  const [apiUrl, setApiUrl] = useState('');
  const [basePath, setBasePath] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [models, setModels] = useState('');
  const [requiresAuth, setRequiresAuth] = useState(false);
  const [supportsStreaming, setSupportsStreaming] = useState(true);
  const [toolshim, setToolshim] = useState(false);
  const [headers, setHeaders] = useState<{ key: string; value: string }[]>([]);
  const [newHeaderKey, setNewHeaderKey] = useState('');
  const [newHeaderValue, setNewHeaderValue] = useState('');
  const [headerValidationError, setHeaderValidationError] = useState<string | null>(null);
  const [invalidHeaderFields, setInvalidHeaderFields] = useState<{ key: boolean; value: boolean }>({
    key: false,
    value: false,
  });
  const [validationErrors, setValidationErrors] = useState<Record<string, string>>({});
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [showDeleteConfirmation, setShowDeleteConfirmation] = useState(false);
  const contextVersionRef = useRef(0);

  // Template + step state
  const [selectedTemplate, setSelectedTemplate] = useState<ProviderTemplateDto | null>(null);
  const [step, setStep] = useState<Step>(initialData ? 'form' : 'choice');

  const clearSensitiveState = () => {
    contextVersionRef.current += 1;
    setApiKey('');
    setHeaders([]);
    setNewHeaderKey('');
    setNewHeaderValue('');
    setHeaderValidationError(null);
    setInvalidHeaderFields({ key: false, value: false });
    setValidationErrors({});
    setSubmitError(null);
  };

  useEffect(() => {
    if (initialData) {
      setEngine(normalizeEngine(initialData.engine));
      setDisplayName(initialData.display_name);
      setApiUrl(initialData.api_url);
      setBasePath(initialData.base_path ?? '');
      setModels(initialData.models.join(', '));
      setSupportsStreaming(initialData.supports_streaming ?? true);
      setToolshim(initialData.toolshim);
      setRequiresAuth(initialData.requires_auth ?? true);

      if (initialData.headers) {
        const headerList = Object.entries(initialData.headers).map(([key, value]) => ({
          key,
          value,
        }));
        setHeaders(headerList);
      }

      setStep('form');
    }
  }, [initialData]);

  const handleTemplateSelect = (template: ProviderTemplateDto) => {
    clearSensitiveState();
    setSelectedTemplate(template);

    // Prefill fields from template
    setDisplayName(template.name);
    setApiUrl(template.apiUrl);
    setBasePath('');
    setSupportsStreaming(template.supportsStreaming);
    setRequiresAuth(true);

    setEngine(normalizeEngine(template.format));

    const templateModels = template.models.filter((m) => !m.deprecated).map((m) => m.id);
    setModels(templateModels.join(', '));

    setStep('form');
  };

  const handleClearTemplate = () => {
    clearSensitiveState();
    setSelectedTemplate(null);
    setDisplayName('');
    setApiUrl('');
    setBasePath('');
    setModels('');
    setEngine('openai_compatible');
    setSupportsStreaming(true);
    setRequiresAuth(false);
    setStep('choice');
  };

  const handleBackToChoice = () => {
    clearSensitiveState();
    setStep('choice');
  };

  const handleCancel = () => {
    clearSensitiveState();
    onCancel();
  };

  const handleRequiresAuthChange = (checked: boolean) => {
    setRequiresAuth(checked);
    if (!checked) {
      setApiKey('');
    }
  };

  const handleAddHeader = () => {
    const keyEmpty = !newHeaderKey.trim();
    const valueEmpty = !newHeaderValue.trim();
    const keyHasSpaces = newHeaderKey.includes(' ');
    const normalizedNewKey = newHeaderKey.trim().toLowerCase();
    const isDuplicate = headers.some((h) => h.key.trim().toLowerCase() === normalizedNewKey);

    if (keyEmpty || valueEmpty) {
      setInvalidHeaderFields({ key: keyEmpty, value: valueEmpty });
      setHeaderValidationError(intl.formatMessage(i18n.headerBothRequired));
      return;
    }

    if (keyHasSpaces) {
      setInvalidHeaderFields({ key: true, value: false });
      setHeaderValidationError(intl.formatMessage(i18n.headerNoSpaces));
      return;
    }

    if (isDuplicate) {
      setInvalidHeaderFields({ key: true, value: false });
      setHeaderValidationError(intl.formatMessage(i18n.headerDuplicate));
      return;
    }

    setHeaderValidationError(null);
    setInvalidHeaderFields({ key: false, value: false });
    setHeaders([...headers, { key: newHeaderKey, value: newHeaderValue }]);
    setNewHeaderKey('');
    setNewHeaderValue('');
  };

  const handleRemoveHeader = (index: number) => {
    setHeaders(headers.filter((_, i) => i !== index));
  };

  const handleHeaderChange = (index: number, field: 'key' | 'value', value: string) => {
    if (field === 'key') {
      if (value.includes(' ')) return;
      const normalizedValue = value.trim().toLowerCase();
      const isDuplicate = headers.some(
        (h, i) => i !== index && h.key.trim().toLowerCase() === normalizedValue
      );
      if (isDuplicate && normalizedValue !== '') return;
      const updatedHeaders = [...headers];
      updatedHeaders[index].key = value;
      setHeaders(updatedHeaders);
      return;
    }
    const updatedHeaders = [...headers];
    updatedHeaders[index][field] = value;
    setHeaders(updatedHeaders);
  };

  const clearHeaderValidation = () => {
    setHeaderValidationError(null);
    setInvalidHeaderFields({ key: false, value: false });
  };

  const handleHeaderKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleAddHeader();
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const contextVersion = contextVersionRef.current;
    setSubmitError(null);
    setValidationErrors({});

    const errors: Record<string, string> = {};
    if (!displayName) errors.displayName = intl.formatMessage(i18n.displayNameRequired);
    if (!apiUrl) errors.apiUrl = intl.formatMessage(i18n.apiUrlRequired);
    const existingHadAuth = initialData && (initialData.requires_auth ?? true);
    if (requiresAuth && !apiKey && !existingHadAuth)
      errors.apiKey = intl.formatMessage(i18n.apiKeyRequired);
    if (!models) errors.models = intl.formatMessage(i18n.modelsRequired);

    if (Object.keys(errors).length > 0) {
      setValidationErrors(errors);
      return;
    }

    const modelList = models
      .split(',')
      .map((name) => name.trim())
      .filter(Boolean);

    let allHeaders = [...headers];

    if (newHeaderKey.trim() && newHeaderValue.trim()) {
      const keyHasSpaces = newHeaderKey.includes(' ');
      const normalizedPendingKey = newHeaderKey.trim().toLowerCase();
      const isDuplicate = headers.some((h) => h.key.trim().toLowerCase() === normalizedPendingKey);

      if (!keyHasSpaces && !isDuplicate) {
        allHeaders.push({ key: newHeaderKey, value: newHeaderValue });
      }
    }

    const headersObject = allHeaders.reduce(
      (acc, header) => {
        if (header.key.trim() && header.value.trim()) {
          acc[header.key.trim()] = header.value.trim();
        }
        return acc;
      },
      {} as Record<string, string>
    );

    try {
      await onSubmit({
        engine,
        display_name: displayName,
        api_url: apiUrl,
        api_key: apiKey,
        models: modelList,
        supports_streaming: supportsStreaming,
        toolshim,
        requires_auth: requiresAuth,
        headers: headersObject,
        catalog_provider_id:
          selectedTemplate?.providerId ?? initialData?.catalog_provider_id ?? undefined,
        base_path: basePath || undefined,
      });
    } catch (error) {
      if (contextVersionRef.current !== contextVersion) return;
      console.error('Failed to save custom provider:', error);
      setSubmitError(intl.formatMessage(i18n.submitError));
    }
  };

  // Aggregate capability badges for template models
  const templateModelCapabilities = selectedTemplate?.models
    .filter((m) => !m.deprecated)
    .reduce(
      (acc, m) => {
        if (m.capabilities.toolCall) acc.tool_call = true;
        if (m.capabilities.reasoning) acc.reasoning = true;
        if (m.capabilities.attachment) acc.attachment = true;
        return acc;
      },
      { tool_call: false, reasoning: false, attachment: false }
    );

  // -- Step: Choice --
  if (step === 'choice') {
    return (
      <div className="mt-4 space-y-3">
        <p className="text-sm text-textSubtle">{intl.formatMessage(i18n.chooseSetup)}</p>
        <button
          type="button"
          onClick={() => setStep('catalog')}
          className="w-full p-4 text-left border border-border rounded-lg hover:bg-surfaceHover hover:border-primary transition-colors group"
        >
          <div className="flex items-center gap-3">
            <Search className="w-5 h-5 text-primary flex-shrink-0" />
            <div>
              <div className="font-medium text-textStandard">
                {intl.formatMessage(i18n.startFromTemplate)}
              </div>
              <div className="text-sm text-textSubtle mt-0.5">
                {intl.formatMessage(i18n.startFromTemplateDesc)}
              </div>
            </div>
          </div>
        </button>
        <button
          type="button"
          onClick={() => setStep('form')}
          className="w-full p-4 text-left border border-border rounded-lg hover:bg-surfaceHover hover:border-primary transition-colors group"
        >
          <div className="flex items-center gap-3">
            <Settings className="w-5 h-5 text-textSubtle flex-shrink-0" />
            <div>
              <div className="font-medium text-textStandard">
                {intl.formatMessage(i18n.configureManually)}
              </div>
              <div className="text-sm text-textSubtle mt-0.5">
                {intl.formatMessage(i18n.configureManuallyDesc)}
              </div>
            </div>
          </div>
        </button>
        <div className="flex justify-end pt-2">
          <Button type="button" variant="outline" onClick={handleCancel}>
            {intl.formatMessage(i18n.cancel)}
          </Button>
        </div>
      </div>
    );
  }

  // -- Step: Catalog picker --
  if (step === 'catalog') {
    return (
      <div className="mt-4">
        <ProviderCatalogPicker onSelect={handleTemplateSelect} onCancel={handleCancel} embedded />
        <div className="flex justify-between pt-4">
          <Button type="button" variant="ghost" onClick={handleBackToChoice}>
            {intl.formatMessage(i18n.back)}
          </Button>
          <Button type="button" variant="outline" onClick={handleCancel}>
            {intl.formatMessage(i18n.cancel)}
          </Button>
        </div>
      </div>
    );
  }

  // -- Step: Form --
  return (
    <form onSubmit={handleSubmit} className="mt-4 space-y-4">
      {/* Template info banner */}
      {selectedTemplate && (
        <div className="p-3 bg-surfaceHover border border-border rounded-lg">
          <div className="flex items-center justify-between">
            <div className="text-sm">
              <div className="font-medium text-textStandard">
                {intl.formatMessage(i18n.usingTemplate, { name: selectedTemplate.name })}
              </div>
              <div className="text-textSubtle mt-1">{selectedTemplate.apiUrl}</div>
            </div>
            <div className="flex items-center gap-2">
              {selectedTemplate.docUrl && (
                <a
                  href={selectedTemplate.docUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-primary hover:underline text-sm flex items-center gap-1"
                >
                  {intl.formatMessage(i18n.docs)} <ExternalLink className="w-3 h-3" />
                </a>
              )}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={handleClearTemplate}
                className="text-textSubtle hover:text-textStandard"
              >
                {intl.formatMessage(i18n.clear)}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* Back to choice (create without template only) */}
      {!initialData && !selectedTemplate && (
        <Button type="button" variant="ghost" size="sm" onClick={handleBackToChoice}>
          {intl.formatMessage(i18n.back)}
        </Button>
      )}

      {/* Provider type dropdown */}
      {isEditable && (
        <div>
          <label
            htmlFor="provider-select"
            className="flex items-center text-sm font-medium text-text-primary mb-2"
          >
            {intl.formatMessage(i18n.providerType)}
            <span className="text-red-500 ml-1">*</span>
          </label>
          <Select
            id="provider-select"
            aria-invalid={!!validationErrors.providerType}
            aria-describedby={validationErrors.providerType ? 'provider-select-error' : undefined}
            options={engineOptions}
            value={engineOptions.find((option) => option.value === engine)}
            onChange={(option: unknown) => {
              const selectedOption = option as { value: ProviderEngine } | null;
              if (selectedOption) setEngine(selectedOption.value);
            }}
            isSearchable={false}
          />
          {validationErrors.providerType && (
            <p id="provider-select-error" className="text-red-500 text-sm mt-1">
              {validationErrors.providerType}
            </p>
          )}
        </div>
      )}

      {/* Display name */}
      {isEditable && (
        <div>
          <label
            htmlFor="display-name"
            className="flex items-center text-sm font-medium text-text-primary mb-2"
          >
            {intl.formatMessage(i18n.displayName)}
            <span className="text-red-500 ml-1">*</span>
          </label>
          <Input
            id="display-name"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            placeholder={intl.formatMessage(i18n.displayNamePlaceholder)}
            aria-invalid={!!validationErrors.displayName}
            aria-describedby={validationErrors.displayName ? 'display-name-error' : undefined}
            className={validationErrors.displayName ? 'border-red-500' : ''}
          />
          {validationErrors.displayName && (
            <p id="display-name-error" className="text-red-500 text-sm mt-1">
              {validationErrors.displayName}
            </p>
          )}
        </div>
      )}

      {/* API URL */}
      {isEditable && (
        <div>
          <label
            htmlFor="api-url"
            className="flex items-center text-sm font-medium text-text-primary mb-2"
          >
            {intl.formatMessage(i18n.apiUrl)}
            <span className="text-red-500 ml-1">*</span>
          </label>
          <Input
            id="api-url"
            value={apiUrl}
            onChange={(e) => setApiUrl(e.target.value)}
            placeholder={intl.formatMessage(i18n.apiUrlPlaceholder)}
            aria-invalid={!!validationErrors.apiUrl}
            aria-describedby={validationErrors.apiUrl ? 'api-url-error' : undefined}
            className={validationErrors.apiUrl ? 'border-red-500' : ''}
          />
          {validationErrors.apiUrl && (
            <p id="api-url-error" className="text-red-500 text-sm mt-1">
              {validationErrors.apiUrl}
            </p>
          )}
        </div>
      )}

      {/* Base Path */}
      {isEditable && (
        <div>
          <label
            htmlFor="base-path"
            className="flex items-center text-sm font-medium text-text-primary mb-2"
          >
            {intl.formatMessage(i18n.apiBasePath)}
          </label>
          <Input
            id="base-path"
            value={basePath}
            onChange={(e) => setBasePath(e.target.value)}
            placeholder={intl.formatMessage(i18n.apiBasePathPlaceholder)}
          />
          <p className="text-xs text-textSubtle mt-1">{intl.formatMessage(i18n.apiBasePathHint)}</p>
        </div>
      )}

      {/* Authentication */}
      <div>
        <label className="block text-sm font-medium text-text-primary mb-2">
          {intl.formatMessage(i18n.authentication)}
        </label>
        <p className="text-sm text-text-secondary mb-3">{intl.formatMessage(i18n.authHint)}</p>
        <div className="flex items-center space-x-2">
          <input
            type="checkbox"
            id="requires-auth"
            checked={requiresAuth}
            onChange={(e) => handleRequiresAuthChange(e.target.checked)}
            className="rounded border-border-primary"
          />
          <label htmlFor="requires-auth" className="text-sm text-text-secondary">
            {intl.formatMessage(i18n.requiresApiKey)}
          </label>
        </div>

        {requiresAuth && (
          <div className="mt-3">
            <label
              htmlFor="api-key"
              className="flex items-center text-sm font-medium text-text-primary mb-2"
            >
              {intl.formatMessage(i18n.apiKey)}
              {selectedTemplate?.envVar && (
                <span className="text-textSubtle ml-1 font-normal">
                  ({selectedTemplate.envVar})
                </span>
              )}
              {!initialData && <span className="text-red-500 ml-1">*</span>}
            </label>
            <Input
              id="api-key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={
                initialData
                  ? intl.formatMessage(i18n.apiKeyPlaceholderExisting)
                  : intl.formatMessage(i18n.apiKeyPlaceholderNew)
              }
              aria-invalid={!!validationErrors.apiKey}
              aria-describedby={validationErrors.apiKey ? 'api-key-error' : undefined}
              className={validationErrors.apiKey ? 'border-red-500' : ''}
            />
            {validationErrors.apiKey && (
              <p id="api-key-error" className="text-red-500 text-sm mt-1">
                {validationErrors.apiKey}
              </p>
            )}
          </div>
        )}
      </div>

      {/* Models */}
      {isEditable && (
        <div>
          <label
            htmlFor="available-models"
            className="flex items-center text-sm font-medium text-text-primary mb-2"
          >
            {intl.formatMessage(i18n.availableModels)}
            <span className="text-red-500 ml-1">*</span>
          </label>
          <Input
            id="available-models"
            value={models}
            onChange={(e) => setModels(e.target.value)}
            placeholder={intl.formatMessage(i18n.modelsPlaceholder)}
            aria-invalid={!!validationErrors.models}
            aria-describedby={validationErrors.models ? 'available-models-error' : undefined}
            className={validationErrors.models ? 'border-red-500' : ''}
          />
          {validationErrors.models && (
            <p id="available-models-error" className="text-red-500 text-sm mt-1">
              {validationErrors.models}
            </p>
          )}
          {/* Capability badges when template is active */}
          {selectedTemplate && templateModelCapabilities && (
            <div className="flex gap-2 mt-2">
              {templateModelCapabilities.tool_call && (
                <span className="text-xs px-2 py-0.5 rounded-full bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300">
                  {intl.formatMessage(i18n.toolCalling)}
                </span>
              )}
              {templateModelCapabilities.reasoning && (
                <span className="text-xs px-2 py-0.5 rounded-full bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-300">
                  {intl.formatMessage(i18n.reasoning)}
                </span>
              )}
              {templateModelCapabilities.attachment && (
                <span className="text-xs px-2 py-0.5 rounded-full bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-300">
                  {intl.formatMessage(i18n.attachments)}
                </span>
              )}
            </div>
          )}
        </div>
      )}

      {isEditable && (
        <div className="space-y-3 mb-10">
          <div className="flex items-center space-x-2">
            <input
              type="checkbox"
              id="supports-streaming"
              checked={supportsStreaming}
              onChange={(e) => setSupportsStreaming(e.target.checked)}
              className="rounded border-border-primary"
            />
            <label htmlFor="supports-streaming" className="text-sm text-text-secondary">
              {intl.formatMessage(i18n.supportsStreaming)}
            </label>
          </div>
          <div className="flex items-center space-x-2">
            <input
              type="checkbox"
              id="always-use-toolshim"
              checked={toolshim}
              onChange={(e) => setToolshim(e.target.checked)}
              className="rounded border-border-primary"
            />
            <label htmlFor="always-use-toolshim" className="text-sm text-text-secondary">
              {intl.formatMessage(i18n.alwaysUseToolshim)}
            </label>
          </div>
        </div>
      )}

      {/* Custom headers */}
      {isEditable && (
        <div>
          <label className="text-sm font-medium text-textStandard mb-2 block">
            {intl.formatMessage(i18n.customHeaders)}
          </label>
          <p className="text-xs text-textSubtle mb-4">
            {intl.formatMessage(i18n.customHeadersHint)}
          </p>
          <div className="grid grid-cols-[1fr_1fr_auto] gap-2 items-center">
            {headers.map((header, index) => (
              <React.Fragment key={index}>
                <Input
                  value={header.key}
                  onChange={(e) => handleHeaderChange(index, 'key', e.target.value)}
                  placeholder={intl.formatMessage(i18n.headerNamePlaceholder)}
                  className="w-full text-textStandard border-borderSubtle hover:border-borderStandard"
                />
                <Input
                  value={header.value}
                  onChange={(e) => handleHeaderChange(index, 'value', e.target.value)}
                  placeholder={intl.formatMessage(i18n.valuePlaceholder)}
                  className="w-full text-textStandard border-borderSubtle hover:border-borderStandard"
                />
                <Button
                  onClick={() => handleRemoveHeader(index)}
                  variant="ghost"
                  type="button"
                  className="group p-2 h-auto text-iconSubtle hover:bg-transparent"
                >
                  <X className="h-3 w-3 text-gray-400 group-hover:text-white group-hover:drop-shadow-sm transition-all" />
                </Button>
              </React.Fragment>
            ))}

            <Input
              value={newHeaderKey}
              onChange={(e) => {
                setNewHeaderKey(e.target.value);
                clearHeaderValidation();
              }}
              onKeyDown={handleHeaderKeyDown}
              placeholder="Header name"
              className={cn(
                'w-full text-textStandard border-borderSubtle hover:border-borderStandard',
                invalidHeaderFields.key && 'border-red-500 focus:border-red-500'
              )}
            />
            <Input
              value={newHeaderValue}
              onChange={(e) => {
                setNewHeaderValue(e.target.value);
                clearHeaderValidation();
              }}
              onKeyDown={handleHeaderKeyDown}
              placeholder={intl.formatMessage(i18n.valuePlaceholder)}
              className={cn(
                'w-full text-textStandard border-borderSubtle hover:border-borderStandard',
                invalidHeaderFields.value && 'border-red-500 focus:border-red-500'
              )}
            />
            <Button
              onClick={handleAddHeader}
              variant="ghost"
              type="button"
              className="flex items-center justify-start gap-1 px-2 pr-4 text-sm rounded-full text-textStandard bg-background-primary border border-borderSubtle hover:border-borderStandard transition-colors min-w-[60px] h-9 [&>svg]:!size-4"
            >
              <Plus /> {intl.formatMessage(i18n.add)}
            </Button>
          </div>
          {headerValidationError && (
            <div className="mt-2 text-red-500 text-sm">{headerValidationError}</div>
          )}
        </div>
      )}

      <SecureStorageNotice />

      {submitError && <p className="text-red-500 text-sm">{submitError}</p>}

      {showDeleteConfirmation ? (
        <div className="pt-4 space-y-3">
          {isActiveProvider ? (
            <div className="px-4 py-3 bg-yellow-600/20 border border-yellow-500/30 rounded">
              <p className="text-yellow-500 text-sm flex items-start">
                <AlertTriangle className="h-4 w-4 mr-2 mt-0.5 flex-shrink-0" />
                <span>{intl.formatMessage(i18n.cannotDeleteActive)}</span>
              </p>
            </div>
          ) : (
            <div className="px-4 py-3 bg-red-900/20 border border-red-500/30 rounded">
              <p className="text-red-400 text-sm">{intl.formatMessage(i18n.deleteConfirmation)}</p>
            </div>
          )}
          <div className="flex justify-end space-x-2">
            <Button
              type="button"
              variant="outline"
              onClick={() => setShowDeleteConfirmation(false)}
            >
              {intl.formatMessage(i18n.cancel)}
            </Button>
            {!isActiveProvider && (
              <Button type="button" variant="destructive" onClick={onDelete}>
                <Trash2 className="h-4 w-4 mr-2" />
                {intl.formatMessage(i18n.confirmDelete)}
              </Button>
            )}
          </div>
        </div>
      ) : (
        <div className="flex justify-end space-x-2 pt-4">
          {initialData && onDelete && (
            <Button
              type="button"
              variant="outline"
              className="text-red-500 hover:text-red-600 mr-auto"
              onClick={() => setShowDeleteConfirmation(true)}
            >
              <Trash2 className="h-4 w-4 mr-2" />
              {intl.formatMessage(i18n.deleteProvider)}
            </Button>
          )}
          <Button type="button" variant="outline" onClick={handleCancel}>
            {intl.formatMessage(i18n.cancel)}
          </Button>
          <Button type="submit">
            {initialData
              ? intl.formatMessage(i18n.updateProvider)
              : intl.formatMessage(i18n.createProvider)}
          </Button>
        </div>
      )}
    </form>
  );
}

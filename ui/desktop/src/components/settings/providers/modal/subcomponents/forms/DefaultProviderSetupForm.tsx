import React, { useEffect, useMemo, useState, useCallback } from 'react';
import { Input } from '../../../../../ui/input';
import { acpReadProviderConfig } from '../../../../../../acp/providers';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '../../../../../ui/collapsible';
import type { ConfigKey, ProviderDetails } from '../../../../../../types/providers';
import { configLabels, configPlaceholders } from '../../../../../../utils/configUtils';
import { defineMessages, useIntl } from '../../../../../../i18n';

const i18n = defineMessages({
  loadingConfig: {
    id: 'defaultProviderSetupForm.loadingConfig',
    defaultMessage: 'Loading configuration values...',
  },
  noConfigParameters: {
    id: 'defaultProviderSetupForm.noConfigParameters',
    defaultMessage: 'No configuration parameters for this provider.',
  },
  apiKeyPlaceholder: {
    id: 'defaultProviderSetupForm.apiKeyPlaceholder',
    defaultMessage: 'Your API key',
  },
  apiHostPlaceholder: {
    id: 'defaultProviderSetupForm.apiHostPlaceholder',
    defaultMessage: 'https://api.example.com',
  },
  modelsPlaceholder: {
    id: 'defaultProviderSetupForm.modelsPlaceholder',
    defaultMessage: 'model-a, model-b',
  },
  apiKeyLabel: {
    id: 'defaultProviderSetupForm.apiKeyLabel',
    defaultMessage: 'API Key',
  },
  apiHostLabel: {
    id: 'defaultProviderSetupForm.apiHostLabel',
    defaultMessage: 'API Host',
  },
  modelsLabel: {
    id: 'defaultProviderSetupForm.modelsLabel',
    defaultMessage: 'Models',
  },
  showOptions: {
    id: 'defaultProviderSetupForm.showOptions',
    defaultMessage: 'Show {count} options',
  },
  hideOptions: {
    id: 'defaultProviderSetupForm.hideOptions',
    defaultMessage: 'Hide {count} options',
  },
});

type ValidationErrors = Record<string, string>;

type ConfigValue = string | { maskedValue: string };
export interface ConfigInput {
  serverValue?: ConfigValue;
  value?: string;
}

interface DefaultProviderSetupFormProps {
  configValues: Record<string, ConfigInput>;
  setConfigValues: React.Dispatch<React.SetStateAction<Record<string, ConfigInput>>>;
  provider: ProviderDetails;
  validationErrors: ValidationErrors;
  showOptions?: boolean;
}

const envToPrettyName = (envVar: string) => {
  const wordReplacements: { [w: string]: string } = {
    Api: 'API',
    Aws: 'AWS',
    Gcp: 'GCP',
  };

  return envVar
    .toLowerCase()
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .map((word) => wordReplacements[word] || word)
    .join(' ')
    .trim();
};

export default function DefaultProviderSetupForm({
  configValues,
  setConfigValues,
  provider,
  validationErrors = {},
  showOptions = true,
}: DefaultProviderSetupFormProps) {
  const parameters = useMemo(
    () => provider.metadata.config_keys || [],
    [provider.metadata.config_keys]
  );
  const intl = useIntl();
  const [isLoading, setIsLoading] = useState(true);
  const [optionalExpanded, setOptionalExpanded] = useState(false);

  const loadConfigValues = useCallback(async () => {
    setIsLoading(true);
    try {
      const values: { [k: string]: ConfigInput } = {};

      let fields: Awaited<ReturnType<typeof acpReadProviderConfig>> = [];
      let readFailed = false;
      try {
        fields = await acpReadProviderConfig(provider.name);
      } catch {
        // A failed read cannot be distinguished from "nothing is stored", so
        // seeding defaults here would submit them over values already on disk.
        readFailed = true;
      }
      const fieldByKey = new Map(fields.map((field) => [field.key, field]));

      for (const parameter of parameters) {
        const field = fieldByKey.get(parameter.name);

        if (field?.isSet && field.value != null) {
          // Secrets come back masked from the server; preserve the masked shape
          // so the form renders a placeholder rather than the raw value.
          const serverValue: ConfigValue = parameter.secret
            ? { maskedValue: field.value }
            : field.value;
          values[parameter.name] = { serverValue };
        } else if (!readFailed && parameter.default !== undefined && parameter.default !== null) {
          values[parameter.name] = { value: parameter.default };
        }
      }

      setConfigValues((prev) => ({
        ...prev,
        ...values,
      }));
    } finally {
      setIsLoading(false);
    }
  }, [parameters, provider.name, setConfigValues]);

  useEffect(() => {
    loadConfigValues();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const getPlaceholder = (parameter: ConfigKey): string => {
    if (parameter.secret) {
      const serverValue = configValues[parameter.name]?.serverValue;
      if (typeof serverValue === 'object' && 'maskedValue' in serverValue) {
        return serverValue.maskedValue;
      }
    }

    if (parameter.default !== undefined && parameter.default !== null) {
      return parameter.default;
    }

    if (configPlaceholders[parameter.name]) return configPlaceholders[parameter.name];
    const name = parameter.name.toLowerCase();
    if (name.includes('api_key')) return intl.formatMessage(i18n.apiKeyPlaceholder);
    if (name.includes('api_url') || name.includes('host'))
      return intl.formatMessage(i18n.apiHostPlaceholder);
    if (name.includes('models')) return intl.formatMessage(i18n.modelsPlaceholder);

    return parameter.name
      .replace(/_/g, ' ')
      .replace(/^./, (str) => str.toUpperCase())
      .trim();
  };

  const getFieldLabel = (parameter: ConfigKey) => {
    if (configLabels[parameter.name]) return configLabels[parameter.name];
    const name = parameter.name.toLowerCase();
    if (name.includes('api_key')) return intl.formatMessage(i18n.apiKeyLabel);
    if (name.includes('api_url') || name.includes('host'))
      return intl.formatMessage(i18n.apiHostLabel);
    if (name.includes('models')) return intl.formatMessage(i18n.modelsLabel);

    let parameter_name = parameter.name.toUpperCase();
    if (parameter_name.startsWith(provider.name.toUpperCase().replace('-', '_'))) {
      parameter_name = parameter_name.slice(provider.name.length + 1);
    }
    let pretty = envToPrettyName(parameter_name);
    return (
      <span>
        <span>{pretty}</span>
        <span className="text-sm font-light ml-2">({parameter.name})</span>
      </span>
    );
  };

  if (isLoading) {
    return <div className="text-center py-4">{intl.formatMessage(i18n.loadingConfig)}</div>;
  }

  function getRenderValue(parameter: ConfigKey): string {
    const entry = configValues[parameter.name];
    // If the user has edited the field (even to empty string), use their value.
    // This prevents the input from snapping back to the stored serverValue
    // when the user backspaces to clear the field.
    if (entry?.value !== undefined) {
      return entry.value;
    }
    if (parameter.secret) {
      return '';
    }
    // Convert serverValue to string explicitly — native booleans (false) would
    // be falsy and get collapsed to '' by the || operator, losing the value.
    if (entry?.serverValue !== undefined && entry?.serverValue !== null) {
      return String(entry.serverValue);
    }
    return '';
  }

  // Detect boolean parameters (default is "true" or "false")
  function isBooleanParameter(parameter: ConfigKey): boolean {
    const def = parameter.default?.toLowerCase();
    return def === 'true' || def === 'false';
  }

  function getBooleanValue(parameter: ConfigKey): boolean {
    const raw = getRenderValue(parameter);
    const val = String(raw).toLowerCase();
    if (val === '' && parameter.default) {
      return parameter.default.toLowerCase() === 'true';
    }
    return val === 'true';
  }

  // Pretty label for boolean toggle (strip provider prefix, humanize)
  function getBooleanLabel(parameter: ConfigKey): string {
    let name = parameter.name.toUpperCase();
    const prefix = provider.name.toUpperCase().replace('-', '_') + '_';
    if (name.startsWith(prefix)) {
      name = name.slice(prefix.length);
    }
    return envToPrettyName(name);
  }

  const renderParametersList = (parameters: ConfigKey[]) => {
    return parameters.map((parameter) => {
      if (isBooleanParameter(parameter)) {
        return (
          <div key={parameter.name} className="flex items-center space-x-2 py-2">
            <input
              type="checkbox"
              id={`toggle-${parameter.name}`}
              checked={getBooleanValue(parameter)}
              onChange={(e) => {
                setConfigValues((prev) => ({
                  ...prev,
                  [parameter.name]: {
                    ...(prev[parameter.name] || {}),
                    value: e.target.checked ? 'true' : 'false',
                  },
                }));
              }}
              className="rounded border-border-primary h-4 w-4"
            />
            <label htmlFor={`toggle-${parameter.name}`} className="text-sm text-text-secondary">
              {getBooleanLabel(parameter)}
            </label>
          </div>
        );
      }

      return (
        <div key={parameter.name}>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {getFieldLabel(parameter)}
            {parameter.required && <span className="text-red-500 ml-1">*</span>}
          </label>
          <Input
            type="text"
            value={getRenderValue(parameter)}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              setConfigValues((prev) => {
                const newValue = { ...(prev[parameter.name] || {}), value: e.target.value };
                return {
                  ...prev,
                  [parameter.name]: newValue,
                };
              });
            }}
            placeholder={getPlaceholder(parameter)}
            className={`w-full h-14 px-4 font-regular rounded-lg shadow-none ${
              validationErrors[parameter.name]
                ? 'border-2 border-red-500'
                : 'border border-border-primary hover:border-border-primary'
            } bg-background-primary text-lg placeholder:text-text-secondary font-regular text-text-primary`}
            required={parameter.required}
          />
          {validationErrors[parameter.name] && (
            <p className="text-red-500 text-sm mt-1">{validationErrors[parameter.name]}</p>
          )}
        </div>
      );
    });
  };

  let aboveFoldParameters = parameters.filter(
    (p) => p.primary || (p.required && (p.default === undefined || p.default === null))
  );
  let belowFoldParameters = parameters.filter(
    (p) => !p.primary && !(p.required && (p.default === undefined || p.default === null))
  );

  if (aboveFoldParameters.length === 0 && parameters.length > 0) {
    aboveFoldParameters = parameters;
    belowFoldParameters = [];
  }

  const expandCtaText = optionalExpanded
    ? intl.formatMessage(i18n.hideOptions, { count: belowFoldParameters.length })
    : intl.formatMessage(i18n.showOptions, { count: belowFoldParameters.length });

  return (
    <div className="mt-4 space-y-4">
      {aboveFoldParameters.length === 0 && belowFoldParameters.length === 0 ? (
        <div className="text-center text-gray-500">
          {intl.formatMessage(i18n.noConfigParameters)}
        </div>
      ) : (
        <div>
          <div>{renderParametersList(aboveFoldParameters)}</div>
          {showOptions && belowFoldParameters.length > 0 && (
            <Collapsible
              open={optionalExpanded}
              onOpenChange={setOptionalExpanded}
              className="my-4 border-2 border-dashed border-secondary rounded-lg bg-secondary/10"
            >
              <CollapsibleTrigger className="m-3 w-full">
                <div>
                  <span className="text-sm">{expandCtaText}</span>
                  <span className="text-sm">{optionalExpanded ? '↑' : '↓'}</span>
                </div>
              </CollapsibleTrigger>
              <CollapsibleContent className="mx-3 mb-3">
                {renderParametersList(belowFoldParameters)}
              </CollapsibleContent>
            </Collapsible>
          )}
        </div>
      )}
    </div>
  );
}

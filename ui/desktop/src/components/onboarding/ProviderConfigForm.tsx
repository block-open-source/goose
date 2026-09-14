import { useState } from 'react';
import { acpAuthenticateProvider } from '../../acp/providers';
import { useProviderDeviceCode } from '../../hooks/useProviderDeviceCode';
import type { ProviderDetails } from '../../types/providers';
import DefaultProviderSetupForm, {
  ConfigInput,
} from '../settings/providers/modal/subcomponents/forms/DefaultProviderSetupForm';
import { providerConfigSubmitHandler } from '../settings/providers/modal/subcomponents/handlers/DefaultSubmitHandler';
import ProviderLogo from '../settings/providers/modal/subcomponents/ProviderLogo';
import { SecureStorageNotice } from '../settings/providers/modal/subcomponents/SecureStorageNotice';
import AcpReadinessPanel from '../settings/providers/AcpReadinessPanel';
import { Button } from '../ui/button';
import { ChevronRight, LogIn } from 'lucide-react';
import { defineMessages, useIntl } from '../../i18n';
import { errorMessage } from '../../utils/conversionUtils';

type OnConfigured = (name: string) => void | Promise<void>;

const i18n = defineMessages({
  browserWindowOpen: {
    id: 'providerConfigForm.browserWindowOpen',
    defaultMessage: 'A browser window will open for you to complete the login.',
  },
  deviceCodeFlowHint: {
    id: 'providerConfigForm.deviceCodeFlowHint',
    defaultMessage:
      'A browser window will open. The verification code will appear here so you can enter it to complete sign-in.',
  },
  signingIn: {
    id: 'providerConfigForm.signingIn',
    defaultMessage: 'Signing in...',
  },
  signInWith: {
    id: 'providerConfigForm.signInWith',
    defaultMessage: 'Sign in with {providerName}',
  },
  noApiKey: {
    id: 'providerConfigForm.noApiKey',
    defaultMessage: "Don't have an API key?",
  },
  configuring: {
    id: 'providerConfigForm.configuring',
    defaultMessage: 'Configuring...',
  },
  continue: {
    id: 'providerConfigForm.continue',
    defaultMessage: 'Continue',
  },
  deviceCodeVisit: {
    id: 'providerConfigForm.deviceCodeVisit',
    defaultMessage: 'Visit',
  },
  deviceCodeAndEnter: {
    id: 'providerConfigForm.deviceCodeAndEnter',
    defaultMessage: 'and enter:',
  },
  deviceCodeCopy: {
    id: 'providerConfigForm.deviceCodeCopy',
    defaultMessage: 'Copy',
  },
});

function parseLinks(text: string) {
  return text.split(/(https?:\/\/[^\s]+)/g).map((part, i) =>
    /^https?:\/\//.test(part) ? (
      <a
        key={i}
        href="#"
        onClick={(e) => {
          e.preventDefault();
          window.electron.openExternal(part);
        }}
        className="underline hover:text-text-default cursor-pointer"
      >
        {part}
      </a>
    ) : (
      part
    )
  );
}

function OAuthForm({
  provider,
  onConfigured,
  onError,
}: {
  provider: ProviderDetails;
  onConfigured: OnConfigured;
  onError: (msg: string) => void;
}) {
  const intl = useIntl();
  const [isLoading, setIsLoading] = useState(false);
  const { deviceCode, clearDeviceCode } = useProviderDeviceCode(provider.name);

  const handleLogin = async () => {
    setIsLoading(true);
    clearDeviceCode();
    try {
      await acpAuthenticateProvider(provider.name);
      await onConfigured(provider.name);
    } catch (err) {
      onError(`Setup failed: ${errorMessage(err)}`);
    } finally {
      setIsLoading(false);
    }
  };

  const isDeviceCodeFlow = provider.metadata.config_keys.some((key) => key.device_code_flow);

  return (
    <div className="flex flex-col items-center gap-3 py-4">
      <Button
        onClick={handleLogin}
        disabled={isLoading}
        className="flex items-center gap-2 px-6 py-3"
        size="lg"
      >
        <LogIn size={20} />
        {isLoading
          ? intl.formatMessage(i18n.signingIn)
          : intl.formatMessage(i18n.signInWith, { providerName: provider.metadata.display_name })}
      </Button>
      {isDeviceCodeFlow && isLoading && deviceCode ? (
        <div className="flex flex-col items-center gap-2 w-full">
          <p className="text-xs text-text-muted text-center">
            {intl.formatMessage(i18n.deviceCodeVisit)}{' '}
            <a
              href="#"
              onClick={(e) => {
                e.preventDefault();
                window.electron.openExternal(deviceCode.verificationUri);
              }}
              className="underline"
            >
              {deviceCode.verificationUri}
            </a>{' '}
            {intl.formatMessage(i18n.deviceCodeAndEnter)}
          </p>
          <div className="flex items-center gap-2">
            <code className="text-lg font-mono tracking-widest bg-background-muted px-3 py-1 rounded">
              {deviceCode.userCode}
            </code>
            <button
              type="button"
              onClick={() => navigator.clipboard.writeText(deviceCode.userCode)}
              className="text-xs text-text-muted hover:text-text-default underline"
            >
              {intl.formatMessage(i18n.deviceCodeCopy)}
            </button>
          </div>
        </div>
      ) : (
        <p className="text-xs text-text-muted text-center">
          {isDeviceCodeFlow
            ? intl.formatMessage(i18n.deviceCodeFlowHint)
            : intl.formatMessage(i18n.browserWindowOpen)}
        </p>
      )}
    </div>
  );
}

function ApiKeyForm({
  provider,
  onConfigured,
  onError,
}: {
  provider: ProviderDetails;
  onConfigured: OnConfigured;
  onError: (msg: string) => void;
}) {
  const intl = useIntl();
  const [configValues, setConfigValues] = useState<Record<string, ConfigInput>>({});
  const [validationErrors, setValidationErrors] = useState<Record<string, string>>({});
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [showSetupHelp, setShowSetupHelp] = useState(false);
  const setupSteps = provider.metadata.setup_steps;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setValidationErrors({});

    const parameters = provider.metadata.config_keys || [];
    const errors: Record<string, string> = {};
    parameters.forEach((param) => {
      if (
        param.required &&
        !configValues[param.name]?.value &&
        !configValues[param.name]?.serverValue
      ) {
        errors[param.name] = `${param.name} is required`;
      }
    });

    if (Object.keys(errors).length > 0) {
      setValidationErrors(errors);
      return;
    }

    const toSubmit = Object.fromEntries(
      Object.entries(configValues)
        .filter(
          ([, entry]) =>
            !!entry.value || (entry.serverValue != null && typeof entry.serverValue === 'string')
        )
        .map(([k, entry]) => [
          k,
          entry.value ?? (typeof entry.serverValue === 'string' ? entry.serverValue : ''),
        ])
    );

    setIsSubmitting(true);
    try {
      await providerConfigSubmitHandler(provider, toSubmit);
      await onConfigured(provider.name);
    } catch (err) {
      onError(errorMessage(err));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <form onSubmit={handleSubmit}>
      <DefaultProviderSetupForm
        configValues={configValues}
        setConfigValues={setConfigValues}
        provider={provider}
        validationErrors={validationErrors}
        showOptions={false}
      />
      {provider.metadata.config_keys.some((k) => k.required && k.secret) && <SecureStorageNotice />}
      {setupSteps && setupSteps.length > 0 && (
        <div className="mt-3">
          <button
            type="button"
            onClick={() => setShowSetupHelp(!showSetupHelp)}
            className="flex items-center gap-1 text-sm text-text-muted hover:text-text-default transition-colors"
          >
            <ChevronRight
              size={14}
              className={`transition-transform duration-200 ${showSetupHelp ? 'rotate-90' : ''}`}
            />
            {intl.formatMessage(i18n.noApiKey)}
          </button>
          {showSetupHelp && (
            <ol className="mt-2 ml-5 list-decimal text-sm text-text-muted space-y-1">
              {setupSteps.map((step, i) => (
                <li key={i}>{parseLinks(step)}</li>
              ))}
            </ol>
          )}
        </div>
      )}
      <div className="mt-4">
        <Button type="submit" disabled={isSubmitting} className="w-full">
          {isSubmitting ? intl.formatMessage(i18n.configuring) : intl.formatMessage(i18n.continue)}
        </Button>
      </div>
    </form>
  );
}

interface ProviderConfigFormProps {
  provider: ProviderDetails;
  onConfigured: OnConfigured;
}

export default function ProviderConfigForm({ provider, onConfigured }: ProviderConfigFormProps) {
  const intl = useIntl();
  const [error, setError] = useState<string | null>(null);

  const isOAuthProvider = provider.metadata.config_keys.some((key) => key.oauth_flow);

  const renderForm = () => {
    if (provider.uses_acp) {
      return (
        <AcpReadinessPanel
          provider={provider}
          actionLabel={intl.formatMessage(i18n.continue)}
          onConfigured={(configured) => onConfigured(configured.name)}
          onError={setError}
        />
      );
    }
    if (isOAuthProvider) {
      return <OAuthForm provider={provider} onConfigured={onConfigured} onError={setError} />;
    }
    return <ApiKeyForm provider={provider} onConfigured={onConfigured} onError={setError} />;
  };

  return (
    <div>
      <div className="p-4 border rounded-xl bg-background-muted">
        <div className="flex items-center gap-3 mb-4">
          <ProviderLogo providerName={provider.name} />
          <div>
            <h3 className="font-medium text-text-default">{provider.metadata.display_name}</h3>
            <p className="text-xs text-text-muted">{provider.metadata.description}</p>
          </div>
        </div>

        {renderForm()}

        {error && (
          <div className="mt-3 p-3 rounded-lg bg-red-50 text-red-800 border border-red-200 dark:bg-red-900/20 dark:text-red-200 dark:border-red-800 text-sm">
            {error}
          </div>
        )}
      </div>
    </div>
  );
}

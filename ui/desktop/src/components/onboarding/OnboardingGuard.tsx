import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { useConfig } from '../ConfigContext';
import { useModelAndProvider } from '../ModelAndProviderContext';
import { acpListProviderDetails, acpReadDefaults, acpSaveDefaults } from '../../acp/providers';
import { Goose } from '../icons';
import { Button } from '../ui/button';
import ProviderSelector from './ProviderSelector';
import OnboardingSuccess from './OnboardingSuccess';
import {
  trackOnboardingStarted,
  trackOnboardingCompleted,
  trackOnboardingProviderSelected,
  trackTelemetryPreference,
  setTelemetryEnabled as setAnalyticsTelemetryEnabled,
} from '../../utils/analytics';
import { defineMessages, useIntl } from '../../i18n';
import { reconnectAcpToNewBackend } from '../../acp/acpConnection';
import { setWorkingDir } from '../../utils/workingDir';

const i18n = defineMessages({
  welcomeTitle: {
    id: 'onboardingGuard.welcomeTitle',
    defaultMessage: 'Welcome to goose',
  },
  welcomeDescription: {
    id: 'onboardingGuard.welcomeDescription',
    defaultMessage: 'Your local AI agent. Connect an AI model provider to get started.',
  },
  checkProviderErrorTitle: {
    id: 'onboardingGuard.checkProviderErrorTitle',
    defaultMessage: 'Unable to connect to Goose server',
  },
  checkProviderErrorDescription: {
    id: 'onboardingGuard.checkProviderErrorDescription',
    defaultMessage: 'The server may be starting up or temporarily unavailable.',
  },
  retry: {
    id: 'onboardingGuard.retry',
    defaultMessage: 'Retry',
  },
  externalBackendErrorTitle: {
    id: 'onboardingGuard.externalBackendErrorTitle',
    defaultMessage: 'Unable to connect to the external backend',
  },
  switchToLocal: {
    id: 'onboardingGuard.switchToLocal',
    defaultMessage: 'Switch to Local',
  },
  retrying: {
    id: 'onboardingGuard.retrying',
    defaultMessage: 'Retrying…',
  },
});

const TELEMETRY_CONFIG_KEY = 'GOOSE_TELEMETRY_ENABLED';

interface OnboardingGuardProps {
  children: React.ReactNode;
}

const initialExternalBackendError = (): string =>
  (window.appConfig?.get('GOOSE_EXTERNAL_BACKEND_ERROR') as string) ?? '';

export default function OnboardingGuard({ children }: OnboardingGuardProps) {
  const intl = useIntl();
  const navigate = useNavigate();
  const { upsert } = useConfig();
  const { getFallbackModelAndProvider, refreshCurrentModelAndProvider } = useModelAndProvider();

  const [isCheckingProvider, setIsCheckingProvider] = useState(true);
  const [hasProvider, setHasProvider] = useState(false);
  const [checkProviderError, setCheckProviderError] = useState(false);
  const [hasSelection, setHasSelection] = useState(false);
  const [configuredProvider, setConfiguredProvider] = useState<string | null>(null);
  const [configuredProviderDisplayName, setConfiguredProviderDisplayName] = useState<string | null>(
    null
  );
  const [configuredModel, setConfiguredModel] = useState<string | null>(null);
  const hasTrackedOnboardingStart = useRef(false);
  const [externalBackendError, setExternalBackendError] = useState(initialExternalBackendError);
  const [isRetrying, setIsRetrying] = useState(false);

  const checkProvider = async (retries = 3, delay = 1000) => {
    setIsCheckingProvider(true);
    setCheckProviderError(false);
    for (let attempt = 0; attempt <= retries; attempt++) {
      try {
        const { providerId: provider } = await acpReadDefaults();
        if (provider?.trim()) {
          setHasProvider(true);
          setIsCheckingProvider(false);
          return;
        }

        const fallback = await getFallbackModelAndProvider();
        if (fallback.provider?.trim() && fallback.model?.trim()) {
          const { providerId: configuredProvider, modelId: configuredModel } =
            await acpReadDefaults();
          if (configuredProvider?.trim() && configuredModel?.trim()) {
            await refreshCurrentModelAndProvider();
            setHasProvider(true);
            setIsCheckingProvider(false);
            return;
          }
        }

        setHasProvider(false);
        setIsCheckingProvider(false);
        return;
      } catch (error) {
        console.error(`Error checking provider (attempt ${attempt + 1}/${retries + 1}):`, error);
        if (attempt < retries) {
          await new Promise((resolve) => setTimeout(resolve, delay));
        }
      }
    }
    setCheckProviderError(true);
    setIsCheckingProvider(false);
  };

  useEffect(() => {
    checkProvider();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (
      !isCheckingProvider &&
      !hasProvider &&
      !checkProviderError &&
      !hasTrackedOnboardingStart.current
    ) {
      trackOnboardingStarted();
      hasTrackedOnboardingStart.current = true;
    }
  }, [isCheckingProvider, hasProvider, checkProviderError]);

  const handleConfigured = async (providerName: string, modelId?: string) => {
    trackOnboardingProviderSelected({ provider: providerName });
    const providers = await acpListProviderDetails();
    const matchedProvider = providers.find((p) => p.name === providerName);
    const resolvedModel = modelId ?? matchedProvider?.metadata.default_model ?? null;
    await acpSaveDefaults(providerName, resolvedModel);
    setConfiguredModel(resolvedModel);
    await refreshCurrentModelAndProvider();
    setConfiguredProvider(providerName);
    setConfiguredProviderDisplayName(matchedProvider?.metadata.display_name || providerName);
  };

  const finishOnboarding = async (telemetryEnabled: boolean) => {
    try {
      await upsert(TELEMETRY_CONFIG_KEY, telemetryEnabled, false);
    } catch (error) {
      console.error('Failed to save telemetry preference:', error);
    }
    trackTelemetryPreference(telemetryEnabled, 'onboarding');
    if (configuredProvider) {
      trackOnboardingCompleted(configuredProvider, configuredModel ?? undefined);
    }
    if (!telemetryEnabled) {
      setAnalyticsTelemetryEnabled(false);
    }
    navigate('/', { replace: true });
    setHasProvider(true);
  };

  const retryExternalBackend = async () => {
    setIsRetrying(true);
    try {
      const result = await window.electron.switchBackend();
      if (!result.ok) {
        setExternalBackendError(result.error ?? 'Unknown error');
        return;
      }
      setWorkingDir(result.workingDir ?? null);
      reconnectAcpToNewBackend();
      setExternalBackendError('');
      await checkProvider();
    } finally {
      setIsRetrying(false);
    }
  };

  const switchToLocalBackend = async () => {
    setIsRetrying(true);
    try {
      const result = await window.electron.disconnectBackend();
      if (!result.ok) {
        setExternalBackendError(result.error ?? 'Unknown error');
        return;
      }
      setWorkingDir(result.workingDir ?? null);
      reconnectAcpToNewBackend();
      setExternalBackendError('');
      await checkProvider();
    } finally {
      setIsRetrying(false);
    }
  };

  if (externalBackendError) {
    return (
      <div className="h-screen w-full bg-background-default flex flex-col items-center justify-center">
        <div className="text-center max-w-md">
          <div className="mb-4">
            <Goose className="size-8 mx-auto" />
          </div>
          <h1 className="text-xl font-light mb-3">
            {intl.formatMessage(i18n.externalBackendErrorTitle)}
          </h1>
          <p className="text-text-muted mb-6 whitespace-pre-line break-words">
            {externalBackendError}
          </p>
          <div className="flex items-center justify-center gap-2">
            <Button onClick={retryExternalBackend} disabled={isRetrying}>
              {intl.formatMessage(isRetrying ? i18n.retrying : i18n.retry)}
            </Button>
            <Button variant="outline" onClick={switchToLocalBackend} disabled={isRetrying}>
              {intl.formatMessage(i18n.switchToLocal)}
            </Button>
          </div>
        </div>
      </div>
    );
  }

  if (isCheckingProvider) {
    return null;
  }

  if (checkProviderError) {
    return (
      <div className="h-screen w-full bg-background-default flex flex-col items-center justify-center">
        <div className="text-center max-w-md">
          <div className="mb-4">
            <Goose className="size-8 mx-auto" />
          </div>
          <h1 className="text-xl font-light mb-3">
            {intl.formatMessage(i18n.checkProviderErrorTitle)}
          </h1>
          <p className="text-text-muted mb-6">
            {intl.formatMessage(i18n.checkProviderErrorDescription)}
          </p>
          <Button onClick={() => checkProvider()}>{intl.formatMessage(i18n.retry)}</Button>
        </div>
      </div>
    );
  }

  if (hasProvider) {
    return <>{children}</>;
  }

  if (configuredProviderDisplayName) {
    return (
      <OnboardingSuccess providerName={configuredProviderDisplayName} onFinish={finishOnboarding} />
    );
  }

  return (
    <div className="h-screen w-full bg-background-default overflow-hidden">
      <div className="h-full overflow-y-auto">
        <div
          className={`flex flex-col items-center p-4 pb-8 transition-all duration-500 ease-in-out ${hasSelection ? 'pt-8' : 'pt-[15vh]'}`}
        >
          <div className="max-w-2xl w-full mx-auto">
            <div
              className={`text-left transition-all duration-500 ease-in-out overflow-hidden ${hasSelection ? 'max-h-0 opacity-0 mb-0' : 'max-h-60 opacity-100 mb-8'}`}
            >
              <div className="mb-4">
                <Goose className="size-8" />
              </div>
              <h1 className="text-2xl sm:text-4xl font-light mb-3">
                {intl.formatMessage(i18n.welcomeTitle)}
              </h1>
              <p className="text-text-muted text-base sm:text-lg">
                {intl.formatMessage(i18n.welcomeDescription)}
              </p>
            </div>

            <ProviderSelector
              onConfigured={handleConfigured}
              onFirstSelection={() => setHasSelection(true)}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

import { useState, useEffect, useCallback, useRef } from 'react';
import {
  listLocalModels,
  downloadHfModel,
  getLocalModelDownloadProgress,
  cancelLocalModelDownload,
  type DownloadProgress,
  type LocalModelResponse,
} from '../../acp/local-inference';
import { trackOnboardingSetupFailed } from '../../utils/analytics';
import { defineMessages, useIntl } from '../../i18n';
import { errorMessage as formatErrorMessage } from '../../utils/conversionUtils';
import { HuggingFaceModelSearch } from '../settings/localInference/HuggingFaceModelSearch';

const i18n = defineMessages({
  checkingModels: {
    id: 'localModelPicker.checkingModels',
    defaultMessage: 'Checking available models...',
  },
  tryAgain: {
    id: 'localModelPicker.tryAgain',
    defaultMessage: 'Try Again',
  },
  bestForMachine: {
    id: 'localModelPicker.bestForMachine',
    defaultMessage: 'Best for your machine',
  },
  ready: {
    id: 'localModelPicker.ready',
    defaultMessage: 'Ready',
  },
  showOtherSizes: {
    id: 'localModelPicker.showOtherSizes',
    defaultMessage: 'Show {count} other sizes',
  },
  hideOtherSizes: {
    id: 'localModelPicker.hideOtherSizes',
    defaultMessage: 'Hide other sizes',
  },
  selectModel: {
    id: 'localModelPicker.selectModel',
    defaultMessage: 'Select a model',
  },
  useModel: {
    id: 'localModelPicker.useModel',
    defaultMessage: 'Use {modelId}',
  },
  downloadModel: {
    id: 'localModelPicker.downloadModel',
    defaultMessage: 'Download {modelId} ({size})',
  },
  downloading: {
    id: 'localModelPicker.downloading',
    defaultMessage: 'Downloading {modelId}',
  },
  startingDownload: {
    id: 'localModelPicker.startingDownload',
    defaultMessage: 'Starting download...',
  },
  cancelDownload: {
    id: 'localModelPicker.cancelDownload',
    defaultMessage: 'Cancel Download',
  },
  localModelsNote: {
    id: 'localModelPicker.localModelsNote',
    defaultMessage:
      'Local models keep everything on your machine for full privacy. Performance and context window size may vary compared to cloud providers depending on your hardware and model size.',
  },
  failedToLoad: {
    id: 'localModelPicker.failedToLoad',
    defaultMessage: 'Failed to load available models. Please try again.',
  },
  modelNotFound: {
    id: 'localModelPicker.modelNotFound',
    defaultMessage: 'Model not found',
  },
  failedToStartDownload: {
    id: 'localModelPicker.failedToStartDownload',
    defaultMessage: 'Failed to start download. Please try again.',
  },
  lostConnection: {
    id: 'localModelPicker.lostConnection',
    defaultMessage: 'Lost connection to download. Please try again.',
  },
});

interface LocalModelPickerProps {
  onConfigured: (providerName: string, modelId: string) => void | Promise<void>;
}

const formatBytes = (bytes: number): string => {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)}KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(0)}MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)}GB`;
};

const formatSize = (bytes: number): string => {
  const mb = bytes / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)}GB` : `${mb.toFixed(0)}MB`;
};

const LOCAL_PROVIDER = 'local';

type Phase = 'loading' | 'select' | 'downloading' | 'error';

export default function LocalModelPicker({ onConfigured }: LocalModelPickerProps) {
  const intl = useIntl();
  const [phase, setPhase] = useState<Phase>('loading');
  const [models, setModels] = useState<LocalModelResponse[]>([]);
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [showAllModels, setShowAllModels] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const cleanup = useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }, []);

  useEffect(() => cleanup, [cleanup]);

  const finishSetup = useCallback(
    async (modelId: string) => {
      try {
        await onConfigured(LOCAL_PROVIDER, modelId);
      } catch (error) {
        console.error('Failed to finish local model setup:', error);
        setErrorMessage(formatErrorMessage(error));
        trackOnboardingSetupFailed(LOCAL_PROVIDER, 'save_defaults_failed');
        setPhase('error');
      }
    },
    [onConfigured]
  );

  const pollDownload = useCallback(
    (modelId: string) => {
      cleanup();
      pollRef.current = setInterval(async () => {
        try {
          const progress = await getLocalModelDownloadProgress(modelId);
          if (!progress) {
            cleanup();
            setErrorMessage(intl.formatMessage(i18n.lostConnection));
            trackOnboardingSetupFailed(LOCAL_PROVIDER, 'progress_missing');
            setPhase('error');
            return;
          }

          setDownloadProgress(progress);
          if (progress.status === 'completed') {
            cleanup();
            await finishSetup(modelId);
          } else if (progress.status === 'failed') {
            cleanup();
            setErrorMessage(progress.error || 'Download failed.');
            trackOnboardingSetupFailed(LOCAL_PROVIDER, progress.error || 'download_failed');
            setPhase('error');
          } else if (progress.status === 'cancelled') {
            cleanup();
            setPhase('select');
          }
        } catch {
          cleanup();
          setErrorMessage(intl.formatMessage(i18n.lostConnection));
          trackOnboardingSetupFailed(LOCAL_PROVIDER, 'progress_poll_failed');
          setPhase('error');
        }
      }, 500);
    },
    [cleanup, finishSetup, intl]
  );

  useEffect(() => {
    const load = async () => {
      try {
        const models = await listLocalModels();
        if (models) {
          setModels(models);

          const activeDownload = models.find((m) => m.status.state === 'Downloading');
          if (activeDownload) {
            setSelectedModelId(activeDownload.id);
            setPhase('downloading');
            pollDownload(activeDownload.id);
            return;
          }

          const alreadyDownloaded = models.find((m) => m.status.state === 'Downloaded');
          if (alreadyDownloaded) {
            setSelectedModelId(alreadyDownloaded.id);
          } else {
            const recommended = models.find((m: LocalModelResponse) => m.recommended);
            if (recommended) setSelectedModelId(recommended.id);
          }
        }
      } catch (error) {
        console.error('Failed to load local models:', error);
        setErrorMessage(intl.formatMessage(i18n.failedToLoad));
        setPhase('error');
        return;
      }
      setPhase('select');
    };
    load();
  }, [intl, pollDownload]);

  const startDownload = async (modelId: string) => {
    setPhase('downloading');
    setDownloadProgress(null);
    setErrorMessage(null);

    const model = models.find((m) => m.id === modelId);
    if (!model) {
      setErrorMessage(intl.formatMessage(i18n.modelNotFound));
      setPhase('error');
      return;
    }

    try {
      await downloadHfModel({ spec: model.id });
    } catch (error) {
      console.error('Failed to start download:', error);
      setErrorMessage(intl.formatMessage(i18n.failedToStartDownload));
      trackOnboardingSetupFailed(LOCAL_PROVIDER, 'download_start_failed');
      setPhase('error');
      return;
    }

    pollDownload(modelId);
  };

  const handleHfDownloadStarted = (modelId: string) => {
    setSelectedModelId(modelId);
    setDownloadProgress(null);
    setErrorMessage(null);
    setPhase('downloading');
    pollDownload(modelId);
  };

  const handleCancelDownload = async () => {
    if (phase === 'downloading' && selectedModelId) {
      cleanup();
      try {
        await cancelLocalModelDownload(selectedModelId);
      } catch {
        // best-effort
      }
      setDownloadProgress(null);
      setPhase('select');
    }
  };

  const handlePrimaryAction = async () => {
    if (!selectedModelId) return;
    const model = models.find((m) => m.id === selectedModelId);
    if (!model) return;
    if (model.status.state === 'Downloaded') {
      await finishSetup(model.id);
    } else {
      await startDownload(model.id);
    }
  };

  const recommended = models.find((m) => m.recommended);
  const otherModels = models.filter((m) => m.id !== recommended?.id);
  const selectedModel = models.find((m) => m.id === selectedModelId);
  const activeDownloadIds = new Set(
    models.filter((model) => model.status.state === 'Downloading').map((model) => model.id)
  );
  const downloadedModelIds = new Set(
    models.filter((model) => model.status.state === 'Downloaded').map((model) => model.id)
  );

  if (phase === 'loading') {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <div className="animate-spin rounded-full h-8 w-8 border-t-2 border-b-2 border-text-muted mb-4"></div>
        <p className="text-text-muted text-sm">{intl.formatMessage(i18n.checkingModels)}</p>
      </div>
    );
  }

  return (
    <div>
      <div className="p-4 border rounded-xl bg-background-muted">
        {phase === 'error' && (
          <div className="space-y-3">
            <div className="border border-red-300 dark:border-red-700 bg-red-50 dark:bg-red-900/20 rounded-lg p-3">
              <p className="text-sm text-red-700 dark:text-red-400">{errorMessage}</p>
            </div>
            <button
              onClick={() => {
                setErrorMessage(null);
                setPhase('select');
              }}
              className="w-full px-4 py-2 bg-transparent border rounded-lg text-text-default text-sm font-medium hover:bg-background-muted/80 transition-colors"
            >
              {intl.formatMessage(i18n.tryAgain)}
            </button>
          </div>
        )}

        {phase === 'select' && (
          <div className="space-y-3">
            {recommended && (
              <div
                onClick={() => setSelectedModelId(recommended.id)}
                className={`relative w-full p-4 border rounded-lg cursor-pointer transition-all duration-200 ${
                  selectedModelId === recommended.id
                    ? 'border-blue-500 bg-blue-500/5'
                    : 'border-border-subtle hover:border-border-default'
                }`}
              >
                <div className="absolute -top-2 -right-2 z-10">
                  <span className="inline-block px-2 py-0.5 text-xs font-medium bg-blue-600 text-white rounded-full">
                    {intl.formatMessage(i18n.bestForMachine)}
                  </span>
                </div>
                <div className="flex items-start gap-3">
                  <input
                    type="radio"
                    checked={selectedModelId === recommended.id}
                    onChange={() => setSelectedModelId(recommended.id)}
                    className="cursor-pointer flex-shrink-0 mt-1"
                  />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="font-medium text-text-default text-sm">
                        {recommended.id}
                      </span>
                      {recommended.status.state === 'Downloaded' && (
                        <span className="text-xs bg-green-600 text-white px-2 py-0.5 rounded-full">
                          {intl.formatMessage(i18n.ready)}
                        </span>
                      )}
                    </div>
                    <p className="text-text-muted text-xs mt-1">
                      {formatSize(recommended.sizeBytes)}
                    </p>
                  </div>
                </div>
              </div>
            )}

            {otherModels.length > 0 && (
              <div>
                <button
                  onClick={() => setShowAllModels(!showAllModels)}
                  className="text-sm text-blue-500 hover:text-blue-400 transition-colors flex items-center gap-1"
                >
                  {showAllModels
                    ? intl.formatMessage(i18n.hideOtherSizes)
                    : intl.formatMessage(i18n.showOtherSizes, { count: otherModels.length })}
                  <svg
                    className={`w-3.5 h-3.5 transition-transform ${showAllModels ? 'rotate-180' : ''}`}
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={2}
                      d="M19 9l-7 7-7-7"
                    />
                  </svg>
                </button>

                {showAllModels && (
                  <div className="mt-2 space-y-2">
                    {otherModels.map((model) => (
                      <div
                        key={model.id}
                        onClick={() => setSelectedModelId(model.id)}
                        className={`w-full p-4 border rounded-lg cursor-pointer transition-all duration-200 ${
                          selectedModelId === model.id
                            ? 'border-blue-500 bg-blue-500/5'
                            : 'border-border-subtle hover:border-border-default'
                        }`}
                      >
                        <div className="flex items-start gap-3">
                          <input
                            type="radio"
                            checked={selectedModelId === model.id}
                            onChange={() => setSelectedModelId(model.id)}
                            className="cursor-pointer flex-shrink-0 mt-0.5"
                          />
                          <div className="flex-1 min-w-0">
                            <div className="flex items-center gap-2 flex-wrap">
                              <span className="font-medium text-text-default text-sm">
                                {model.id}
                              </span>
                              <span className="text-xs text-text-muted">
                                {formatSize(model.sizeBytes)}
                              </span>
                              {model.status.state === 'Downloaded' && (
                                <span className="text-xs bg-green-600 text-white px-2 py-0.5 rounded-full">
                                  Ready
                                </span>
                              )}
                            </div>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}

            <button
              onClick={handlePrimaryAction}
              disabled={!selectedModelId}
              className="w-full px-4 py-2.5 bg-blue-600 rounded-lg text-white text-sm font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed hover:bg-blue-700 cursor-pointer"
            >
              {selectedModel?.status.state === 'Downloaded'
                ? intl.formatMessage(i18n.useModel, { modelId: selectedModel.id })
                : selectedModel
                  ? intl.formatMessage(i18n.downloadModel, {
                      modelId: selectedModel.id,
                      size: formatSize(selectedModel.sizeBytes),
                    })
                  : intl.formatMessage(i18n.selectModel)}
            </button>

            <div className="border-t border-border-subtle pt-3">
              <HuggingFaceModelSearch
                onDownloadStarted={handleHfDownloadStarted}
                activeDownloadIds={activeDownloadIds}
                downloadedModelIds={downloadedModelIds}
              />
            </div>
          </div>
        )}

        {phase === 'downloading' && selectedModelId && (
          <div className="space-y-3">
            <div className="border border-border-subtle rounded-lg p-4 bg-background-default">
              <p className="font-medium text-text-default text-sm mb-3">
                {intl.formatMessage(i18n.downloading, { modelId: selectedModelId })}
              </p>

              {downloadProgress ? (
                <div className="space-y-2">
                  <div className="w-full bg-background-subtle rounded-full h-2 overflow-hidden">
                    <div
                      className="bg-blue-500 h-2 rounded-full transition-all duration-500 ease-out"
                      style={{ width: `${downloadProgress.progressPercent}%` }}
                    />
                  </div>

                  <div className="flex justify-between text-xs text-text-muted">
                    <span>
                      {formatBytes(downloadProgress.bytesDownloaded)} of{' '}
                      {formatBytes(downloadProgress.totalBytes)}
                    </span>
                    <span>{downloadProgress.progressPercent.toFixed(0)}%</span>
                  </div>

                  <div className="flex justify-between text-xs text-text-muted">
                    {downloadProgress.speedBps ? (
                      <span>{formatBytes(downloadProgress.speedBps)}/s</span>
                    ) : (
                      <span />
                    )}
                    {downloadProgress.etaSeconds != null && downloadProgress.etaSeconds > 0 && (
                      <span>
                        ~
                        {downloadProgress.etaSeconds < 60
                          ? `${Math.round(downloadProgress.etaSeconds)}s`
                          : `${Math.round(downloadProgress.etaSeconds / 60)}m`}{' '}
                        remaining
                      </span>
                    )}
                  </div>
                </div>
              ) : (
                <div className="flex items-center gap-3">
                  <div className="animate-spin rounded-full h-4 w-4 border-t-2 border-b-2 border-text-muted"></div>
                  <span className="text-sm text-text-muted">
                    {intl.formatMessage(i18n.startingDownload)}
                  </span>
                </div>
              )}
            </div>

            <button
              onClick={handleCancelDownload}
              className="w-full px-4 py-2.5 bg-transparent text-text-muted border rounded-lg text-sm hover:bg-background-default/80 transition-colors"
            >
              {intl.formatMessage(i18n.cancelDownload)}
            </button>
          </div>
        )}
      </div>
      <div className="rounded-lg bg-yellow-50/50 dark:bg-yellow-900/10 p-3 mt-3">
        <p className="text-sm text-yellow-700 dark:text-yellow-300 leading-relaxed">
          {intl.formatMessage(i18n.localModelsNote)}
        </p>
      </div>
    </div>
  );
}

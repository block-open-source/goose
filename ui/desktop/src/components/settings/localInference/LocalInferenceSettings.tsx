import { useState, useEffect, useCallback, useRef } from 'react';
import { Trash2, X, Settings2, Eye, RefreshCw, Cpu, PowerOff } from 'lucide-react';
import { Button } from '../../ui/button';
import { useModelAndProvider } from '../../ModelAndProviderContext';
import { defineMessages, useIntl } from '../../../i18n';
import {
  listLocalModels,
  downloadHfModel,
  getLocalModelDownloadProgress,
  cancelLocalModelDownload,
  deleteLocalModel,
  evictLocalModel,
  type DownloadProgress,
  type DownloadModelRequest,
  type LocalModelResponse,
} from '../../../acp/local-inference';
import { HuggingFaceModelSearch } from './HuggingFaceModelSearch';
import { ModelSettingsPanel } from './ModelSettingsPanel';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '../../ui/dialog';
import HuggingFaceSignInPrompt from '../auth/HuggingFaceSignInPrompt';
import { acpSaveDefaults } from '../../../acp/providers';

const i18n = defineMessages({
  title: {
    id: 'localInferenceSettings.title',
    defaultMessage: 'Local Inference Models',
  },
  description: {
    id: 'localInferenceSettings.description',
    defaultMessage:
      'Download and manage local LLM models for inference without API keys. Search Hugging Face for GGUF or MLX models.',
  },
  downloading: {
    id: 'localInferenceSettings.downloading',
    defaultMessage: 'Downloading',
  },
  downloadedModels: {
    id: 'localInferenceSettings.downloadedModels',
    defaultMessage: 'Downloaded Models',
  },
  recommended: {
    id: 'localInferenceSettings.recommended',
    defaultMessage: 'Recommended',
  },
  modelSettings: {
    id: 'localInferenceSettings.modelSettings',
    defaultMessage: 'Model Settings',
  },
  noModels: {
    id: 'localInferenceSettings.noModels',
    defaultMessage: 'No models available',
  },
  downloadProgress: {
    id: 'localInferenceSettings.downloadProgress',
    defaultMessage: '{downloaded} / {total} ({percent}%)',
  },
  remaining: {
    id: 'localInferenceSettings.remaining',
    defaultMessage: '{time} remaining',
  },
  downloadFailed: {
    id: 'localInferenceSettings.downloadFailed',
    defaultMessage: 'Download failed',
  },
  downloadCancelled: {
    id: 'localInferenceSettings.downloadCancelled',
    defaultMessage: 'Download cancelled',
  },
  retry: {
    id: 'localInferenceSettings.retry',
    defaultMessage: 'Retry',
  },
  dismiss: {
    id: 'localInferenceSettings.dismiss',
    defaultMessage: 'Dismiss',
  },
  deleteConfirm: {
    id: 'localInferenceSettings.deleteConfirm',
    defaultMessage: 'Delete this model? You can re-download it later.',
  },
  modelSettingsTitle: {
    id: 'localInferenceSettings.modelSettingsTitle',
    defaultMessage: 'Model settings',
  },
  loadedInMemory: {
    id: 'localInferenceSettings.loadedInMemory',
    defaultMessage: 'Loaded in memory',
  },
  evictFromMemory: {
    id: 'localInferenceSettings.evictFromMemory',
    defaultMessage: 'Evict from memory',
  },
  vision: {
    id: 'localInferenceSettings.vision',
    defaultMessage: 'Vision',
  },
  visionEncoderDownloading: {
    id: 'localInferenceSettings.visionEncoderDownloading',
    defaultMessage: 'Vision encoder downloading…',
  },
  visionEncoderNotDownloaded: {
    id: 'localInferenceSettings.visionEncoderNotDownloaded',
    defaultMessage: 'Vision encoder not downloaded',
  },
  huggingFaceSignInNote: {
    id: 'localInferenceSettings.huggingFaceSignInNote',
    defaultMessage:
      'Sign in to increase rate limits when searching and downloading models, and to access private or gated Hugging Face repositories.',
  },
});

const VisionBadge = ({
  model,
  intl,
}: {
  model: LocalModelResponse;
  intl: ReturnType<typeof useIntl>;
}) => {
  if (!model.visionCapable) return null;

  const mmproj = model.mmprojStatus;
  const isDownloaded = mmproj?.state === 'Downloaded';
  const isDownloading = mmproj?.state === 'Downloading';

  if (isDownloaded) {
    return (
      <span className="inline-flex items-center gap-1 text-xs text-green-400 bg-green-500/10 px-2 py-0.5 rounded">
        <Eye className="w-3 h-3" />
        {intl.formatMessage(i18n.vision)}
      </span>
    );
  }

  if (isDownloading) {
    const percent =
      mmproj && mmproj.progressPercent != null ? Math.round(mmproj.progressPercent) : null;
    return (
      <span className="inline-flex items-center gap-1 text-xs text-yellow-400 bg-yellow-500/10 px-2 py-0.5 rounded">
        <Eye className="w-3 h-3" />
        {intl.formatMessage(i18n.visionEncoderDownloading)}
        {percent != null && ` ${percent}%`}
      </span>
    );
  }

  return (
    <span className="inline-flex items-center gap-1 text-xs text-text-muted bg-background-subtle px-2 py-0.5 rounded">
      <Eye className="w-3 h-3" />
      {intl.formatMessage(i18n.vision)}
    </span>
  );
};

const formatBytes = (bytes: number): string => {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)}KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(0)}MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)}GB`;
};

export const LocalInferenceSettings = () => {
  const intl = useIntl();
  const [models, setModels] = useState<LocalModelResponse[]>([]);
  const [downloads, setDownloads] = useState<Map<string, DownloadProgress>>(new Map());
  const [downloadRequests, setDownloadRequests] = useState<Map<string, DownloadModelRequest>>(
    new Map()
  );
  const [evictingModelId, setEvictingModelId] = useState<string | null>(null);
  const [settingsOpenFor, setSettingsOpenFor] = useState<string | null>(null);
  const { currentModel, currentProvider, refreshCurrentModelAndProvider } = useModelAndProvider();
  const downloadSectionRef = useRef<HTMLDivElement>(null);
  const activePolls = useRef(new Set<string>());
  const selectedModelId = currentProvider === 'local' ? currentModel : null;

  const loadModels = useCallback(async (): Promise<LocalModelResponse[] | undefined> => {
    try {
      const models = await listLocalModels();
      if (models) {
        setModels(models);
        const downloadedIds = new Set(
          models.filter((model) => model.status.state === 'Downloaded').map((model) => model.id)
        );
        if (downloadedIds.size > 0) {
          setDownloads((prev) => {
            const next = new Map(prev);
            downloadedIds.forEach((modelId) => next.delete(modelId));
            return next;
          });
          setDownloadRequests((prev) => {
            const next = new Map(prev);
            downloadedIds.forEach((modelId) => next.delete(modelId));
            return next;
          });
        }
        models.forEach((model) => {
          if (model.status.state === 'Downloading') {
            pollDownloadProgress(model.id);
          }
        });

        return models;
      }
    } catch (error) {
      console.error('Failed to load models:', error);
    }
    return undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    loadModels();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Poll model list while any vision encoder is downloading
  useEffect(() => {
    const hasDownloadingMmproj = models.some(
      (m) => m.visionCapable && m.mmprojStatus?.state === 'Downloading'
    );
    if (!hasDownloadingMmproj) return;

    const interval = setInterval(() => {
      loadModels();
    }, 2000);
    return () => clearInterval(interval);
  }, [models, loadModels]);

  const selectModel = async (modelId: string) => {
    try {
      await acpSaveDefaults('local', modelId);
      await refreshCurrentModelAndProvider();
    } catch (error) {
      console.error('Failed to select model:', error);
    }
  };

  const scrollToDownloads = useCallback(() => {
    requestAnimationFrame(() => {
      downloadSectionRef.current?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
  }, []);

  const pollDownloadProgress = (modelId: string) => {
    if (activePolls.current.has(modelId)) return;
    activePolls.current.add(modelId);

    const stopPolling = (interval: ReturnType<typeof setInterval>) => {
      clearInterval(interval);
      activePolls.current.delete(modelId);
    };

    const interval = setInterval(async () => {
      try {
        const progress = await getLocalModelDownloadProgress(modelId);
        if (progress) {
          setDownloads((prev) => new Map(prev).set(modelId, progress));

          if (progress.status === 'completed') {
            stopPolling(interval);
            setDownloads((prev) => {
              const next = new Map(prev);
              next.delete(modelId);
              return next;
            });
            await loadModels();
            await selectModel(modelId);
          } else if (progress.status === 'failed' || progress.status === 'cancelled') {
            stopPolling(interval);
            await loadModels();
          }
        } else {
          stopPolling(interval);
        }
      } catch {
        stopPolling(interval);
      }
    }, 1000);
  };

  const cancelDownload = async (modelId: string) => {
    try {
      await cancelLocalModelDownload(modelId);
      setDownloads((prev) => {
        const next = new Map(prev);
        const progress = next.get(modelId);
        if (progress) {
          next.set(modelId, { ...progress, status: 'cancelled' });
        }
        return next;
      });
      await loadModels();
    } catch (error) {
      console.error('Failed to cancel download:', error);
    }
  };

  const dismissDownload = (modelId: string) => {
    setDownloads((prev) => {
      const next = new Map(prev);
      next.delete(modelId);
      return next;
    });
    setDownloadRequests((prev) => {
      const next = new Map(prev);
      next.delete(modelId);
      return next;
    });
  };

  const retryDownload = async (modelId: string) => {
    const request = downloadRequests.get(modelId) ?? { spec: modelId };
    dismissDownload(modelId);
    try {
      const nextModelId = await downloadHfModel(request);
      setDownloadRequests((prev) => new Map(prev).set(nextModelId, request));
      pollDownloadProgress(nextModelId);
      scrollToDownloads();
    } catch (error) {
      console.error('Failed to retry download:', error);
    }
  };

  const handleDeleteModel = async (modelId: string) => {
    if (!window.confirm(intl.formatMessage(i18n.deleteConfirm))) return;
    try {
      await deleteLocalModel(modelId);
      const updatedModels = await loadModels();

      if (selectedModelId === modelId && updatedModels) {
        const remainingDownloaded = updatedModels.filter(
          (m) => m.id !== modelId && m.status.state === 'Downloaded'
        );
        if (remainingDownloaded.length > 0) {
          selectModel(remainingDownloaded[0].id);
        }
      }
    } catch (error) {
      console.error('Failed to delete model:', error);
    }
  };

  const handleEvictModel = async (modelId: string) => {
    setEvictingModelId(modelId);
    try {
      await evictLocalModel(modelId);
      await loadModels();
    } catch (error) {
      console.error('Failed to evict model:', error);
    } finally {
      setEvictingModelId(null);
    }
  };

  const handleHfDownloadStarted = (modelId: string, request: DownloadModelRequest) => {
    setDownloadRequests((prev) => new Map(prev).set(modelId, request));
    pollDownloadProgress(modelId);
    loadModels();
    scrollToDownloads();
  };

  const isDownloaded = (model: LocalModelResponse) => model.status.state === 'Downloaded';

  const downloadedModels = models.filter(isDownloaded);
  const activeDownloadIds = new Set(
    Array.from(downloads.entries())
      .filter(([, progress]) => progress.status === 'downloading')
      .map(([modelId]) => modelId)
  );

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-text-default font-medium">{intl.formatMessage(i18n.title)}</h3>
        <p className="text-xs text-text-muted max-w-2xl mt-1">
          {intl.formatMessage(i18n.description)}
        </p>
      </div>

      <HuggingFaceSignInPrompt description={intl.formatMessage(i18n.huggingFaceSignInNote)} />

      {/* Active Downloads */}
      {downloads.size > 0 && (
        <div ref={downloadSectionRef}>
          <h4 className="text-sm font-medium text-text-default mb-2">
            {intl.formatMessage(i18n.downloading)}
          </h4>
          <div className="space-y-2">
            {Array.from(downloads.entries()).map(([modelId, progress]) => {
              if (progress.status === 'completed') return null;
              return (
                <div
                  key={modelId}
                  className="border rounded-lg p-3 border-border-subtle bg-background-default"
                >
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-sm font-medium text-text-default truncate">
                      {modelId}
                    </span>
                    {progress.status === 'downloading' && (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => cancelDownload(modelId)}
                        className="text-destructive hover:text-destructive"
                      >
                        <X className="w-4 h-4" />
                      </Button>
                    )}
                  </div>
                  {progress.status === 'downloading' && (
                    <div className="space-y-1">
                      <div className="w-full bg-gray-700 rounded-full h-2">
                        <div
                          className="bg-blue-500 h-2 rounded-full transition-all duration-300"
                          style={{ width: `${progress.progressPercent}%` }}
                        />
                      </div>
                      <div className="flex justify-between text-xs text-text-muted">
                        <span>
                          {intl.formatMessage(i18n.downloadProgress, {
                            downloaded: formatBytes(progress.bytesDownloaded),
                            total: formatBytes(progress.totalBytes),
                            percent: progress.progressPercent.toFixed(0),
                          })}
                        </span>
                        <span className="flex gap-2">
                          {progress.etaSeconds != null && progress.etaSeconds > 0 && (
                            <span>
                              {intl.formatMessage(i18n.remaining, {
                                time:
                                  progress.etaSeconds < 60
                                    ? `${Math.round(progress.etaSeconds)}s`
                                    : `${Math.round(progress.etaSeconds / 60)}m`,
                              })}
                            </span>
                          )}
                          {progress.speedBps != null && progress.speedBps > 0 && (
                            <span>{formatBytes(progress.speedBps)}/s</span>
                          )}
                        </span>
                      </div>
                    </div>
                  )}
                  {progress.status === 'failed' && (
                    <div className="space-y-2">
                      <p className="text-xs text-destructive">
                        {progress.error || intl.formatMessage(i18n.downloadFailed)}
                      </p>
                      <div className="flex gap-2">
                        <Button variant="outline" size="sm" onClick={() => retryDownload(modelId)}>
                          <RefreshCw className="w-3 h-3 mr-1" />
                          {intl.formatMessage(i18n.retry)}
                        </Button>
                        <Button variant="ghost" size="sm" onClick={() => dismissDownload(modelId)}>
                          <X className="w-3 h-3 mr-1" />
                          {intl.formatMessage(i18n.dismiss)}
                        </Button>
                      </div>
                    </div>
                  )}
                  {progress.status === 'cancelled' && (
                    <div className="space-y-2">
                      <p className="text-xs text-text-muted">
                        {progress.error || intl.formatMessage(i18n.downloadCancelled)}
                      </p>
                      <div className="flex gap-2">
                        <Button variant="outline" size="sm" onClick={() => retryDownload(modelId)}>
                          <RefreshCw className="w-3 h-3 mr-1" />
                          {intl.formatMessage(i18n.retry)}
                        </Button>
                        <Button variant="ghost" size="sm" onClick={() => dismissDownload(modelId)}>
                          <X className="w-3 h-3 mr-1" />
                          {intl.formatMessage(i18n.dismiss)}
                        </Button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Downloaded Models */}
      {downloadedModels.length > 0 && (
        <div>
          <h4 className="text-sm font-medium text-text-default mb-2">
            {intl.formatMessage(i18n.downloadedModels)}
          </h4>
          <div className="space-y-2">
            {downloadedModels.map((model) => {
              const isSelected = selectedModelId === model.id;
              return (
                <div
                  key={model.id}
                  className={`border rounded-lg p-3 transition-colors ${
                    isSelected
                      ? 'border-accent-primary bg-accent-primary/5'
                      : 'border-border-subtle bg-background-default hover:border-border-default'
                  }`}
                >
                  <div className="flex items-center justify-between gap-3">
                    <div className="flex min-w-0 items-center gap-2 flex-wrap">
                      <input
                        type="radio"
                        checked={isSelected}
                        onChange={() => selectModel(model.id)}
                        className="cursor-pointer"
                      />
                      <span className="text-sm font-medium text-text-default break-all">
                        {model.id}
                      </span>
                      <span className="text-xs text-text-muted">
                        {formatBytes(model.sizeBytes)}
                      </span>
                      {model.isLoaded && (
                        <span className="inline-flex items-center gap-1 text-xs text-green-400 bg-green-500/10 px-2 py-0.5 rounded">
                          <Cpu className="w-3 h-3" />
                          {intl.formatMessage(i18n.loadedInMemory)}
                        </span>
                      )}
                      {model.recommended && (
                        <span className="text-xs bg-blue-500 text-white px-2 py-0.5 rounded">
                          {intl.formatMessage(i18n.recommended)}
                        </span>
                      )}
                      <VisionBadge model={model} intl={intl} />
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => setSettingsOpenFor(model.id)}
                        title={intl.formatMessage(i18n.modelSettingsTitle)}
                      >
                        <Settings2 className="w-4 h-4" />
                      </Button>
                      {model.isLoaded && (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => handleEvictModel(model.id)}
                          disabled={evictingModelId === model.id}
                          title={intl.formatMessage(i18n.evictFromMemory)}
                        >
                          <PowerOff className="w-4 h-4" />
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => handleDeleteModel(model.id)}
                        className="text-destructive hover:text-destructive"
                      >
                        <Trash2 className="w-4 h-4" />
                      </Button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* HuggingFace Search */}
      <div className="border-t border-border-subtle pt-4">
        <HuggingFaceModelSearch
          onDownloadStarted={handleHfDownloadStarted}
          activeDownloadIds={activeDownloadIds}
          downloadedModelIds={
            new Set(models.filter((m) => m.status.state === 'Downloaded').map((m) => m.id))
          }
        />
      </div>

      {models.length === 0 && (
        <div className="text-center py-6 text-text-muted text-sm">
          {intl.formatMessage(i18n.noModels)}
        </div>
      )}

      <Dialog
        open={!!settingsOpenFor}
        onOpenChange={(open) => {
          if (!open) setSettingsOpenFor(null);
        }}
      >
        <DialogContent className="max-h-[80vh] overflow-y-auto sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>{intl.formatMessage(i18n.modelSettings)}</DialogTitle>
            <p className="text-sm text-text-muted">{settingsOpenFor || ''}</p>
          </DialogHeader>
          {settingsOpenFor && <ModelSettingsPanel modelId={settingsOpenFor} />}
        </DialogContent>
      </Dialog>
    </div>
  );
};

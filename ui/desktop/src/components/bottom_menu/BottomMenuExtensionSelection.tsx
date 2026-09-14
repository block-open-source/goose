import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useConfig, type FixedExtensionEntry } from '../ConfigContext';
import { toastService } from '../../toasts';
import { formatExtensionName } from '../settings/extensions/subcomponents/ExtensionList';
import {
  getSessionExtensions as getAcpSessionExtensions,
  type SessionExtension,
} from '../../acp/session-extensions';
import { addToAgent, removeFromAgent } from '../settings/extensions/agent-api';
import { defineMessages, useIntl } from '../../i18n';
import { AppEvents } from '../../constants/events';
import { ExtensionMenu } from './ExtensionMenu';
import {
  isNextChatExtensionSelected,
  toggleNextChatExtension,
  type NextChatExtensionDraft,
} from '../../utils/nextChatExtensions';

const i18n = defineMessages({
  manageExtensions: {
    id: 'bottomMenuExtensionSelection.manageExtensions',
    defaultMessage: 'manage extensions',
  },
  searchExtensions: {
    id: 'bottomMenuExtensionSelection.searchExtensions',
    defaultMessage: 'search extensions...',
  },
  extensionsForNewChats: {
    id: 'bottomMenuExtensionSelection.extensionsForNewChats',
    defaultMessage: 'Extensions for new chats',
  },
  extensionsForThisSession: {
    id: 'bottomMenuExtensionSelection.extensionsForThisSession',
    defaultMessage: 'Extensions for this chat session',
  },
  noExtensionsFound: {
    id: 'bottomMenuExtensionSelection.noExtensionsFound',
    defaultMessage: 'no extensions found',
  },
  noExtensionsAvailable: {
    id: 'bottomMenuExtensionSelection.noExtensionsAvailable',
    defaultMessage: 'no extensions available',
  },
  extensionUpdated: {
    id: 'bottomMenuExtensionSelection.extensionUpdated',
    defaultMessage: 'Extension Updated',
  },
  extensionWillBeEnabled: {
    id: 'bottomMenuExtensionSelection.extensionWillBeEnabled',
    defaultMessage: '{name} will be enabled in new chats',
  },
  extensionWillBeDisabled: {
    id: 'bottomMenuExtensionSelection.extensionWillBeDisabled',
    defaultMessage: '{name} will be disabled in new chats',
  },
});

interface BottomMenuExtensionSelectionProps {
  sessionId: string | null;
  nextChatExtensionDraft?: NextChatExtensionDraft;
  onNextChatExtensionDraftChange?: (draft: NextChatExtensionDraft) => void;
}

type GetSessionExtensionsSignal = { aborted: boolean };

const EXTENSION_SORT_DELAY_MS = 800;

function mergeSessionExtensions(
  configuredExtensions: FixedExtensionEntry[],
  sessionExtensions: SessionExtension[]
): FixedExtensionEntry[] | null {
  const sessionExtensionsByKey = new Map<string, SessionExtension>();
  for (const extension of sessionExtensions) {
    if (sessionExtensionsByKey.has(extension.extensionKey)) {
      return null;
    }
    sessionExtensionsByKey.set(extension.extensionKey, extension);
  }

  const configuredExtensionKeys = new Set<string>();
  const mergedExtensions: FixedExtensionEntry[] = [];
  for (const extension of configuredExtensions) {
    if (extension.configKey === undefined || configuredExtensionKeys.has(extension.configKey)) {
      return null;
    }
    configuredExtensionKeys.add(extension.configKey);

    const sessionExtension = sessionExtensionsByKey.get(extension.configKey);
    mergedExtensions.push({
      ...extension,
      enabled: sessionExtension !== undefined,
      extensionKey: sessionExtension?.extensionKey,
    });
  }

  for (const sessionExtension of sessionExtensions) {
    if (!configuredExtensionKeys.has(sessionExtension.extensionKey)) {
      mergedExtensions.push({ ...sessionExtension, enabled: true });
    }
  }

  return mergedExtensions;
}

function useExtensionMenuTransition() {
  const [isTransitioning, setIsTransitioning] = useState(false);
  const [isSortPending, setIsSortPending] = useState(false);
  const [togglingExtensionName, setTogglingExtensionName] = useState<string | null>(null);
  const sortTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearSortTimeout = useCallback(() => {
    if (sortTimeoutRef.current) {
      clearTimeout(sortTimeoutRef.current);
      sortTimeoutRef.current = null;
    }
  }, []);

  const resetTransition = useCallback(() => {
    clearSortTimeout();
    setIsTransitioning(false);
    setIsSortPending(false);
    setTogglingExtensionName(null);
  }, [clearSortTimeout]);

  const beginToggle = useCallback(
    (extensionName: string) => {
      if (togglingExtensionName === extensionName) {
        return false;
      }

      setIsTransitioning(true);
      setTogglingExtensionName(extensionName);
      return true;
    },
    [togglingExtensionName]
  );

  const finishTransition = useCallback(() => {
    setIsSortPending(false);
    setIsTransitioning(false);
    setTogglingExtensionName(null);
  }, []);

  const scheduleSort = useCallback(
    (
      callback: () => void | Promise<void>,
      options?: {
        shouldFinish?: () => boolean;
      }
    ) => {
      setIsSortPending(true);
      clearSortTimeout();

      sortTimeoutRef.current = setTimeout(() => {
        Promise.resolve()
          .then(callback)
          .finally(() => {
            sortTimeoutRef.current = null;
            if (options?.shouldFinish?.() ?? true) {
              finishTransition();
            }
          });
      }, EXTENSION_SORT_DELAY_MS);
    },
    [clearSortTimeout, finishTransition]
  );

  useEffect(() => clearSortTimeout, [clearSortTimeout]);

  return {
    isTransitioning,
    isSortPending,
    togglingExtensionName,
    beginToggle,
    scheduleSort,
    resetTransition,
  };
}

export const BottomMenuExtensionSelection = ({
  sessionId,
  nextChatExtensionDraft,
  onNextChatExtensionDraftChange,
}: BottomMenuExtensionSelectionProps) => {
  if (!sessionId) {
    if (!nextChatExtensionDraft || !onNextChatExtensionDraftChange) {
      return null;
    }

    return (
      <DraftExtensionsMenu
        draft={nextChatExtensionDraft}
        onDraftChange={onNextChatExtensionDraftChange}
      />
    );
  }

  return <SessionExtensionsMenu sessionId={sessionId} />;
};

function DraftExtensionsMenu({
  draft,
  onDraftChange,
}: {
  draft: NextChatExtensionDraft;
  onDraftChange: (draft: NextChatExtensionDraft) => void;
}) {
  const intl = useIntl();
  const { extensionsList: allExtensions } = useConfig();
  const [visibleDraft, setVisibleDraft] = useState<NextChatExtensionDraft>(draft);
  const {
    isTransitioning,
    isSortPending,
    togglingExtensionName,
    beginToggle,
    scheduleSort,
    resetTransition,
  } = useExtensionMenuTransition();

  useEffect(() => {
    if (!isTransitioning) {
      setVisibleDraft(draft);
    }
  }, [draft, isTransitioning]);

  const handleToggle = useCallback(
    (extensionConfig: FixedExtensionEntry) => {
      if (!beginToggle(extensionConfig.name)) {
        return;
      }

      const currentState = isNextChatExtensionSelected(extensionConfig, draft);
      const nextDraft = toggleNextChatExtension(draft, extensionConfig);
      onDraftChange(nextDraft);
      scheduleSort(() => {
        setVisibleDraft(nextDraft);
      });

      toastService.success({
        title: intl.formatMessage(i18n.extensionUpdated),
        msg: intl.formatMessage(
          !currentState ? i18n.extensionWillBeEnabled : i18n.extensionWillBeDisabled,
          { name: formatExtensionName(extensionConfig.name) }
        ),
      });
    },
    [beginToggle, draft, intl, onDraftChange, scheduleSort]
  );

  const extensions = useMemo(() => {
    return allExtensions.map(
      (extension) =>
        ({
          ...extension,
          enabled: isNextChatExtensionSelected(extension, visibleDraft),
        }) as FixedExtensionEntry
    );
  }, [allExtensions, visibleDraft]);

  return (
    <ExtensionMenu
      extensions={extensions}
      title={intl.formatMessage(i18n.manageExtensions)}
      searchPlaceholder={intl.formatMessage(i18n.searchExtensions)}
      description={intl.formatMessage(i18n.extensionsForNewChats)}
      emptyMessage={intl.formatMessage(i18n.noExtensionsAvailable)}
      noResultsMessage={intl.formatMessage(i18n.noExtensionsFound)}
      hidden={extensions.length === 0}
      isTransitioning={isTransitioning}
      isSortPending={isSortPending}
      togglingExtensionName={togglingExtensionName}
      onToggle={handleToggle}
      onClose={resetTransition}
    />
  );
}

function SessionExtensionsMenu({ sessionId }: { sessionId: string }) {
  const intl = useIntl();
  const [sessionExtensions, setSessionExtensions] = useState<SessionExtension[]>([]);
  const [isSessionExtensionsLoaded, setIsSessionExtensionsLoaded] = useState(false);
  const latestSessionIdRef = useRef(sessionId);
  const { extensionsList: allExtensions } = useConfig();
  const {
    isTransitioning,
    isSortPending,
    togglingExtensionName,
    beginToggle,
    scheduleSort,
    resetTransition,
  } = useExtensionMenuTransition();

  useEffect(() => {
    latestSessionIdRef.current = sessionId;
    setIsSessionExtensionsLoaded(false);
    setSessionExtensions([]);
    resetTransition();
  }, [sessionId, resetTransition]);

  const loadSessionExtensions = useCallback(
    async (targetSessionId: string, signal?: GetSessionExtensionsSignal) => {
      const extensions = await getAcpSessionExtensions(targetSessionId);

      if (signal?.aborted || latestSessionIdRef.current !== targetSessionId) {
        return;
      }

      setSessionExtensions(extensions);
      setIsSessionExtensionsLoaded(true);
    },
    []
  );

  useEffect(() => {
    let controller: AbortController | null = null;

    const loadForSession = (targetSessionId: string) => {
      controller?.abort();
      const currentController = new AbortController();
      controller = currentController;

      loadSessionExtensions(targetSessionId, currentController.signal).catch((error) => {
        if (currentController.signal.aborted || latestSessionIdRef.current !== targetSessionId) {
          return;
        }

        console.error('Failed to fetch session extensions:', error);
        setIsSessionExtensionsLoaded(false);
      });
    };

    const loadExtensionsForCurrentSession = (event: Event) => {
      const targetSessionId = (event as CustomEvent<{ sessionId?: string }>).detail?.sessionId;

      if (targetSessionId !== sessionId) {
        return;
      }

      loadForSession(targetSessionId);
    };

    window.addEventListener(AppEvents.SESSION_EXTENSIONS_LOADED, loadExtensionsForCurrentSession);
    loadForSession(sessionId);

    return () => {
      controller?.abort();
      window.removeEventListener(
        AppEvents.SESSION_EXTENSIONS_LOADED,
        loadExtensionsForCurrentSession
      );
    };
  }, [sessionId, loadSessionExtensions]);

  const handleToggle = useCallback(
    async (extensionConfig: FixedExtensionEntry) => {
      if (!beginToggle(extensionConfig.name)) {
        return;
      }

      try {
        if (extensionConfig.enabled) {
          if (extensionConfig.extensionKey === undefined) {
            throw new Error('Missing session extension key');
          }
          await removeFromAgent(
            extensionConfig.extensionKey,
            extensionConfig.name,
            sessionId,
            true
          );
        } else {
          await addToAgent(extensionConfig, sessionId, true);
        }

        scheduleSort(
          () =>
            loadSessionExtensions(sessionId).catch((error) => {
              if (latestSessionIdRef.current === sessionId) {
                console.error('Failed to fetch session extensions:', error);
              }
            }),
          {
            shouldFinish: () => latestSessionIdRef.current === sessionId,
          }
        );
      } catch {
        resetTransition();
      }
    },
    [beginToggle, loadSessionExtensions, resetTransition, scheduleSort, sessionId]
  );

  const extensions = useMemo(
    () => mergeSessionExtensions(allExtensions, sessionExtensions),
    [allExtensions, sessionExtensions]
  );

  return (
    <ExtensionMenu
      extensions={extensions ?? []}
      title={intl.formatMessage(i18n.manageExtensions)}
      searchPlaceholder={intl.formatMessage(i18n.searchExtensions)}
      description={intl.formatMessage(i18n.extensionsForThisSession)}
      emptyMessage={intl.formatMessage(i18n.noExtensionsAvailable)}
      noResultsMessage={intl.formatMessage(i18n.noExtensionsFound)}
      hidden={extensions === null || extensions.length === 0 || !isSessionExtensionsLoaded}
      isTransitioning={isTransitioning}
      isSortPending={isSortPending}
      togglingExtensionName={togglingExtensionName}
      onToggle={handleToggle}
    />
  );
}

import { useEffect, useState, useRef, type RefObject } from 'react';
import { IpcRendererEvent } from 'electron';
import { HashRouter, Routes, Route, useNavigate, useLocation, useSearchParams } from 'react-router';
import { importNostrSessionFromDeepLink } from './sessionLinks';
import { ErrorUI } from './components/ErrorBoundary';
import { ExtensionInstallModal } from './components/ExtensionInstallModal';
import RecipeParamsModalContainer from './components/RecipeParamsModalContainer';
import { isRecipeParamsCancelled, isRecipeParameterScopesUnsupported } from './acp/errors';
import { toast, ToastContainer } from 'react-toastify';
import AnnouncementModal from './components/AnnouncementModal';
import TelemetryConsentPrompt from './components/TelemetryConsentPrompt';
import OnboardingGuard from './components/onboarding/OnboardingGuard';
import { createSession } from './sessions';
import { acpListSessions, acpDeleteSession } from './acp/sessions';
import { acpChatSessionActions } from './acp/chatSessionStore';

import { ChatType } from './types/chat';
import Hub from './components/Hub';
import { UserInput } from './types/message';

interface PairRouteState {
  resumeSessionId?: string;
  initialMessage?: UserInput;
  noAutoSubmit?: boolean;
}
import SettingsView, { SettingsViewOptions } from './components/settings/SettingsView';
import SessionsView from './components/sessions/SessionsView';
import SchedulesView from './components/schedule/SchedulesView';
import ProviderSettings from './components/settings/providers/ProviderSettingsPage';
import { AppLayout } from './components/Layout/AppLayout';
import { ChatProvider, DEFAULT_CHAT_TITLE } from './contexts/ChatContext';
import LauncherView from './components/LauncherView';

import 'react-toastify/dist/ReactToastify.css';
import { useConfig } from './components/ConfigContext';
import { ModelAndProviderProvider } from './components/ModelAndProviderContext';
import { ThemeProvider } from './contexts/ThemeContext';
import { FeaturesProvider } from './contexts/FeaturesContext';
import PermissionSettingsView from './components/settings/permission/PermissionSetting';

import ExtensionsView, { ExtensionsViewOptions } from './components/extensions/ExtensionsView';
import RecipesView from './components/recipes/RecipesView';
import SkillsView from './components/skills/SkillsView';
import AppsView from './components/apps/AppsView';
import StandaloneAppView from './components/apps/StandaloneAppView';
import { View, ViewOptions } from './utils/navigationUtils';

import { useNavigation } from './hooks/useNavigation';
import { errorMessage } from './utils/conversionUtils';
import { getInitialWorkingDir, refreshWorkingDir } from './utils/workingDir';
import { usePageViewTracking } from './hooks/useAnalytics';
import { trackErrorWithContext } from './utils/analytics';
import { AppEvents } from './constants/events';
import { registerPlatformEventHandlers } from './utils/platform_events';
import { reconnectAcpAfterSystemResume } from './acp/acpConnection';

function PageViewTracker() {
  usePageViewTracking();
  return null;
}

// Route Components
const HubRouteWrapper = ({ draftRef }: { draftRef: RefObject<string> }) => {
  const setView = useNavigation();
  return <Hub setView={setView} draftRef={draftRef} />;
};

export function resolveSessionInitialMessage(
  session: { recipe?: { prompt?: string | null } | null },
  initialMessage?: UserInput
): UserInput | undefined {
  return (
    initialMessage ??
    (session.recipe?.prompt ? { msg: session.recipe.prompt, images: [] } : undefined)
  );
}

export const PairRouteWrapper = ({
  activeSessions,
}: {
  activeSessions: Array<{
    sessionId: string;
    initialMessage?: UserInput;
    noAutoSubmit?: boolean;
  }>;
  setActiveSessions: (
    sessions: Array<{ sessionId: string; initialMessage?: UserInput; noAutoSubmit?: boolean }>
  ) => void;
}) => {
  const { extensionsList } = useConfig();
  const location = useLocation();
  const routeState =
    (location.state as PairRouteState) || (window.history.state as PairRouteState) || {};
  const [searchParams, setSearchParams] = useSearchParams();
  const isCreatingSessionRef = useRef(false);
  const navigate = useNavigate();

  const resumeSessionId = searchParams.get('resumeSessionId') ?? undefined;
  const recipeDeeplinkFromConfig = window.appConfig?.get('recipeDeeplink') as string | undefined;
  const recipeIdFromConfig = window.appConfig?.get('recipeId') as string | undefined;
  const initialMessage = routeState.initialMessage;
  const noAutoSubmit = routeState.noAutoSubmit;

  // Create session if we have an initialMessage, recipeDeeplink, or recipeId but no sessionId
  useEffect(() => {
    if (
      (initialMessage || recipeDeeplinkFromConfig || recipeIdFromConfig) &&
      !resumeSessionId &&
      !isCreatingSessionRef.current
    ) {
      isCreatingSessionRef.current = true;

      (async () => {
        try {
          const newSession = await createSession(await refreshWorkingDir(), {
            recipeDeeplink: recipeDeeplinkFromConfig,
            recipeId: recipeIdFromConfig,
            allExtensions: extensionsList,
          });
          const sessionInitialMessage = resolveSessionInitialMessage(newSession, initialMessage);

          window.dispatchEvent(
            new CustomEvent(AppEvents.ADD_ACTIVE_SESSION, {
              detail: {
                sessionId: newSession.id,
                initialMessage: sessionInitialMessage,
                noAutoSubmit,
              },
            })
          );

          setSearchParams((prev) => {
            prev.set('resumeSessionId', newSession.id);
            return prev;
          });
        } catch (error) {
          if (isRecipeParamsCancelled(error)) {
            navigate('/');
            return;
          }
          if (isRecipeParameterScopesUnsupported(error)) {
            toast.error(error.message);
            navigate('/');
            return;
          }
          console.error('Failed to create session:', error);
          trackErrorWithContext(error, {
            component: 'PairRouteWrapper',
            action: 'create_session',
            recoverable: true,
          });
        } finally {
          isCreatingSessionRef.current = false;
        }
      })();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    initialMessage,
    recipeDeeplinkFromConfig,
    recipeIdFromConfig,
    resumeSessionId,
    setSearchParams,
    extensionsList,
  ]);

  // Add resumed session to active sessions if not already there
  useEffect(() => {
    if (resumeSessionId && !activeSessions.some((s) => s.sessionId === resumeSessionId)) {
      window.dispatchEvent(
        new CustomEvent(AppEvents.ADD_ACTIVE_SESSION, {
          detail: {
            sessionId: resumeSessionId,
            initialMessage: initialMessage,
            noAutoSubmit,
          },
        })
      );
    }
  }, [resumeSessionId, activeSessions, initialMessage, noAutoSubmit]);

  return null;
};

const SettingsRoute = () => {
  const location = useLocation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const setView = useNavigation();

  // Get viewOptions from location.state, history.state, or URL search params
  const viewOptions =
    (location.state as SettingsViewOptions) || (window.history.state as SettingsViewOptions) || {};

  // If section is provided via URL search params, add it to viewOptions
  const sectionFromUrl = searchParams.get('section');
  if (sectionFromUrl) {
    viewOptions.section = sectionFromUrl;
  }

  return <SettingsView onClose={() => navigate('/')} setView={setView} viewOptions={viewOptions} />;
};

const SessionsRoute = () => {
  return <SessionsView />;
};

const SchedulesRoute = () => {
  const navigate = useNavigate();
  return <SchedulesView onClose={() => navigate('/')} />;
};

const RecipesRoute = () => {
  return <RecipesView />;
};

const SkillsRoute = () => {
  return <SkillsView />;
};

const PermissionRoute = () => {
  const location = useLocation();
  const navigate = useNavigate();
  const parentView = location.state?.parentView as View;
  const parentViewOptions = location.state?.parentViewOptions as ViewOptions;

  return (
    <PermissionSettingsView
      onClose={() => {
        // Navigate back to parent view with options
        switch (parentView) {
          case 'chat':
            navigate('/');
            break;
          case 'pair':
            navigate('/pair');
            break;
          case 'settings':
            navigate('/settings', { state: parentViewOptions });
            break;
          case 'sessions':
            navigate('/sessions');
            break;
          case 'schedules':
            navigate('/schedules');
            break;
          case 'recipes':
            navigate('/recipes');
            break;
          case 'skills':
            navigate('/skills');
            break;
          default:
            navigate('/');
        }
      }}
    />
  );
};

const ConfigureProvidersRoute = () => {
  const navigate = useNavigate();

  return (
    <div className="w-screen h-screen bg-background-primary">
      <ProviderSettings
        onClose={() => navigate('/settings', { state: { section: 'models' } })}
        isOnboarding={false}
      />
    </div>
  );
};

const ExtensionsRoute = () => {
  const navigate = useNavigate();
  const location = useLocation();

  // Get viewOptions from location.state or history.state (for deep link extensions)
  const viewOptions =
    (location.state as ExtensionsViewOptions) ||
    (window.history.state as ExtensionsViewOptions) ||
    {};

  return (
    <ExtensionsView
      onClose={() => navigate(-1)}
      setView={(view, options) => {
        switch (view) {
          case 'chat':
            navigate('/');
            break;
          case 'pair':
            navigate('/pair', { state: options });
            break;
          case 'settings':
            navigate('/settings', { state: options });
            break;
          default:
            navigate('/');
        }
      }}
      viewOptions={viewOptions}
    />
  );
};

export function AppInner() {
  const [fatalError, setFatalError] = useState<string | null>(null);

  const nostrImportInFlight = useRef<string | null>(null);

  const navigate = useNavigate();
  const setView = useNavigation();

  const [chat, setChat] = useState<ChatType>({
    sessionId: '',
    name: DEFAULT_CHAT_TITLE,
    messages: [],
    recipe: null,
  });

  // New Chat is the only chat that unmounts on navigation; the rest stay mounted in
  // `ChatSessionsContainer` and keep their text in local state. Its unsent input lives
  // here so it outlives that unmount, and in a ref rather than state because nothing
  // above the outlet has to render on a keystroke.
  const hubDraftRef = useRef('');

  const MAX_ACTIVE_SESSIONS = 10;

  const [activeSessions, setActiveSessions] = useState<
    Array<{ sessionId: string; initialMessage?: UserInput; noAutoSubmit?: boolean }>
  >([]);

  useEffect(() => {
    const handleAddActiveSession = (event: Event) => {
      const { sessionId, initialMessage, noAutoSubmit } = (
        event as CustomEvent<{
          sessionId: string;
          initialMessage?: UserInput;
          noAutoSubmit?: boolean;
        }>
      ).detail;

      setActiveSessions((prev) => {
        const existingIndex = prev.findIndex((s) => s.sessionId === sessionId);

        if (existingIndex !== -1) {
          // Session exists - move to end of LRU list (most recently used)
          const existing = prev[existingIndex];
          return [...prev.slice(0, existingIndex), ...prev.slice(existingIndex + 1), existing];
        }

        // New session - add to end with LRU eviction if needed
        const newSession = { sessionId, initialMessage, noAutoSubmit };
        const updated = [...prev, newSession];
        if (updated.length > MAX_ACTIVE_SESSIONS) {
          return updated.slice(updated.length - MAX_ACTIVE_SESSIONS);
        }
        return updated;
      });
    };

    const handleClearInitialMessage = (event: Event) => {
      const { sessionId } = (event as CustomEvent<{ sessionId: string }>).detail;

      setActiveSessions((prev) => {
        return prev.map((session) => {
          if (session.sessionId === sessionId) {
            return { ...session, initialMessage: undefined };
          }
          return session;
        });
      });
    };

    const handleSessionDeleted = (event: Event) => {
      const { sessionId } = (event as CustomEvent<{ sessionId: string }>).detail;

      setActiveSessions((prev) => {
        return prev.filter((session) => session.sessionId !== sessionId);
      });
    };

    const handleBackendSwitched = () => {
      setActiveSessions((prev) => {
        for (const session of prev) {
          acpChatSessionActions.deleteSnapshot(session.sessionId);
        }
        return [];
      });
    };

    window.addEventListener(AppEvents.ADD_ACTIVE_SESSION, handleAddActiveSession);
    window.addEventListener(AppEvents.CLEAR_INITIAL_MESSAGE, handleClearInitialMessage);
    window.addEventListener(AppEvents.SESSION_DELETED, handleSessionDeleted);
    window.addEventListener(AppEvents.BACKEND_SWITCHED, handleBackendSwitched);
    return () => {
      window.removeEventListener(AppEvents.ADD_ACTIVE_SESSION, handleAddActiveSession);
      window.removeEventListener(AppEvents.CLEAR_INITIAL_MESSAGE, handleClearInitialMessage);
      window.removeEventListener(AppEvents.SESSION_DELETED, handleSessionDeleted);
      window.removeEventListener(AppEvents.BACKEND_SWITCHED, handleBackendSwitched);
    };
  }, []);

  const { addExtension } = useConfig();

  useEffect(() => {
    try {
      window.electron.reactReady();
    } catch (error) {
      console.error('Error sending reactReady:', error);
      setFatalError(`React ready notification failed: ${errorMessage(error, 'Unknown error')}`);
    }
  }, []);

  useEffect(() => {
    const handleSystemResume = () => reconnectAcpAfterSystemResume();
    window.electron.on('system-resume', handleSystemResume);
    return () => window.electron.off('system-resume', handleSystemResume);
  }, []);

  useEffect(() => {
    acpListSessions()
      .then(({ sessions }) => {
        const phantom = sessions.filter(
          (s) => s.messageCount === 0 && !s.userSetName && !s.hasRecipe
        );
        for (const s of phantom) {
          acpDeleteSession(s.id).catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    const handleOpenSharedSession = async (_event: IpcRendererEvent, ...args: unknown[]) => {
      const link = args[0] as string;
      window.electron.logInfo('Opening session share link');

      if (!link.startsWith('goose://sessions/nostr')) {
        toast.error('Unsupported session share link');
        navigate('/sessions');
        return;
      }

      if (nostrImportInFlight.current === link) {
        window.electron.logInfo('Skipping duplicate Nostr deep link import');
        return;
      }
      nostrImportInFlight.current = link;

      try {
        await importNostrSessionFromDeepLink(link);
        navigate('/sessions');
      } catch (error) {
        console.error('Unexpected error opening Nostr session share:', error);
        trackErrorWithContext(error, {
          component: 'AppInner',
          action: 'open_nostr_session_share',
          recoverable: true,
        });
        toast.error(`Failed to import Nostr session: ${errorMessage(error, 'Unknown error')}`);
        navigate('/sessions');
      } finally {
        if (nostrImportInFlight.current === link) {
          nostrImportInFlight.current = null;
        }
      }
    };
    window.electron.on('open-shared-session', handleOpenSharedSession);
    return () => {
      window.electron.off('open-shared-session', handleOpenSharedSession);
    };
  }, [navigate]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const isMac = window.electron.platform === 'darwin';
      if ((isMac ? event.metaKey : event.ctrlKey) && event.key === 'n') {
        event.preventDefault();
        try {
          window.electron.createChatWindow({ dir: getInitialWorkingDir() });
        } catch (error) {
          console.error('Error creating new window:', error);
        }
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, []);

  // Prevent default drag and drop behavior globally to avoid opening files in new windows
  // but allow our React components to handle drops in designated areas
  useEffect(() => {
    const preventDefaults = (e: globalThis.DragEvent) => {
      // Only prevent default if we're not over a designated drop zone
      const target = e.target as HTMLElement;
      const isOverDropZone = target.closest('[data-drop-zone="true"]') !== null;

      if (!isOverDropZone) {
        e.preventDefault();
        e.stopPropagation();
      }
    };

    const handleDragOver = (e: globalThis.DragEvent) => {
      // Always prevent default for dragover to allow dropping
      e.preventDefault();
      e.stopPropagation();
    };

    const handleDrop = (e: globalThis.DragEvent) => {
      // Only prevent default if we're not over a designated drop zone
      const target = e.target as HTMLElement;
      const isOverDropZone = target.closest('[data-drop-zone="true"]') !== null;

      if (!isOverDropZone) {
        e.preventDefault();
        e.stopPropagation();
      }
    };

    // Add event listeners to document to catch drag events
    document.addEventListener('dragenter', preventDefaults, false);
    document.addEventListener('dragleave', preventDefaults, false);
    document.addEventListener('dragover', handleDragOver, false);
    document.addEventListener('drop', handleDrop, false);

    return () => {
      document.removeEventListener('dragenter', preventDefaults, false);
      document.removeEventListener('dragleave', preventDefaults, false);
      document.removeEventListener('dragover', handleDragOver, false);
      document.removeEventListener('drop', handleDrop, false);
    };
  }, []);

  useEffect(() => {
    const handleFatalError = (_event: IpcRendererEvent, ...args: unknown[]) => {
      const errorMessage = args[0] as string;
      console.error('Encountered a fatal error:', errorMessage);
      setFatalError(errorMessage);
    };
    window.electron.on('fatal-error', handleFatalError);
    return () => {
      window.electron.off('fatal-error', handleFatalError);
    };
  }, []);

  useEffect(() => {
    const handleSetView = (_event: IpcRendererEvent, ...args: unknown[]) => {
      const newView = args[0] as View;
      const section = args[1] as string | undefined;

      if (section && newView === 'settings') {
        navigate(`/settings?section=${section}`);
      } else {
        navigate(`/${newView}`);
      }
    };

    window.electron.on('set-view', handleSetView);
    return () => window.electron.off('set-view', handleSetView);
  }, [navigate]);

  useEffect(() => {
    const handleNewChat = (_event: IpcRendererEvent, ..._args: unknown[]) => {
      navigate('/');
    };

    window.electron.on('new-chat', handleNewChat);
    return () => window.electron.off('new-chat', handleNewChat);
  }, [navigate]);

  useEffect(() => {
    const handleFocusInput = (_event: IpcRendererEvent, ..._args: unknown[]) => {
      const inputField = document.querySelector('input[type="text"], textarea') as HTMLInputElement;
      if (inputField) {
        inputField.focus();
      }
    };
    window.electron.on('focus-input', handleFocusInput);
    return () => {
      window.electron.off('focus-input', handleFocusInput);
    };
  }, []);

  // Handle initial message from launcher
  const isProcessingRef = useRef(false);

  useEffect(() => {
    const handleSetInitialMessage = async (_event: IpcRendererEvent, ...args: unknown[]) => {
      const initialMessage = args[0] as string;
      const options = (args[1] as { noAutoSubmit?: boolean } | undefined) || {};

      if (initialMessage && !isProcessingRef.current) {
        isProcessingRef.current = true;
        navigate('/pair', {
          state: {
            initialMessage: { msg: initialMessage, images: [] },
            noAutoSubmit: options.noAutoSubmit,
          },
        });
        setTimeout(() => {
          isProcessingRef.current = false;
        }, 1000);
      }
    };
    window.electron.on('set-initial-message', handleSetInitialMessage);
    return () => {
      window.electron.off('set-initial-message', handleSetInitialMessage);
    };
  }, [navigate]);

  // Register platform event handlers for app lifecycle management
  useEffect(() => {
    return registerPlatformEventHandlers();
  }, []);

  if (fatalError) {
    return <ErrorUI error={errorMessage(fatalError)} />;
  }

  return (
    <>
      <PageViewTracker />
      <ToastContainer
        aria-label="Toast notifications"
        toastClassName={() =>
          `relative min-h-16 mb-4 p-2 rounded-lg
               flex justify-between overflow-hidden cursor-pointer
               text-text-inverse bg-background-inverse
              `
        }
        style={{ width: '450px' }}
        className="mt-6"
        position="top-right"
        autoClose={3000}
        closeOnClick
        pauseOnHover
      />
      <ExtensionInstallModal addExtension={addExtension} setView={setView} />
      <RecipeParamsModalContainer />
      <div className="relative w-screen h-screen overflow-hidden bg-background-secondary flex flex-col">
        <div className="titlebar-drag-region" />
        <div style={{ position: 'relative', width: '100%', height: '100%' }}>
          <Routes>
            <Route path="launcher" element={<LauncherView />} />
            <Route path="configure-providers" element={<ConfigureProvidersRoute />} />
            <Route path="standalone-app" element={<StandaloneAppView />} />
            <Route
              path="/"
              element={
                <OnboardingGuard>
                  <ChatProvider chat={chat} setChat={setChat} contextKey="hub">
                    <AppLayout activeSessions={activeSessions} />
                  </ChatProvider>
                </OnboardingGuard>
              }
            >
              <Route index element={<HubRouteWrapper draftRef={hubDraftRef} />} />
              <Route
                path="pair"
                element={
                  <PairRouteWrapper
                    activeSessions={activeSessions}
                    setActiveSessions={setActiveSessions}
                  />
                }
              />
              <Route path="settings" element={<SettingsRoute />} />
              <Route
                path="extensions"
                element={
                  <ChatProvider chat={chat} setChat={setChat} contextKey="extensions">
                    <ExtensionsRoute />
                  </ChatProvider>
                }
              />
              <Route path="apps" element={<AppsView />} />
              <Route path="sessions" element={<SessionsRoute />} />
              <Route path="schedules" element={<SchedulesRoute />} />
              <Route path="recipes" element={<RecipesRoute />} />
              <Route path="skills" element={<SkillsRoute />} />
              <Route path="permission" element={<PermissionRoute />} />
            </Route>
          </Routes>
        </div>
      </div>
    </>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <FeaturesProvider>
        <ModelAndProviderProvider>
          <HashRouter>
            <AppInner />
          </HashRouter>
          <AnnouncementModal />
          <TelemetryConsentPrompt />
        </ModelAndProviderProvider>
      </FeaturesProvider>
    </ThemeProvider>
  );
}

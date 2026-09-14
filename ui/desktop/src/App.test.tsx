/* eslint-disable @typescript-eslint/no-explicit-any */

/**
 * @vitest-environment jsdom
 */
import React from 'react';
import { screen, render, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { AppInner, PairRouteWrapper, resolveSessionInitialMessage } from './App';
import { IntlTestWrapper } from './i18n/test-utils';
import { FeaturesProvider } from './contexts/FeaturesContext';
import { reconnectAcpAfterSystemResume } from './acp/acpConnection';
import { createSession } from './sessions';
import { getEffectiveWorkingDir } from './utils/workingDir';
import { RecipeParameterScopesUnsupportedError } from './acp/errors';
import { AppEvents } from './constants/events';

const mockToastError = vi.hoisted(() => vi.fn());

// Set up globals for jsdom
Object.defineProperty(window, 'location', {
  value: {
    hash: '',
    search: '',
    href: 'http://localhost:3000',
    origin: 'http://localhost:3000',
    pathname: '/',
  },
  writable: true,
});

Object.defineProperty(window, 'history', {
  value: {
    replaceState: vi.fn(),
    state: null,
  },
  writable: true,
});

vi.mock('./utils/costDatabase', () => ({
  initializeCostDatabase: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./acp/sessions', () => ({
  acpListSessions: vi.fn().mockResolvedValue({ sessions: [], nextCursor: null }),
  acpDeleteSession: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./sessions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./sessions')>()),
  fetchSessionDetails: vi
    .fn()
    .mockResolvedValue({ sessionId: 'test', messages: [], metadata: { description: '' } }),
  generateSessionId: vi.fn(),
  createSession: vi.fn(),
}));

vi.mock('./utils/workingDir', () => ({
  getInitialWorkingDir: () => '/test/dir',
  getEffectiveWorkingDir: vi.fn().mockResolvedValue('/test/dir'),
}));

vi.mock('./acp/capabilities', () => ({
  getAcpFeatureCapabilities: vi.fn().mockResolvedValue({ localInference: true }),
}));

vi.mock('./acp/acpConnection', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./acp/acpConnection')>()),
  reconnectAcpAfterSystemResume: vi.fn(),
}));

// Mock the ACP providers module used by OnboardingGuard so it doesn't try to
// open a real ACP client connection during tests. Returning null defaults
// keeps the app in the "brand new" (no provider configured) onboarding state.
vi.mock('./acp/providers', () => ({
  acpReadDefaults: vi.fn().mockResolvedValue({ providerId: null, modelId: null }),
  acpSaveDefaults: vi.fn().mockResolvedValue(undefined),
  acpListProviderDetails: vi.fn().mockResolvedValue([]),
}));

// Mock the ConfigContext module
vi.mock('./components/ConfigContext', () => ({
  useConfig: () => ({
    read: vi.fn().mockResolvedValue(null),
    update: vi.fn(),
    getExtensions: vi.fn().mockReturnValue([]),
    addExtension: vi.fn(),
    updateExtension: vi.fn(),
    createProviderDefaults: vi.fn(),
    extensionsList: [],
  }),
  ConfigProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

// Mock other components to simplify testing
vi.mock('./components/ErrorBoundary', () => ({
  ErrorUI: ({ error }: { error: Error }) => <div>Error: {error.message}</div>,
}));

vi.mock('./components/ModelAndProviderContext', () => ({
  ModelAndProviderProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  useModelAndProvider: () => ({
    provider: null,
    model: null,
    getCurrentModelAndProvider: vi.fn(),
    getFallbackModelAndProvider: vi.fn().mockResolvedValue({ provider: '', model: '' }),
    refreshCurrentModelAndProvider: vi.fn().mockResolvedValue(undefined),
    setCurrentModelAndProvider: vi.fn(),
  }),
}));

vi.mock('./contexts/ChatContext', () => ({
  ChatProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  useChatContext: () => ({
    chat: {
      id: 'test-id',
      name: 'Test Chat',
      messages: [],
      recipe: null,
    },
    setChat: vi.fn(),
    setPairChat: vi.fn(), // Keep this from HEAD
    resetChat: vi.fn(),
    hasActiveSession: false,
    setRecipe: vi.fn(),
    clearRecipe: vi.fn(),
    contextKey: 'hub',
  }),
  DEFAULT_CHAT_TITLE: 'New Chat', // Keep this from HEAD
}));

vi.mock('./components/ui/ConfirmationModal', () => ({
  ConfirmationModal: () => null,
}));

vi.mock('react-toastify', () => ({
  ToastContainer: () => null,
  toast: {
    error: mockToastError,
  },
}));

vi.mock('./components/GoosehintsModal', () => ({
  GoosehintsModal: () => null,
}));

vi.mock('./components/AnnouncementModal', () => ({
  default: () => null,
}));

// Create mocks that we can track and configure per test
const mockNavigate = vi.fn();
const mockSearchParams = new URLSearchParams();
const mockSetSearchParams = vi.fn();
const mockLocation = { state: null as Record<string, unknown> | null, pathname: '/' };

// Mock react-router to avoid HashRouter issues in tests
vi.mock('react-router', () => ({
  HashRouter: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  Routes: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  Route: ({ element }: { element: React.ReactNode }) => element,
  useNavigate: () => mockNavigate,
  useLocation: () => mockLocation,
  useSearchParams: () => [mockSearchParams, mockSetSearchParams],
  Outlet: () => null,
}));

// Mock electron API
const mockElectron = {
  getConfig: vi.fn().mockReturnValue({
    GOOSE_ALLOWLIST_WARNING: false,
    GOOSE_WORKING_DIR: '/test/dir',
  }),
  logInfo: vi.fn(),
  on: vi.fn(),
  off: vi.fn(),
  reactReady: vi.fn(),
  getAllowedExtensions: vi.fn().mockResolvedValue([]),
  platform: 'darwin',
  createChatWindow: vi.fn(),
  getSetting: vi.fn().mockResolvedValue(null),
  setSetting: vi.fn().mockResolvedValue(undefined),
};

// Mock appConfig
const mockAppConfig = {
  get: vi.fn((key: string): string | null => {
    if (key === 'GOOSE_WORKING_DIR') return '/test/dir';
    return null;
  }),
};

// Attach mocks to window
(window as any).electron = mockElectron;
(window as any).appConfig = mockAppConfig;

// Mock matchMedia
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation((query) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(), // deprecated
    removeListener: vi.fn(), // deprecated
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

function AppInnerTestWrapper({ children }: { children: React.ReactNode }) {
  return (
    <IntlTestWrapper>
      <FeaturesProvider>{children}</FeaturesProvider>
    </IntlTestWrapper>
  );
}

describe('App Component - Brand New State', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockNavigate.mockClear();
    mockSetSearchParams.mockClear();
    mockLocation.state = null;
    mockLocation.pathname = '/';
    mockAppConfig.get.mockImplementation((key: string): string | null => {
      if (key === 'GOOSE_WORKING_DIR') return '/test/dir';
      return null;
    });
    vi.mocked(getEffectiveWorkingDir).mockResolvedValue('/test/dir');

    // Reset search params
    mockSearchParams.forEach((_, key) => {
      mockSearchParams.delete(key);
    });

    window.location.hash = '';
    window.location.search = '';
    window.location.pathname = '/';
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should redirect to "/" when app is brand new (no provider configured)', async () => {
    // Mock no provider configured
    mockElectron.getConfig.mockReturnValue({
      GOOSE_DEFAULT_PROVIDER: null,
      GOOSE_DEFAULT_MODEL: null,
      GOOSE_ALLOWLIST_WARNING: false,
    });

    render(<AppInner />, { wrapper: AppInnerTestWrapper });

    // Wait for initialization
    await waitFor(() => {
      expect(mockElectron.reactReady).toHaveBeenCalled();
    });

    // The app should initialize without any navigation calls since we're already at "/"
    // No navigate calls should be made when no provider is configured
    expect(mockNavigate).not.toHaveBeenCalled();
  });

  it('should handle deep links correctly when app is brand new', async () => {
    // Mock no provider configured
    mockElectron.getConfig.mockReturnValue({
      GOOSE_DEFAULT_PROVIDER: null,
      GOOSE_DEFAULT_MODEL: null,
      GOOSE_ALLOWLIST_WARNING: false,
    });

    // Set up search params to simulate view=settings deep link
    mockSearchParams.set('view', 'settings');

    render(<AppInner />, { wrapper: AppInnerTestWrapper });

    // Wait for initialization
    await waitFor(() => {
      expect(mockElectron.reactReady).toHaveBeenCalled();
    });

    expect(screen.getByText(/^Welcome to goose/)).toBeInTheDocument();
  });

  it('should not redirect when provider is configured', async () => {
    // Mock provider configured
    mockElectron.getConfig.mockReturnValue({
      GOOSE_DEFAULT_PROVIDER: 'openai',
      GOOSE_DEFAULT_MODEL: 'gpt-4',
      GOOSE_ALLOWLIST_WARNING: false,
    });

    render(<AppInner />, { wrapper: AppInnerTestWrapper });

    // Wait for initialization
    await waitFor(() => {
      expect(mockElectron.reactReady).toHaveBeenCalled();
    });

    // Should not navigate anywhere since provider is configured and we're already at "/"
    expect(mockNavigate).not.toHaveBeenCalled();
  });

  it('shows the scoped-parameter incompatibility before returning home', async () => {
    mockAppConfig.get.mockImplementation((key: string): string | null => {
      if (key === 'GOOSE_WORKING_DIR') return '/test/dir';
      if (key === 'recipeDeeplink') return 'goose://recipe?url=example';
      return null;
    });
    vi.mocked(createSession).mockRejectedValueOnce(new RecipeParameterScopesUnsupportedError());

    render(<PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} />, {
      wrapper: AppInnerTestWrapper,
    });

    await waitFor(() => {
      expect(mockToastError).toHaveBeenCalledWith(
        'The connected Goose server does not support securely scoped deeplink recipe parameters. Update the server and try again.'
      );
    });
    expect(mockNavigate).toHaveBeenCalledWith('/');
  });

  it('shows a pending chat while createSession is unresolved', async () => {
    let resolveSession: ((value: Awaited<ReturnType<typeof createSession>>) => void) | undefined;
    vi.mocked(createSession).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSession = resolve;
        })
    );
    mockLocation.state = {
      initialMessage: { msg: 'hello from hub', images: [] },
      workingDir: '/tmp/hub-dir',
    };
    mockLocation.pathname = '/pair';

    render(<PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} />, {
      wrapper: AppInnerTestWrapper,
    });

    expect(screen.getByTestId('pending-chat')).toBeInTheDocument();
    expect(screen.getByText('hello from hub')).toBeInTheDocument();
    expect(createSession).toHaveBeenCalledWith('/tmp/hub-dir', {
      recipeDeeplink: undefined,
      recipeId: undefined,
      allExtensions: [],
    });
    expect(mockSetSearchParams).not.toHaveBeenCalled();

    resolveSession?.({
      id: 'session-pending',
      name: 'untitled',
      message_count: 0,
      created_at: '2026-08-21T00:00:00.000Z',
      updated_at: '2026-08-21T00:00:00.000Z',
      working_dir: '/tmp/hub-dir',
      extension_data: { active: [], installed: [] },
    });

    await waitFor(() => {
      expect(mockSetSearchParams).toHaveBeenCalled();
    });
  });

  it('applies createSession after StrictMode remounts PairRouteWrapper', async () => {
    let resolveSession: ((value: Awaited<ReturnType<typeof createSession>>) => void) | undefined;
    vi.mocked(createSession).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveSession = resolve;
        })
    );
    mockLocation.state = {
      initialMessage: { msg: 'hello from hub', images: [] },
      workingDir: '/tmp/hub-dir',
    };
    mockLocation.pathname = '/pair';

    render(
      <React.StrictMode>
        <PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} />
      </React.StrictMode>,
      { wrapper: AppInnerTestWrapper }
    );

    expect(screen.getByTestId('pending-chat')).toBeInTheDocument();

    resolveSession?.({
      id: 'session-strict-mode',
      name: 'untitled',
      message_count: 0,
      created_at: '2026-08-21T00:00:00.000Z',
      updated_at: '2026-08-21T00:00:00.000Z',
      working_dir: '/tmp/hub-dir',
      extension_data: { active: [], installed: [] },
    });

    await waitFor(() => {
      expect(mockSetSearchParams).toHaveBeenCalled();
    });
  });

  it('creates the session with Hub-selected extensions and working dir', async () => {
    vi.mocked(createSession).mockResolvedValueOnce({
      id: 'session-hub',
      recipe: null,
    } as Awaited<ReturnType<typeof createSession>>);
    mockLocation.state = {
      initialMessage: { msg: 'use developer only', images: [] },
      workingDir: '/tmp/project',
      extensionConfigs: [{ name: 'developer', type: 'builtin', description: 'developer' }],
    };
    mockLocation.pathname = '/pair';

    render(<PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} />, {
      wrapper: AppInnerTestWrapper,
    });

    await waitFor(() => {
      expect(createSession).toHaveBeenCalledWith('/tmp/project', {
        recipeDeeplink: undefined,
        recipeId: undefined,
        extensionConfigs: [{ name: 'developer', type: 'builtin', description: 'developer' }],
      });
    });
    expect(getEffectiveWorkingDir).not.toHaveBeenCalled();
  });

  it('resolves the effective working directory when Hub did not pass one', async () => {
    vi.mocked(getEffectiveWorkingDir).mockResolvedValueOnce('/tmp/effective-remote');
    vi.mocked(createSession).mockResolvedValueOnce({
      id: 'session-effective-dir',
      recipe: null,
    } as Awaited<ReturnType<typeof createSession>>);
    mockLocation.state = {
      initialMessage: { msg: 'hello from hub', images: [] },
    };
    mockLocation.pathname = '/pair';

    render(<PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} />, {
      wrapper: AppInnerTestWrapper,
    });

    await waitFor(() => {
      expect(createSession).toHaveBeenCalledWith('/tmp/effective-remote', {
        recipeDeeplink: undefined,
        recipeId: undefined,
        allExtensions: [],
      });
    });
  });

  it('restores the Hub draft when createSession fails', async () => {
    vi.mocked(createSession).mockRejectedValueOnce(new Error('backend down'));
    const images = [{ data: 'abc123', mimeType: 'image/png' }];
    const draftRef = { current: { msg: '', images: [] } };
    mockLocation.state = {
      initialMessage: { msg: 'retry me', images },
      workingDir: '/tmp/hub-dir',
    };
    mockLocation.pathname = '/pair';

    render(
      <PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} draftRef={draftRef} />,
      { wrapper: AppInnerTestWrapper }
    );

    await waitFor(() => {
      expect(mockNavigate).toHaveBeenCalledWith('/');
    });
    expect(draftRef.current).toEqual({ msg: 'retry me', images });
  });

  it('restores attached images when an image-only Hub submission fails', async () => {
    vi.mocked(createSession).mockRejectedValueOnce(new Error('backend down'));
    const images = [{ data: 'imgonly', mimeType: 'image/jpeg' }];
    const draftRef = { current: { msg: '', images: [] } };
    mockLocation.state = {
      initialMessage: { msg: '', images },
      workingDir: '/tmp/hub-dir',
    };
    mockLocation.pathname = '/pair';

    render(
      <PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} draftRef={draftRef} />,
      { wrapper: AppInnerTestWrapper }
    );

    await waitFor(() => {
      expect(mockNavigate).toHaveBeenCalledWith('/');
    });
    expect(draftRef.current).toEqual({ msg: '', images });
  });

  it('clears the Hub draft after createSession succeeds', async () => {
    vi.mocked(createSession).mockResolvedValueOnce({
      id: 'session-success',
      recipe: null,
    } as Awaited<ReturnType<typeof createSession>>);
    const draftRef = { current: { msg: 'hello from hub', images: [] } };
    mockLocation.state = {
      initialMessage: { msg: 'hello from hub', images: [] },
      workingDir: '/tmp/hub-dir',
    };
    mockLocation.pathname = '/pair';

    render(
      <PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} draftRef={draftRef} />,
      { wrapper: AppInnerTestWrapper }
    );

    await waitFor(() => {
      expect(createSession).toHaveBeenCalled();
    });
    expect(draftRef.current).toEqual({ msg: '', images: [] });
  });

  it('publishes a session that finishes after PairRouteWrapper unmounts', async () => {
    let resolveSession: ((value: Awaited<ReturnType<typeof createSession>>) => void) | undefined;
    vi.mocked(createSession).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSession = resolve;
        })
    );
    mockLocation.state = {
      initialMessage: { msg: 'hello from hub', images: [] },
      workingDir: '/tmp/hub-dir',
    };
    mockLocation.pathname = '/pair';

    const onSessionCreated = vi.fn();
    const onAddActiveSession = vi.fn();
    window.addEventListener(AppEvents.SESSION_CREATED, onSessionCreated);
    window.addEventListener(AppEvents.ADD_ACTIVE_SESSION, onAddActiveSession);

    try {
      const { unmount } = render(
        <PairRouteWrapper activeSessions={[]} setActiveSessions={vi.fn()} />,
        { wrapper: AppInnerTestWrapper }
      );

      expect(createSession).toHaveBeenCalled();
      unmount();

      resolveSession?.({
        id: 'session-after-unmount',
        name: 'untitled',
        message_count: 0,
        created_at: '2026-08-21T00:00:00.000Z',
        updated_at: '2026-08-21T00:00:00.000Z',
        working_dir: '/tmp/hub-dir',
        extension_data: { active: [], installed: [] },
      });

      await waitFor(() => {
        expect(onSessionCreated).toHaveBeenCalled();
        expect(onAddActiveSession).toHaveBeenCalled();
      });
      expect(mockSetSearchParams).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener(AppEvents.SESSION_CREATED, onSessionCreated);
      window.removeEventListener(AppEvents.ADD_ACTIVE_SESSION, onAddActiveSession);
    }
  });

  it('should navigate home when the main process emits new-chat', async () => {
    mockElectron.getConfig.mockReturnValue({
      GOOSE_DEFAULT_PROVIDER: 'openai',
      GOOSE_DEFAULT_MODEL: 'gpt-4',
      GOOSE_ALLOWLIST_WARNING: false,
    });

    render(<AppInner />, { wrapper: AppInnerTestWrapper });

    await waitFor(() => {
      expect(mockElectron.reactReady).toHaveBeenCalled();
    });

    const newChatHandler = mockElectron.on.mock.calls.find(
      ([channel]) => channel === 'new-chat'
    )?.[1];
    expect(newChatHandler).toBeDefined();

    newChatHandler?.({} as any);

    expect(mockNavigate).toHaveBeenCalledWith('/');
  });

  it('should reconnect ACP when the main process emits system-resume', async () => {
    render(<AppInner />, { wrapper: AppInnerTestWrapper });

    await waitFor(() => {
      expect(mockElectron.reactReady).toHaveBeenCalled();
    });

    const systemResumeHandler = mockElectron.on.mock.calls.find(
      ([channel]) => channel === 'system-resume'
    )?.[1];
    expect(systemResumeHandler).toBeDefined();

    systemResumeHandler?.({} as any);

    expect(reconnectAcpAfterSystemResume).toHaveBeenCalledOnce();
  });

  it('should seed recipe sessions with the recipe prompt when no initial message is provided', () => {
    expect(
      resolveSessionInitialMessage(
        {
          recipe: {
            prompt: 'Write a release note for the latest change',
          },
        },
        undefined
      )
    ).toEqual({
      msg: 'Write a release note for the latest change',
      images: [],
    });
  });
});

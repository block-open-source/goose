import '@testing-library/jest-dom';
import { vi, afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

// Mock Electron modules before any imports
vi.mock('electron', () => ({
  app: {
    getPath: vi.fn((name: string) => {
      if (name === 'userData') return '/tmp/test-user-data';
      if (name === 'temp') return '/tmp';
      if (name === 'home') return '/tmp/home';
      return '/tmp';
    }),
  },
  ipcRenderer: {
    invoke: vi.fn(),
    send: vi.fn(),
    on: vi.fn(),
    off: vi.fn(),
  },
}));

// This is the standard set up to ensure that React Testing Library's
// automatic cleanup runs after each test.
afterEach(() => {
  cleanup();
});

// Mock console methods to avoid noise in tests
// eslint-disable-next-line no-undef
global.console = {
  ...console,
  log: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
};

// Mock window.navigator.clipboard for copy functionality tests
Object.assign(navigator, {
  clipboard: {
    writeText: vi.fn(() => Promise.resolve()),
  },
});

// Mock settings store for tests
const mockSettings: Record<string, unknown> = {
  showMenuBarIcon: true,
  showDockIcon: true,
  enableWakelock: false,
  spellcheckEnabled: true,
  keyboardShortcuts: {
    focusWindow: 'CommandOrControl+Alt+G',
    quickLauncher: 'CommandOrControl+Alt+Shift+G',
    newChat: 'CommandOrControl+T',
    newChatWindow: 'CommandOrControl+N',
    openDirectory: 'CommandOrControl+O',
    settings: 'CommandOrControl+,',
    find: 'CommandOrControl+F',
    findNext: 'CommandOrControl+G',
    findPrevious: 'CommandOrControl+Shift+G',
    alwaysOnTop: 'CommandOrControl+Shift+T',
  },
  externalGoosed: {
    enabled: false,
    url: '',
    secret: '',
  },
  theme: 'light',
  useSystemTheme: true,
  language: 'system',
  responseStyle: 'concise',
  showPricing: true,
  seenAnnouncementIds: [],
};

// Mock window.electron for renderer process
Object.defineProperty(window, 'electron', {
  writable: true,
  value: {
    platform: 'darwin',
    getSetting: vi.fn((key: string) => Promise.resolve(mockSettings[key])),
    setSetting: vi.fn((key: string, value: unknown) => {
      mockSettings[key] = value;
      return Promise.resolve();
    }),
    reloadApp: vi.fn(),
    showMessageBox: vi.fn(() => Promise.resolve({ response: 0 })),
    getIsFullScreen: vi.fn(() => Promise.resolve(false)),
    logInfo: vi.fn(),
    logError: vi.fn(),
    on: vi.fn(),
    off: vi.fn(),
  },
});

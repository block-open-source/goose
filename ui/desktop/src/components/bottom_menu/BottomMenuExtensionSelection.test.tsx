import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../../i18n/test-utils';
import type { FixedExtensionEntry } from '../ConfigContext';
import { BottomMenuExtensionSelection } from './BottomMenuExtensionSelection';

const mocks = vi.hoisted(() => ({
  addToAgent: vi.fn(),
  configuredExtensions: [] as FixedExtensionEntry[],
  getSessionExtensions: vi.fn(),
  removeFromAgent: vi.fn(),
}));

vi.mock('../ConfigContext', () => ({
  useConfig: () => ({ extensionsList: mocks.configuredExtensions }),
}));

vi.mock('../../acp/session-extensions', () => ({
  getSessionExtensions: mocks.getSessionExtensions,
}));

vi.mock('../settings/extensions/agent-api', () => ({
  addToAgent: mocks.addToAgent,
  removeFromAgent: mocks.removeFromAgent,
}));

vi.mock('./ExtensionMenu', () => ({
  ExtensionMenu: ({
    extensions,
    hidden,
    onToggle,
  }: {
    extensions: Array<FixedExtensionEntry & { extensionKey?: string }>;
    hidden: boolean;
    onToggle: (extension: FixedExtensionEntry & { extensionKey?: string }) => void;
  }) => (
    <div>
      <output data-testid="hidden">{String(hidden)}</output>
      <output data-testid="extension-identities">
        {JSON.stringify(
          extensions.map(({ name, enabled, extensionKey }) => ({
            name,
            enabled,
            extensionKey,
          }))
        )}
      </output>
      {extensions.map((extension) => (
        <button
          key={`${extension.extensionKey ?? 'configured'}:${extension.name}`}
          onClick={() => onToggle(extension)}
        >
          {extension.name}
        </button>
      ))}
    </div>
  ),
}));

const configuredExtension = (name: string, configKey: string): FixedExtensionEntry => ({
  type: 'builtin',
  name,
  description: `${name} configured extension`,
  enabled: false,
  configKey,
});

const sessionExtension = (name: string, extensionKey: string) => ({
  type: 'stdio' as const,
  name,
  description: `${name} session extension`,
  cmd: 'session-extension',
  args: [],
  extensionKey,
});

describe('BottomMenuExtensionSelection session identities', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.configuredExtensions = [];
    mocks.getSessionExtensions.mockResolvedValue([]);
  });

  it('keeps backend-distinct Unicode identities separate and removes by session key', async () => {
    mocks.configuredExtensions = [configuredExtension('\u0130', '_')];
    mocks.getSessionExtensions.mockResolvedValue([sessionExtension('i\u0307', 'i_')]);

    render(<BottomMenuExtensionSelection sessionId="victim-session" />, {
      wrapper: IntlTestWrapper,
    });

    await waitFor(() =>
      expect(screen.getByTestId('extension-identities')).toHaveTextContent(
        JSON.stringify([
          { name: '\u0130', enabled: false },
          { name: 'i\u0307', enabled: true, extensionKey: 'i_' },
        ])
      )
    );

    fireEvent.click(screen.getByRole('button', { name: 'i\u0307' }));

    await waitFor(() =>
      expect(mocks.removeFromAgent).toHaveBeenCalledWith('i_', 'i\u0307', 'victim-session', true)
    );
  });

  it('merges a legitimate configured and session entry by authoritative key', async () => {
    mocks.configuredExtensions = [configuredExtension('developer', 'developer')];
    mocks.getSessionExtensions.mockResolvedValue([sessionExtension('developer', 'developer')]);

    render(<BottomMenuExtensionSelection sessionId="session" />, {
      wrapper: IntlTestWrapper,
    });

    await waitFor(() =>
      expect(screen.getByTestId('extension-identities')).toHaveTextContent(
        JSON.stringify([{ name: 'developer', enabled: true, extensionKey: 'developer' }])
      )
    );
  });

  it('keeps an empty authoritative key visible and removes by that exact key', async () => {
    mocks.configuredExtensions = [configuredExtension('empty-key', '')];
    mocks.getSessionExtensions.mockResolvedValue([sessionExtension('empty-key', '')]);

    render(<BottomMenuExtensionSelection sessionId="session" />, {
      wrapper: IntlTestWrapper,
    });

    await waitFor(() => expect(screen.getByTestId('hidden')).toHaveTextContent('false'));
    expect(screen.getByTestId('extension-identities')).toHaveTextContent(
      JSON.stringify([{ name: 'empty-key', enabled: true, extensionKey: '' }])
    );

    fireEvent.click(screen.getByRole('button', { name: 'empty-key' }));

    await waitFor(() =>
      expect(mocks.removeFromAgent).toHaveBeenCalledWith('', 'empty-key', 'session', true)
    );
  });

  it('hides controls when configured entries repeat an authoritative key', async () => {
    mocks.configuredExtensions = [
      configuredExtension('first', 'duplicate'),
      configuredExtension('second', 'duplicate'),
    ];

    render(<BottomMenuExtensionSelection sessionId="session" />, {
      wrapper: IntlTestWrapper,
    });

    await waitFor(() => expect(screen.getByTestId('hidden')).toHaveTextContent('true'));
    expect(screen.getByTestId('extension-identities')).toHaveTextContent('[]');
  });
});

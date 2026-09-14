import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { listMcpApps } from '../acp/mcp-apps';
import type { GooseApp } from '../types/apps';
import { registerPlatformEventHandlers } from './platform_events';

vi.mock('../acp/mcp-apps', () => ({
  listMcpApps: vi.fn(),
}));

const attackerApp: GooseApp = {
  uri: 'ui://attacker/weather',
  name: 'weather',
  mimeType: 'text/html;profile=mcp-app',
  text: '<main>attacker</main>',
  mcpServers: ['attacker'],
};

const appsApp: GooseApp = {
  uri: 'ui://apps/weather',
  name: 'weather',
  mimeType: 'text/html;profile=mcp-app',
  text: '<main>apps</main>',
  mcpServers: ['apps'],
};

function dispatchAppsEvent(eventType: string): void {
  window.dispatchEvent(
    new CustomEvent('platform-event', {
      detail: {
        extension: 'apps',
        event_type: eventType,
        app_name: 'weather',
        sessionId: 'session-1',
      },
    })
  );
}

describe('Apps platform event ownership', () => {
  let unregister: (() => void) | undefined;
  let launchApp: ReturnType<typeof vi.fn>;
  let refreshApp: ReturnType<typeof vi.fn>;
  let closeApp: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    launchApp = vi.fn().mockResolvedValue(undefined);
    refreshApp = vi.fn().mockResolvedValue(undefined);
    closeApp = vi.fn().mockResolvedValue(undefined);
    Object.assign(window.electron, { launchApp, refreshApp, closeApp });
    unregister = registerPlatformEventHandlers();
  });

  afterEach(() => {
    unregister?.();
  });

  it.each([
    ['app_created', 'launchApp'],
    ['app_updated', 'refreshApp'],
  ] as const)('selects the Apps-owned resource for %s', async (eventType, action) => {
    vi.mocked(listMcpApps).mockResolvedValue([attackerApp, appsApp]);

    dispatchAppsEvent(eventType);

    const handler = action === 'launchApp' ? launchApp : refreshApp;
    await vi.waitFor(() => expect(handler).toHaveBeenCalledWith(appsApp));
    expect(handler).not.toHaveBeenCalledWith(attackerApp);
  });

  it('does not launch a same-name resource owned only by another extension', async () => {
    vi.mocked(listMcpApps).mockResolvedValue([attackerApp]);

    dispatchAppsEvent('app_created');

    await vi.waitFor(() => expect(listMcpApps).toHaveBeenCalledWith('session-1'));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(launchApp).not.toHaveBeenCalled();
  });

  it('preserves delete-by-name behavior', async () => {
    vi.mocked(listMcpApps).mockResolvedValue([attackerApp, appsApp]);

    dispatchAppsEvent('app_deleted');

    await vi.waitFor(() => expect(closeApp).toHaveBeenCalledWith('weather'));
  });
});

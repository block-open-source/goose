import type { GooseApp } from '../types/apps';
import { listMcpApps } from '../acp/mcp-apps';

interface PlatformEventData {
  extension: string;
  sessionId?: string;
  [key: string]: unknown;
}

interface AppsEventData extends PlatformEventData {
  app_name?: string;
  sessionId: string;
}

type PlatformEventHandler = (eventType: string, data: PlatformEventData) => Promise<void>;

async function handleAppsEvent(eventType: string, eventData: PlatformEventData): Promise<void> {
  const { app_name, sessionId } = eventData as AppsEventData;

  if (!sessionId) {
    console.warn('No sessionId in apps platform event, skipping');
    return;
  }

  const apps = await listMcpApps(sessionId);

  const targetApp = apps.find(
    (app: GooseApp) => app.name === app_name && app.mcpServers?.includes(eventData.extension)
  );

  switch (eventType) {
    case 'app_created':
      if (targetApp) {
        await window.electron.launchApp(targetApp).catch((err) => {
          console.error('Failed to launch newly created app:', err);
        });
      }
      break;

    case 'app_updated':
      if (targetApp) {
        await window.electron.refreshApp(targetApp).catch((err) => {
          console.error('Failed to refresh updated app:', err);
        });
      }
      break;

    case 'app_deleted':
      if (app_name) {
        await window.electron.closeApp(app_name).catch((err) => {
          console.error('Failed to close deleted app:', err);
        });
      }
      break;

    default:
      console.warn(`Unknown apps event type: ${eventType}`);
  }
}

const EXTENSION_HANDLERS: Record<string, PlatformEventHandler> = {
  apps: handleAppsEvent,
};

export function maybeHandlePlatformEvent(notification: unknown, sessionId: string): void {
  if (notification && typeof notification === 'object' && 'method' in notification) {
    const msg = notification as { method?: string; params?: unknown };
    if (msg.method === 'platform_event' && msg.params) {
      window.dispatchEvent(
        new CustomEvent('platform-event', {
          detail: { ...msg.params, sessionId },
        })
      );
    }
  }
}

export function registerPlatformEventHandlers(): () => void {
  const handler = (event: Event) => {
    const customEvent = event as CustomEvent;
    const { extension, event_type, ...data } = customEvent.detail;

    const extensionHandler = EXTENSION_HANDLERS[extension];
    if (extensionHandler) {
      extensionHandler(event_type, { ...data, extension }).catch((err) => {
        console.error(`Platform event handler failed for ${extension}:`, err);
      });
    }
  };

  window.addEventListener('platform-event', handler);
  return () => window.removeEventListener('platform-event', handler);
}

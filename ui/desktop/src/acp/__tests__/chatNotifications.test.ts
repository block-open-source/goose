import type { SessionNotification } from '@agentclientprotocol/sdk';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AppEvents } from '../../constants/events';
import { ChatState } from '../../types/chatState';
import type { Session } from '../../types/session';
import { maybeHandlePlatformEvent } from '../../utils/platform_events';
import {
  handleAcpGooseSessionNotification,
  handleAcpSessionNotification,
} from '../chatNotifications';
import type { AcpChatSessionSnapshot } from '../chatSessionStore';
import { acpChatSessionActions, acpChatSessionStore } from '../chatSessionStore';
import { subscribeToLiveVoiceCallEnded } from '../liveVoiceNotifications';

vi.mock('../chatSessionStore', () => ({
  acpChatSessionStore: {
    getSnapshot: vi.fn(),
  },
  acpChatSessionActions: {
    applyAcpSessionNotification: vi.fn(),
    applyAcpGooseSessionNotification: vi.fn(),
  },
}));

vi.mock('../../utils/platform_events', () => ({
  maybeHandlePlatformEvent: vi.fn(),
}));

const SESSION_ID = 'session-1';

function sessionInfoUpdate(title: string): SessionNotification {
  return {
    sessionId: SESSION_ID,
    update: {
      sessionUpdate: 'session_info_update',
      title,
    },
  };
}

function platformEventToolUpdate(status: 'in_progress' | 'completed'): SessionNotification {
  return {
    sessionId: SESSION_ID,
    update: {
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tool-1',
      status,
      _meta: {
        toolNotification: {
          type: 'platform_event',
          params: {
            extension: 'apps',
            event_type: 'app_created',
            app_name: 'platform-event-repro',
          },
        },
      },
    },
  };
}

function sessionWithName(name: string): Session {
  return {
    id: SESSION_ID,
    name,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    working_dir: '/tmp',
    message_count: 0,
    extension_data: {},
    source: 'test',
  } as Session;
}

function snapshotWithName(name: string): AcpChatSessionSnapshot {
  return {
    session: sessionWithName(name),
    messages: [],
    tokenState: {
      inputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      accumulatedInputTokens: 0,
      accumulatedOutputTokens: 0,
      accumulatedTotalTokens: 0,
    },
    notifications: [],
    progressMessage: undefined,
    chatState: ChatState.Idle,
    sessionLoadError: undefined,
    activePromptAttemptId: null,
    activeRunId: null,
    pendingCancelPromptAttemptId: null,
  };
}

function snapshotWithoutSession(): AcpChatSessionSnapshot {
  return {
    session: undefined,
    messages: [],
    tokenState: {
      inputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      accumulatedInputTokens: 0,
      accumulatedOutputTokens: 0,
      accumulatedTotalTokens: 0,
    },
    notifications: [],
    progressMessage: undefined,
    chatState: ChatState.Idle,
    sessionLoadError: undefined,
    activePromptAttemptId: null,
    activeRunId: null,
    pendingCancelPromptAttemptId: null,
  };
}

describe('handleAcpSessionNotification', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('dispatches SESSION_RENAMED when a session info notification changes the name', async () => {
    const dispatchEvent = vi.spyOn(window, 'dispatchEvent');
    vi.mocked(acpChatSessionStore.getSnapshot).mockReturnValueOnce(snapshotWithName('Old name'));
    vi.mocked(acpChatSessionActions.applyAcpSessionNotification).mockReturnValueOnce(
      snapshotWithName('New name')
    );

    await handleAcpSessionNotification(sessionInfoUpdate('New name'));

    expect(dispatchEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        type: AppEvents.SESSION_RENAMED,
        detail: { sessionId: SESSION_ID, newName: 'New name' },
      })
    );
  });

  it('does not dispatch SESSION_RENAMED when the name is unchanged', async () => {
    const dispatchEvent = vi.spyOn(window, 'dispatchEvent');
    vi.mocked(acpChatSessionStore.getSnapshot).mockReturnValueOnce(snapshotWithName('Same name'));
    vi.mocked(acpChatSessionActions.applyAcpSessionNotification).mockReturnValueOnce(
      snapshotWithName('Same name')
    );

    await handleAcpSessionNotification(sessionInfoUpdate('Same name'));

    expect(dispatchEvent).not.toHaveBeenCalled();
  });

  it('dispatches SESSION_RENAMED from the notification title when the session is not loaded', async () => {
    const dispatchEvent = vi.spyOn(window, 'dispatchEvent');
    vi.mocked(acpChatSessionStore.getSnapshot).mockReturnValueOnce(snapshotWithoutSession());
    vi.mocked(acpChatSessionActions.applyAcpSessionNotification).mockReturnValueOnce(
      snapshotWithoutSession()
    );

    await handleAcpSessionNotification(sessionInfoUpdate('Generated name'));

    expect(dispatchEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        type: AppEvents.SESSION_RENAMED,
        detail: { sessionId: SESSION_ID, newName: 'Generated name' },
      })
    );
  });

  it('forwards live ACP platform events to the desktop platform event handler', async () => {
    await handleAcpSessionNotification(platformEventToolUpdate('in_progress'));

    expect(maybeHandlePlatformEvent).toHaveBeenCalledWith(
      {
        method: 'platform_event',
        params: {
          extension: 'apps',
          event_type: 'app_created',
          app_name: 'platform-event-repro',
        },
      },
      SESSION_ID
    );
  });

  it('does not forward completed platform event metadata as a live desktop event', async () => {
    await handleAcpSessionNotification(platformEventToolUpdate('completed'));

    expect(maybeHandlePlatformEvent).not.toHaveBeenCalled();
  });

  it('routes Live voice call endings outside the conversation store', async () => {
    const listener = vi.fn();
    const unsubscribe = subscribeToLiveVoiceCallEnded(listener);

    await handleAcpGooseSessionNotification({
      sessionId: SESSION_ID,
      update: {
        sessionUpdate: 'live_voice_call_ended',
        callId: 'live-opaque',
        outcome: 'failed',
      },
    });

    expect(listener).toHaveBeenCalledWith({
      sessionId: SESSION_ID,
      update: {
        sessionUpdate: 'live_voice_call_ended',
        callId: 'live-opaque',
        outcome: 'failed',
      },
    });
    expect(acpChatSessionActions.applyAcpGooseSessionNotification).not.toHaveBeenCalled();
    unsubscribe();
  });
});

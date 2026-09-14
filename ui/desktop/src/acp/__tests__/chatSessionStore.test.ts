import type { RequestPermissionRequest, SessionNotification } from '@agentclientprotocol/sdk';
import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import type { Message } from '../../types/message';
import { ChatState } from '../../types/chatState';
import type { Session } from '../../types/session';
import {
  acpElicitationUserInputRequestId,
  acpChatSessionActions,
  acpPermissionUserInputRequestId,
  acpChatSessionStore,
  useAcpChatSessionSnapshot,
} from '../chatSessionStore';
import type { AcpElicitationRequest } from '../elicitationRequests';
import type { AcpPermissionRequest } from '../permissionRequestTypes';

function message(id: string, text: string): Message {
  return {
    id,
    role: 'user',
    created: 123,
    content: [{ type: 'text', text }],
    metadata: { userVisible: true, agentVisible: true },
  };
}

function session(id: string, conversation: Message[] = []): Session {
  return {
    id,
    name: `Session ${id}`,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    working_dir: '/tmp',
    message_count: conversation.length,
    extension_data: {},
    source: 'test',
    conversation,
    usage: { input_tokens: 1, output_tokens: 2, total_tokens: 3 },
    accumulated_usage: { input_tokens: 4, output_tokens: 5, total_tokens: 9 },
  } as Session;
}

function permissionRequest(sessionId: string, toolCallId = 'tool-1'): AcpPermissionRequest {
  const request: RequestPermissionRequest = {
    sessionId,
    options: [{ optionId: 'allow-once', name: 'Allow once', kind: 'allow_once' }],
    toolCall: {
      toolCallId,
      title: 'Edit file',
      rawInput: { path: 'README.md' },
      content: [
        {
          type: 'content',
          content: { type: 'text', text: 'Allow editing README.md?' },
        },
      ],
      _meta: {
        goose: {
          toolCall: {
            toolName: 'edit_file',
          },
        },
      },
    },
  };
  return { generation: `generation-${toolCallId}`, request };
}

function elicitationRequest(sessionId: string): AcpElicitationRequest {
  return {
    id: 'acp_elicitation_1',
    sessionId,
    request: {
      mode: 'form',
      sessionId,
      message: 'Choose a project',
      requestedSchema: {
        type: 'object',
        properties: {
          project: {
            type: 'string',
          },
        },
      },
    },
  };
}

function toolProgressNotification(sessionId: string): SessionNotification {
  return {
    sessionId,
    update: {
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tool-1',
      status: 'in_progress',
      _meta: {
        toolNotification: {
          type: 'progress',
          params: {
            progressToken: 'scan-repo',
            progress: 3,
          },
        },
      },
    },
  };
}

function agentMessageChunkNotification(
  sessionId: string,
  messageId: string,
  text: string
): SessionNotification {
  return {
    sessionId,
    update: {
      sessionUpdate: 'agent_message_chunk',
      messageId,
      content: {
        type: 'text',
        text,
      },
    },
  };
}

function userSteerChunkNotification(
  sessionId: string,
  messageId: string,
  text: string
): SessionNotification {
  return {
    sessionId,
    update: {
      sessionUpdate: 'user_message_chunk',
      messageId,
      content: {
        type: 'text',
        text,
      },
      _meta: {
        goose: {
          messageId,
          steer: true,
        },
      },
    } as SessionNotification['update'],
  };
}

function activeRunNotification(sessionId: string, activeRunId: string | null): SessionNotification {
  return {
    sessionId,
    update: {
      sessionUpdate: 'session_info_update',
      _meta: {
        goose: {
          activeRunId,
        },
      },
    } as SessionNotification['update'],
  };
}

function queuedSteerNotification(sessionId: string, messageId: string): SessionNotification {
  return {
    sessionId,
    update: {
      sessionUpdate: 'session_info_update',
      _meta: {
        goose: {
          queuedSteer: { messageId, runId: 'run-1' },
        },
      },
    } as SessionNotification['update'],
  };
}

describe('acpChatSessionStore', () => {
  const sessionIds = new Set<string>();
  const sessionId = (id: string): string => {
    sessionIds.add(id);
    return id;
  };

  afterEach(() => {
    for (const id of sessionIds) {
      acpChatSessionActions.deleteSnapshot(id);
    }
    sessionIds.clear();
  });

  it('finishes session load with session metadata', () => {
    const currentSessionId = sessionId('session-1');
    const initialMessage = message('message-1', 'Hello');

    acpChatSessionActions.setMessages(currentSessionId, [initialMessage]);

    const snapshot = acpChatSessionActions.finishSessionLoad(
      currentSessionId,
      session(currentSessionId)
    );

    expect(snapshot.session?.id).toBe(currentSessionId);
    expect(snapshot.messages).toEqual([initialMessage]);
    expect(snapshot.chatState).toBe(ChatState.Idle);
    expect(snapshot.sessionLoadError).toBeUndefined();
  });

  it('keeps multiple session snapshots isolated', () => {
    const firstSessionId = sessionId('session-1');
    const secondSessionId = sessionId('session-2');

    acpChatSessionActions.setMessages(firstSessionId, [message('message-1', 'One')]);
    acpChatSessionActions.setMessages(secondSessionId, [message('message-2', 'Two')]);

    expect(acpChatSessionStore.getSnapshot(firstSessionId)?.messages[0].id).toBe('message-1');
    expect(acpChatSessionStore.getSnapshot(secondSessionId)?.messages[0].id).toBe('message-2');
  });

  it('deletes session snapshots', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.setMessages(currentSessionId, [message('message-1', 'One')]);

    acpChatSessionActions.deleteSnapshot(currentSessionId);

    expect(acpChatSessionStore.getSnapshot(currentSessionId)).toBeUndefined();
  });

  it('ignores stale prompt attempts and leaves the current attempt active', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-a');
    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-b');

    expect(acpChatSessionActions.finishPromptAttemptIfCurrent(currentSessionId, 'attempt-a')).toBe(
      false
    );

    expect(acpChatSessionStore.getSnapshot(currentSessionId)).toMatchObject({
      activePromptAttemptId: 'attempt-b',
      chatState: ChatState.Streaming,
      sessionLoadError: undefined,
    });

    expect(acpChatSessionActions.finishPromptAttemptIfCurrent(currentSessionId, 'attempt-b')).toBe(
      true
    );
    expect(acpChatSessionStore.getSnapshot(currentSessionId)).toMatchObject({
      activePromptAttemptId: null,
      chatState: ChatState.Idle,
    });
  });

  it('keeps loaded sessions streaming when a prompt attempt is active', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');

    const snapshot = acpChatSessionActions.finishSessionLoad(
      currentSessionId,
      session(currentSessionId)
    );

    expect(snapshot.activePromptAttemptId).toBe('attempt-1');
    expect(snapshot.chatState).toBe(ChatState.Streaming);
  });

  it('tracks prompt cancellation separately from visible prompt activity', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    const cancellationSnapshot = acpChatSessionActions.startPromptCancellation(
      currentSessionId,
      'attempt-1'
    );

    expect(cancellationSnapshot).toMatchObject({
      activePromptAttemptId: null,
      pendingCancelPromptAttemptId: 'attempt-1',
      chatState: ChatState.Idle,
    });

    const staleClearSnapshot = acpChatSessionActions.clearPromptCancellation(
      currentSessionId,
      'attempt-2'
    );
    expect(staleClearSnapshot).toBeUndefined();
    expect(acpChatSessionStore.getSnapshot(currentSessionId)?.pendingCancelPromptAttemptId).toBe(
      'attempt-1'
    );

    const clearedSnapshot = acpChatSessionActions.clearPromptCancellation(
      currentSessionId,
      'attempt-1'
    );

    expect(clearedSnapshot?.pendingCancelPromptAttemptId).toBeNull();
  });

  it('restores pending user input tracking when prompt cancellation is restored', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.applyPermissionRequest(permissionRequest(currentSessionId, 'tool-1'));
    acpChatSessionActions.applyElicitationRequest(elicitationRequest(currentSessionId));
    acpChatSessionActions.startPromptCancellation(currentSessionId, 'attempt-1');

    const restoredSnapshot = acpChatSessionActions.restorePromptCancellation(
      currentSessionId,
      'attempt-1'
    );

    expect(restoredSnapshot?.chatState).toBe(ChatState.WaitingForUserInput);

    const afterPermission = acpChatSessionActions.resolveUserInputRequest(
      currentSessionId,
      acpPermissionUserInputRequestId('tool-1')
    );

    expect(afterPermission?.chatState).toBe(ChatState.WaitingForUserInput);

    const afterElicitation = acpChatSessionActions.resolveUserInputRequest(
      currentSessionId,
      acpElicitationUserInputRequestId('acp_elicitation_1')
    );

    expect(afterElicitation?.chatState).toBe(ChatState.Streaming);
  });

  it('waits for prompt cancellation to clear', async () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.startPromptCancellation(currentSessionId, 'attempt-1');

    let didResolve = false;
    const waitPromise = acpChatSessionActions
      .waitForPromptCancellation(currentSessionId, 'attempt-1')
      .then(() => {
        didResolve = true;
      });

    await Promise.resolve();
    expect(didResolve).toBe(false);

    acpChatSessionActions.clearPromptCancellation(currentSessionId, 'attempt-1');

    await waitPromise;
    expect(didResolve).toBe(true);
  });

  it('removes pending local steer messages when cancellation starts', () => {
    const currentSessionId = sessionId('session-1');
    const localSteerMessage = {
      ...message('steer-1', 'hello'),
      metadata: { userVisible: true, agentVisible: true, steer: true },
    };

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.addPendingLocalSteerMessage(currentSessionId, localSteerMessage);

    expect(acpChatSessionStore.getSnapshot(currentSessionId)?.messages).toHaveLength(1);

    const cancellationSnapshot = acpChatSessionActions.startPromptCancellation(
      currentSessionId,
      'attempt-1'
    );

    expect(cancellationSnapshot?.messages).toEqual([]);
  });

  it('keeps confirmed local steer messages when cancellation starts', () => {
    const currentSessionId = sessionId('session-1');
    const localSteerMessage = {
      ...message('steer-1', 'hello'),
      metadata: { userVisible: true, agentVisible: true, steer: true },
    };

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.addPendingLocalSteerMessage(currentSessionId, localSteerMessage);
    acpChatSessionActions.applyAcpSessionNotification(
      userSteerChunkNotification(currentSessionId, 'steer-1', 'hello')
    );

    const cancellationSnapshot = acpChatSessionActions.startPromptCancellation(
      currentSessionId,
      'attempt-1'
    );

    expect(cancellationSnapshot?.messages).toHaveLength(1);
    expect(cancellationSnapshot?.messages[0].id).toBe('steer-1');
  });

  it('preserves steer text accumulation when another local steer is added', () => {
    const currentSessionId = sessionId('session-1');
    const firstSteerMessage = {
      ...message('steer-1', 'hello'),
      metadata: { userVisible: true, agentVisible: true, steer: true },
    };
    const secondSteerMessage = {
      ...message('steer-2', 'second'),
      metadata: { userVisible: true, agentVisible: true, steer: true },
    };

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.addPendingLocalSteerMessage(currentSessionId, firstSteerMessage);
    acpChatSessionActions.applyAcpSessionNotification(
      userSteerChunkNotification(currentSessionId, 'steer-1', 'hel')
    );

    acpChatSessionActions.addPendingLocalSteerMessage(currentSessionId, secondSteerMessage);
    const snapshot = acpChatSessionActions.applyAcpSessionNotification(
      userSteerChunkNotification(currentSessionId, 'steer-1', 'lo')
    );

    const firstMessage = snapshot.messages.find((item) => item.id === 'steer-1');
    expect(firstMessage?.content[0]).toMatchObject({ type: 'text', text: 'hello' });
  });

  it('keeps steer message confirmed when queuedSteer arrives before addPendingLocalSteerMessage', () => {
    const currentSessionId = sessionId('session-1');
    const localSteerMessage = {
      ...message('steer-1', 'hello'),
      metadata: { userVisible: true, agentVisible: true, steer: true },
    };

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');

    // queuedSteer notification arrives before addPendingLocalSteerMessage (race condition)
    acpChatSessionActions.applyAcpSessionNotification(
      queuedSteerNotification(currentSessionId, 'steer-1')
    );

    // Now the UI adds the pending message after the RPC response returns
    acpChatSessionActions.addPendingLocalSteerMessage(currentSessionId, localSteerMessage);

    // Message should be present and NOT in pending (already confirmed via queuedSteer)
    const snapshot = acpChatSessionStore.getSnapshot(currentSessionId);
    expect(snapshot?.messages).toHaveLength(1);

    // Cancellation should keep it because it's confirmed, not pending
    const cancellationSnapshot = acpChatSessionActions.startPromptCancellation(
      currentSessionId,
      'attempt-1'
    );
    expect(cancellationSnapshot?.messages).toHaveLength(1);
    expect(cancellationSnapshot?.messages[0].id).toBe('steer-1');
  });

  it('stores active run ids from session info notifications', () => {
    const currentSessionId = sessionId('session-1');

    const snapshot = acpChatSessionActions.applyAcpSessionNotification(
      activeRunNotification(currentSessionId, 'run-1')
    );

    expect(snapshot.activeRunId).toBe('run-1');
    expect(acpChatSessionStore.getSnapshot(currentSessionId)?.activeRunId).toBe('run-1');

    const clearedSnapshot = acpChatSessionActions.applyAcpSessionNotification(
      activeRunNotification(currentSessionId, null)
    );

    expect(clearedSnapshot.activeRunId).toBeNull();
  });

  it('clears active run ids when the prompt attempt finishes', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.applyAcpSessionNotification(
      activeRunNotification(currentSessionId, 'run-1')
    );

    expect(acpChatSessionActions.finishPromptAttemptIfCurrent(currentSessionId, 'attempt-1')).toBe(
      true
    );
    expect(acpChatSessionStore.getSnapshot(currentSessionId)?.activeRunId).toBeNull();
  });

  it('clears active run ids before replaying a session load', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.applyAcpSessionNotification(
      activeRunNotification(currentSessionId, 'run-1')
    );

    const snapshot = acpChatSessionActions.startSessionLoad(currentSessionId);

    expect(snapshot.activeRunId).toBeNull();
  });

  it('stores ACP tool notifications and clears them for a new prompt attempt', () => {
    const currentSessionId = sessionId('session-1');

    const snapshot = acpChatSessionActions.applyAcpSessionNotification(
      toolProgressNotification(currentSessionId)
    );

    expect(snapshot.notifications).toHaveLength(1);
    expect(snapshot.notifications[0]).toMatchObject({
      type: 'Notification',
      request_id: 'tool-1',
      message: {
        method: 'notifications/progress',
        params: {
          progressToken: 'scan-repo',
          progress: 3,
        },
      },
    });

    const nextSnapshot = acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');

    expect(nextSnapshot.notifications).toEqual([]);
  });

  it('resets replayed messages before starting an unloaded session load', () => {
    const currentSessionId = sessionId('session-1');
    const replayedChunk = agentMessageChunkNotification(currentSessionId, 'message-1', 'Hello');

    acpChatSessionActions.startSessionLoad(currentSessionId);
    acpChatSessionActions.applyAcpSessionNotification(replayedChunk);

    const loadingSnapshot = acpChatSessionActions.startSessionLoad(currentSessionId);
    expect(loadingSnapshot.messages).toEqual([]);

    // While a load replay is in flight, per-notification snapshots stay frozen
    // at the load-start state (avoids O(n^2) cloning on large sessions); the
    // replayed conversation materializes in the finishSessionLoad snapshot.
    const replayedSnapshot = acpChatSessionActions.applyAcpSessionNotification(replayedChunk);
    expect(replayedSnapshot.messages).toEqual([]);

    const finishedSnapshot = acpChatSessionActions.finishSessionLoad(
      currentSessionId,
      session(currentSessionId)
    );

    expect(finishedSnapshot.messages).toHaveLength(1);
    expect(finishedSnapshot.messages[0].content).toEqual([{ type: 'text', text: 'Hello' }]);
  });

  it('applies permission requests as waiting action-required messages', () => {
    const currentSessionId = sessionId('session-1');

    const snapshot = acpChatSessionActions.applyPermissionRequest(
      permissionRequest(currentSessionId, 'tool-1')
    );

    expect(snapshot.chatState).toBe(ChatState.WaitingForUserInput);
    expect(snapshot.messages).toHaveLength(1);
    expect(snapshot.messages[0].role).toBe('assistant');
    expect(snapshot.messages[0].content[0]).toMatchObject({
      type: 'actionRequired',
      data: {
        actionType: 'toolConfirmation',
        id: 'tool-1',
      },
    });
  });

  it('removes the current permission card when the request is cancelled', () => {
    const currentSessionId = sessionId('session-1');
    const request = permissionRequest(currentSessionId, 'tool-1');
    acpChatSessionActions.applyPermissionRequest(request);

    const snapshot = acpChatSessionActions.cancelPermissionRequest(
      currentSessionId,
      'tool-1',
      request.generation
    );

    expect(snapshot?.messages).toEqual([]);
  });

  it('resumes streaming only after the final pending user input request resolves', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.startPromptAttempt(currentSessionId, 'attempt-1');
    acpChatSessionActions.applyPermissionRequest(permissionRequest(currentSessionId, 'tool-1'));
    acpChatSessionActions.applyElicitationRequest(elicitationRequest(currentSessionId));

    const afterElicitation = acpChatSessionActions.resolveUserInputRequest(
      currentSessionId,
      acpElicitationUserInputRequestId('acp_elicitation_1')
    );

    expect(afterElicitation?.chatState).toBe(ChatState.WaitingForUserInput);

    const afterPermission = acpChatSessionActions.resolveUserInputRequest(
      currentSessionId,
      acpPermissionUserInputRequestId('tool-1')
    );

    expect(afterPermission?.chatState).toBe(ChatState.Streaming);
  });

  it('does not resume streaming after user input resolves without an active prompt', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.applyPermissionRequest(permissionRequest(currentSessionId, 'tool-1'));

    const snapshot = acpChatSessionActions.resolveUserInputRequest(
      currentSessionId,
      acpPermissionUserInputRequestId('tool-1')
    );

    expect(snapshot?.chatState).toBe(ChatState.WaitingForUserInput);
  });

  it('applies elicitation requests as waiting action-required messages', () => {
    const currentSessionId = sessionId('session-1');

    const snapshot = acpChatSessionActions.applyElicitationRequest(
      elicitationRequest(currentSessionId)
    );

    expect(snapshot.chatState).toBe(ChatState.WaitingForUserInput);
    expect(snapshot.messages).toHaveLength(1);
    expect(snapshot.messages[0].role).toBe('assistant');
    expect(snapshot.messages[0].content[0]).toMatchObject({
      type: 'actionRequired',
      data: {
        actionType: 'elicitation',
        id: 'acp_elicitation_1',
        message: 'Choose a project',
      },
    });
  });

  it('stores submitted elicitation status', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.applyElicitationRequest(elicitationRequest(currentSessionId));

    const snapshot = acpChatSessionActions.setElicitationStatus(
      currentSessionId,
      'acp_elicitation_1',
      'submitted'
    );

    expect(snapshot?.messages[0].content[0]).toMatchObject({
      type: 'actionRequired',
      data: {
        actionType: 'elicitation',
        id: 'acp_elicitation_1',
        isSubmitted: true,
        isCancelled: false,
      },
    });
  });

  it('stores cancelled elicitation status', () => {
    const currentSessionId = sessionId('session-1');

    acpChatSessionActions.applyElicitationRequest(elicitationRequest(currentSessionId));

    const snapshot = acpChatSessionActions.setElicitationStatus(
      currentSessionId,
      'acp_elicitation_1',
      'cancelled'
    );

    expect(snapshot?.messages[0].content[0]).toMatchObject({
      type: 'actionRequired',
      data: {
        actionType: 'elicitation',
        id: 'acp_elicitation_1',
        isSubmitted: false,
        isCancelled: true,
      },
    });
  });
});

describe('useAcpChatSessionSnapshot', () => {
  const sessionId = 'hook-session-1';

  afterEach(() => {
    acpChatSessionActions.deleteSnapshot(sessionId);
  });

  it('subscribes to session store snapshots', () => {
    const { result } = renderHook(() => useAcpChatSessionSnapshot(sessionId));

    expect(result.current).toBeUndefined();

    const nextMessage = message('message-1', 'Hello from hook');
    act(() => {
      acpChatSessionActions.setMessages(sessionId, [nextMessage]);
    });

    expect(result.current?.messages).toEqual([nextMessage]);
  });
});

import { useEffect, useState } from 'react';
import type { GooseSessionNotification_unstable } from '@aaif/goose-acp-client';
import type { SessionNotification } from '@agentclientprotocol/sdk';
import type { TokenState } from '../types/chat';
import { ChatState } from '../types/chatState';
import type { Message, NotificationEvent } from '../types/message';
import type { Session } from '../types/session';
import {
  createAcpSessionNotificationAdapter,
  type AcpChatStateChange,
  type AcpSessionNotificationAdapter,
} from './sessionNotificationAdapter';
import type { ElicitationStatus } from './adapter/elicitations';
import { cloneMessage } from './adapter/shared';
import type { AcpElicitationRequest } from './elicitationRequests';
import type { AcpPermissionRequest } from './permissionRequestTypes';

export interface AcpChatSessionSnapshot {
  session: Session | undefined;
  messages: Message[];
  tokenState: TokenState;
  notifications: NotificationEvent[];
  progressMessage: string | undefined;
  chatState: ChatState;
  sessionLoadError: string | undefined;
  activePromptAttemptId: string | null;
  activeRunId: string | null;
  pendingCancelPromptAttemptId: string | null;
}

type SnapshotListener = (snapshot: AcpChatSessionSnapshot) => void;

interface StoreEntry extends AcpChatSessionSnapshot {
  adapter: AcpSessionNotificationAdapter;
  promptCancellationRestoreState: {
    activeRunId: string | null;
    chatState: ChatState;
    pendingUserInputRequestIds: Set<string>;
  } | null;
  pendingUserInputRequestIds: Set<string>;
  pendingLocalSteerMessageIds: Set<string>;
  preConfirmedSteerMessageIds: Set<string>;
  // Cached result of the last notify(); reused while a session-load replay is
  // in flight so per-notification reads don't deep-clone the growing message
  // array (see applyAcpSessionNotification / getSnapshot).
  lastSnapshot?: AcpChatSessionSnapshot;
}

const initialTokenState: TokenState = {
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  accumulatedInputTokens: 0,
  accumulatedOutputTokens: 0,
  accumulatedTotalTokens: 0,
};

export interface AcpChatSessionStore {
  getSnapshot(sessionId: string): AcpChatSessionSnapshot | undefined;
}

export interface AcpChatSessionActions {
  deleteSnapshot(sessionId: string): void;

  applyAcpSessionNotification(notification: SessionNotification): AcpChatSessionSnapshot;
  applyAcpGooseSessionNotification(
    notification: GooseSessionNotification_unstable
  ): AcpChatSessionSnapshot;
  applyPermissionRequest(request: AcpPermissionRequest): AcpChatSessionSnapshot;
  cancelPermissionRequest(
    sessionId: string,
    toolCallId: string,
    generation: string
  ): AcpChatSessionSnapshot | undefined;
  applyElicitationRequest(request: AcpElicitationRequest): AcpChatSessionSnapshot;
  setElicitationStatus(
    sessionId: string,
    elicitationId: string,
    status: ElicitationStatus
  ): AcpChatSessionSnapshot | undefined;

  setSessionMetadata(sessionId: string, session: Session | undefined): AcpChatSessionSnapshot;
  startSessionLoad(sessionId: string): AcpChatSessionSnapshot;
  finishSessionLoad(sessionId: string, session: Session): AcpChatSessionSnapshot;
  failSessionLoad(sessionId: string, sessionLoadError: string): AcpChatSessionSnapshot;
  setSessionLoadError(
    sessionId: string,
    sessionLoadError: string | undefined
  ): AcpChatSessionSnapshot;

  setMessages(sessionId: string, messages: Message[]): AcpChatSessionSnapshot;
  addPendingLocalSteerMessage(sessionId: string, message: Message): AcpChatSessionSnapshot;
  setChatState(sessionId: string, chatState: ChatState): AcpChatSessionSnapshot;
  resolveUserInputRequest(
    sessionId: string,
    userInputRequestId: string
  ): AcpChatSessionSnapshot | undefined;

  startPromptAttempt(sessionId: string, promptAttemptId: string): AcpChatSessionSnapshot;
  startPromptCancellation(
    sessionId: string,
    promptAttemptId: string
  ): AcpChatSessionSnapshot | undefined;
  clearPromptCancellation(
    sessionId: string,
    promptAttemptId: string
  ): AcpChatSessionSnapshot | undefined;
  restorePromptCancellation(
    sessionId: string,
    promptAttemptId: string
  ): AcpChatSessionSnapshot | undefined;
  waitForPromptCancellation(sessionId: string, promptAttemptId: string): Promise<void>;
  finishPromptAttemptIfCurrent(sessionId: string, promptAttemptId: string): boolean;
  clearActivePromptAttempt(sessionId: string): AcpChatSessionSnapshot | undefined;
  isCurrentPromptAttempt(sessionId: string, promptAttemptId: string): boolean;
}

interface AcpChatSessionStoreInternal extends AcpChatSessionStore, AcpChatSessionActions {
  subscribe(sessionId: string, listener: (snapshot: AcpChatSessionSnapshot) => void): () => void;
}

function createAcpChatSessionStoreInternal(): AcpChatSessionStoreInternal {
  const sessionsById = new Map<string, StoreEntry>();
  const listenersBySessionId = new Map<string, Set<SnapshotListener>>();

  const getSnapshot: AcpChatSessionStore['getSnapshot'] = (sessionId) => {
    const entry = sessionsById.get(sessionId);
    if (!entry) {
      return undefined;
    }
    // While a session-load replay is streaming in, serve the snapshot cached
    // by the last notify() instead of deep-cloning the growing message array
    // on every read (getSnapshot is called per replay notification).
    if (entry.chatState === ChatState.LoadingConversation && entry.lastSnapshot) {
      return entry.lastSnapshot;
    }
    return snapshotFromEntry(entry);
  };

  const subscribe: AcpChatSessionStoreInternal['subscribe'] = (sessionId, listener) => {
    const listeners = listenersBySessionId.get(sessionId) ?? new Set<SnapshotListener>();
    listeners.add(listener);
    listenersBySessionId.set(sessionId, listeners);

    let subscribed = true;
    return () => {
      if (!subscribed) {
        return;
      }

      subscribed = false;
      const currentListeners = listenersBySessionId.get(sessionId);
      if (!currentListeners) {
        return;
      }

      currentListeners.delete(listener);
      if (currentListeners.size === 0) {
        listenersBySessionId.delete(sessionId);
      }
    };
  };

  const deleteSnapshot: AcpChatSessionActions['deleteSnapshot'] = (sessionId) => {
    sessionsById.delete(sessionId);
  };

  const getOrCreateEntry = (sessionId: string): StoreEntry => {
    const existing = sessionsById.get(sessionId);
    if (existing) {
      return existing;
    }

    const entry: StoreEntry = {
      session: undefined,
      messages: [],
      tokenState: { ...initialTokenState },
      notifications: [],
      progressMessage: undefined,
      chatState: ChatState.Idle,
      sessionLoadError: undefined,
      activePromptAttemptId: null,
      activeRunId: null,
      pendingCancelPromptAttemptId: null,
      promptCancellationRestoreState: null,
      pendingUserInputRequestIds: new Set(),
      pendingLocalSteerMessageIds: new Set(),
      preConfirmedSteerMessageIds: new Set(),
      adapter: createAcpSessionNotificationAdapter(),
    };
    sessionsById.set(sessionId, entry);
    return entry;
  };

  const notify = (sessionId: string, entry: StoreEntry): AcpChatSessionSnapshot => {
    const snapshot = snapshotFromEntry(entry);
    entry.lastSnapshot = snapshot;
    const listeners = listenersBySessionId.get(sessionId);
    if (listeners) {
      for (const listener of listeners) {
        listener(snapshot);
      }
    }
    return snapshot;
  };

  const setSessionMetadata: AcpChatSessionActions['setSessionMetadata'] = (sessionId, session) => {
    const entry = getOrCreateEntry(sessionId);
    entry.session = session;
    return notify(sessionId, entry);
  };

  const startSessionLoad: AcpChatSessionActions['startSessionLoad'] = (sessionId) => {
    const entry = getOrCreateEntry(sessionId);
    resetReplayState(entry);
    entry.sessionLoadError = undefined;
    entry.progressMessage = undefined;
    entry.chatState = ChatState.LoadingConversation;
    return notify(sessionId, entry);
  };

  const finishSessionLoad: AcpChatSessionActions['finishSessionLoad'] = (sessionId, session) => {
    const entry = getOrCreateEntry(sessionId);
    entry.session = session;
    entry.sessionLoadError = undefined;
    entry.progressMessage = undefined;
    // Materialize the replayed conversation in one pass (the per-notification
    // fast path above skips message copies while loading).
    entry.messages = entry.adapter.getMessages();
    retainPendingLocalSteerMessageIds(entry);
    entry.chatState = entry.activePromptAttemptId ? ChatState.Streaming : ChatState.Idle;
    return notify(sessionId, entry);
  };

  const failSessionLoad: AcpChatSessionActions['failSessionLoad'] = (
    sessionId,
    sessionLoadError
  ) => {
    const entry = getOrCreateEntry(sessionId);
    entry.sessionLoadError = sessionLoadError;
    entry.progressMessage = undefined;
    entry.chatState = ChatState.Idle;
    return notify(sessionId, entry);
  };

  const setMessages: AcpChatSessionActions['setMessages'] = (sessionId, messages) => {
    const entry = getOrCreateEntry(sessionId);
    entry.messages = cloneMessages(messages);
    retainPendingLocalSteerMessageIds(entry);
    entry.adapter = createAdapterForEntry(entry);
    return notify(sessionId, entry);
  };

  const addPendingLocalSteerMessage: AcpChatSessionActions['addPendingLocalSteerMessage'] = (
    sessionId,
    message
  ) => {
    const entry = getOrCreateEntry(sessionId);
    if (!message.id || entry.messages.some((existing) => existing.id === message.id)) {
      return notify(sessionId, entry);
    }

    entry.messages = [...entry.messages, cloneMessage(message)];
    if (!entry.preConfirmedSteerMessageIds.delete(message.id)) {
      entry.pendingLocalSteerMessageIds.add(message.id);
    }
    entry.adapter = createAdapterForEntry(entry);
    return notify(sessionId, entry);
  };

  const setChatState: AcpChatSessionActions['setChatState'] = (sessionId, chatState) => {
    const entry = getOrCreateEntry(sessionId);
    if (chatState === ChatState.Idle) {
      entry.progressMessage = undefined;
    }
    entry.chatState = chatState;
    return notify(sessionId, entry);
  };

  const resolveUserInputRequest: AcpChatSessionActions['resolveUserInputRequest'] = (
    sessionId,
    userInputRequestId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry) {
      return undefined;
    }

    entry.pendingUserInputRequestIds.delete(userInputRequestId);

    if (
      entry.activePromptAttemptId &&
      entry.chatState === ChatState.WaitingForUserInput &&
      entry.pendingUserInputRequestIds.size === 0
    ) {
      entry.chatState = ChatState.Streaming;
      return notify(sessionId, entry);
    }

    return snapshotFromEntry(entry);
  };

  const setSessionLoadError: AcpChatSessionActions['setSessionLoadError'] = (
    sessionId,
    sessionLoadError
  ) => {
    const entry = getOrCreateEntry(sessionId);
    entry.sessionLoadError = sessionLoadError;
    return notify(sessionId, entry);
  };

  const startPromptAttempt: AcpChatSessionActions['startPromptAttempt'] = (
    sessionId,
    promptAttemptId
  ) => {
    const entry = getOrCreateEntry(sessionId);
    discardPendingLocalSteerMessages(entry);
    entry.activePromptAttemptId = promptAttemptId;
    entry.activeRunId = null;
    entry.pendingCancelPromptAttemptId = null;
    entry.promptCancellationRestoreState = null;
    entry.pendingUserInputRequestIds.clear();
    entry.chatState = ChatState.Streaming;
    entry.sessionLoadError = undefined;
    entry.notifications = [];
    entry.progressMessage = undefined;
    return notify(sessionId, entry);
  };

  const startPromptCancellation: AcpChatSessionActions['startPromptCancellation'] = (
    sessionId,
    promptAttemptId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry || entry.activePromptAttemptId !== promptAttemptId) {
      return undefined;
    }

    entry.promptCancellationRestoreState = {
      activeRunId: entry.activeRunId,
      chatState: entry.chatState,
      pendingUserInputRequestIds: new Set(entry.pendingUserInputRequestIds),
    };
    entry.activePromptAttemptId = null;
    entry.activeRunId = null;
    entry.pendingCancelPromptAttemptId = promptAttemptId;
    entry.pendingUserInputRequestIds.clear();
    discardPendingLocalSteerMessages(entry);
    entry.progressMessage = undefined;
    entry.chatState = ChatState.Idle;
    return notify(sessionId, entry);
  };

  const clearPromptCancellation: AcpChatSessionActions['clearPromptCancellation'] = (
    sessionId,
    promptAttemptId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry || entry.pendingCancelPromptAttemptId !== promptAttemptId) {
      return undefined;
    }

    entry.pendingCancelPromptAttemptId = null;
    entry.promptCancellationRestoreState = null;
    return notify(sessionId, entry);
  };

  const restorePromptCancellation: AcpChatSessionActions['restorePromptCancellation'] = (
    sessionId,
    promptAttemptId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (
      !entry ||
      entry.pendingCancelPromptAttemptId !== promptAttemptId ||
      !entry.promptCancellationRestoreState
    ) {
      return undefined;
    }

    const restoreState = entry.promptCancellationRestoreState;
    entry.activePromptAttemptId = promptAttemptId;
    entry.activeRunId = restoreState.activeRunId;
    entry.pendingCancelPromptAttemptId = null;
    entry.promptCancellationRestoreState = null;
    entry.pendingUserInputRequestIds = new Set(restoreState.pendingUserInputRequestIds);
    entry.chatState = restoreState.chatState;
    return notify(sessionId, entry);
  };

  const waitForPromptCancellation: AcpChatSessionActions['waitForPromptCancellation'] = (
    sessionId,
    promptAttemptId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry || entry.pendingCancelPromptAttemptId !== promptAttemptId) {
      return Promise.resolve();
    }

    return new Promise((resolve) => {
      const unsubscribe = subscribe(sessionId, (snapshot) => {
        if (snapshot.pendingCancelPromptAttemptId !== promptAttemptId) {
          unsubscribe();
          resolve();
        }
      });
    });
  };

  const finishPromptAttemptIfCurrent: AcpChatSessionActions['finishPromptAttemptIfCurrent'] = (
    sessionId,
    promptAttemptId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry || entry.activePromptAttemptId !== promptAttemptId) {
      return false;
    }

    entry.activePromptAttemptId = null;
    entry.activeRunId = null;
    entry.pendingCancelPromptAttemptId = null;
    entry.promptCancellationRestoreState = null;
    entry.pendingUserInputRequestIds.clear();
    discardPendingLocalSteerMessages(entry);
    entry.progressMessage = undefined;
    entry.chatState = ChatState.Idle;
    notify(sessionId, entry);
    return true;
  };

  const clearActivePromptAttempt: AcpChatSessionActions['clearActivePromptAttempt'] = (
    sessionId
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry) {
      return undefined;
    }

    entry.activePromptAttemptId = null;
    entry.activeRunId = null;
    entry.pendingUserInputRequestIds.clear();
    discardPendingLocalSteerMessages(entry);
    entry.chatState = ChatState.Idle;
    return notify(sessionId, entry);
  };

  const isCurrentPromptAttempt: AcpChatSessionActions['isCurrentPromptAttempt'] = (
    sessionId,
    promptAttemptId
  ) => sessionsById.get(sessionId)?.activePromptAttemptId === promptAttemptId;

  const applyAcpSessionNotification: AcpChatSessionActions['applyAcpSessionNotification'] = (
    notification
  ) => {
    const entry = getOrCreateEntry(notification.sessionId);
    if (shouldClearProgressMessage(notification)) {
      entry.progressMessage = undefined;
    }
    const changes = entry.adapter.apply(notification);
    // Session-load replay fast path: messages accumulate inside the adapter
    // and are materialized once in finishSessionLoad. Copying them into the
    // entry and re-notifying per replayed message is O(n^2) over the whole
    // conversation and freezes the renderer on large sessions.
    if (entry.chatState === ChatState.LoadingConversation && entry.lastSnapshot) {
      applyChatStateChanges(
        entry,
        changes.filter((change) => change.type !== 'messages')
      );
      return entry.lastSnapshot;
    }
    applyChatStateChanges(entry, changes);
    return notify(notification.sessionId, entry);
  };

  const applyAcpGooseSessionNotification: AcpChatSessionActions['applyAcpGooseSessionNotification'] =
    (notification) => {
      const entry = getOrCreateEntry(notification.sessionId);
      const changes = entry.adapter.applyGoose(notification);
      // Same session-load replay fast path as applyAcpSessionNotification.
      if (entry.chatState === ChatState.LoadingConversation && entry.lastSnapshot) {
        applyChatStateChanges(
          entry,
          changes.filter((change) => change.type !== 'messages')
        );
        return entry.lastSnapshot;
      }
      applyChatStateChanges(entry, changes);
      return notify(notification.sessionId, entry);
    };

  const applyPermissionRequest: AcpChatSessionActions['applyPermissionRequest'] = (request) => {
    const entry = getOrCreateEntry(request.request.sessionId);
    const changes = entry.adapter.applyPermissionRequest(request);
    applyChatStateChanges(entry, changes);
    entry.pendingUserInputRequestIds.add(
      acpPermissionUserInputRequestId(request.request.toolCall.toolCallId)
    );
    entry.chatState = ChatState.WaitingForUserInput;
    return notify(request.request.sessionId, entry);
  };

  const cancelPermissionRequest: AcpChatSessionActions['cancelPermissionRequest'] = (
    sessionId,
    toolCallId,
    generation
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry) {
      return undefined;
    }

    const changes = entry.adapter.cancelPermissionRequest(toolCallId, generation);
    applyChatStateChanges(entry, changes);
    entry.pendingUserInputRequestIds.delete(acpPermissionUserInputRequestId(toolCallId));
    return notify(sessionId, entry);
  };

  const applyElicitationRequest: AcpChatSessionActions['applyElicitationRequest'] = (request) => {
    const entry = getOrCreateEntry(request.sessionId);
    const changes = entry.adapter.applyElicitationRequest(request);
    applyChatStateChanges(entry, changes);
    entry.pendingUserInputRequestIds.add(acpElicitationUserInputRequestId(request.id));
    entry.chatState = ChatState.WaitingForUserInput;
    return notify(request.sessionId, entry);
  };

  const setElicitationStatus: AcpChatSessionActions['setElicitationStatus'] = (
    sessionId,
    elicitationId,
    status
  ) => {
    const entry = sessionsById.get(sessionId);
    if (!entry) {
      return undefined;
    }

    const changes = entry.adapter.applyElicitationStatus(elicitationId, status);
    if (changes.length === 0) {
      return snapshotFromEntry(entry);
    }

    applyChatStateChanges(entry, changes);
    return notify(sessionId, entry);
  };

  return {
    getSnapshot,
    subscribe,
    deleteSnapshot,
    setSessionMetadata,
    startSessionLoad,
    finishSessionLoad,
    failSessionLoad,
    setSessionLoadError,
    setMessages,
    addPendingLocalSteerMessage,
    setChatState,
    resolveUserInputRequest,
    startPromptAttempt,
    startPromptCancellation,
    clearPromptCancellation,
    restorePromptCancellation,
    waitForPromptCancellation,
    finishPromptAttemptIfCurrent,
    clearActivePromptAttempt,
    isCurrentPromptAttempt,
    applyAcpSessionNotification,
    applyAcpGooseSessionNotification,
    applyPermissionRequest,
    cancelPermissionRequest,
    applyElicitationRequest,
    setElicitationStatus,
  };
}

const acpChatSessionStoreInternal = createAcpChatSessionStoreInternal();

export const acpChatSessionStore: AcpChatSessionStore = storeFromInternal(
  acpChatSessionStoreInternal
);

export const acpChatSessionActions: AcpChatSessionActions = actionsFromStore(
  acpChatSessionStoreInternal
);

interface AcpChatSessionSnapshotState {
  sessionId: string;
  snapshot: AcpChatSessionSnapshot | undefined;
}

export function useAcpChatSessionSnapshot(sessionId: string): AcpChatSessionSnapshot | undefined {
  const [snapshotState, setSnapshotState] = useState<AcpChatSessionSnapshotState>(() => ({
    sessionId,
    snapshot: acpChatSessionStoreInternal.getSnapshot(sessionId),
  }));

  useEffect(() => {
    setSnapshotState({
      sessionId,
      snapshot: acpChatSessionStoreInternal.getSnapshot(sessionId),
    });

    return acpChatSessionStoreInternal.subscribe(sessionId, (snapshot) => {
      setSnapshotState({ sessionId, snapshot });
    });
  }, [sessionId]);

  if (snapshotState.sessionId !== sessionId) {
    return acpChatSessionStoreInternal.getSnapshot(sessionId);
  }

  return snapshotState.snapshot;
}

function storeFromInternal(store: AcpChatSessionStoreInternal): AcpChatSessionStore {
  return {
    getSnapshot: store.getSnapshot,
  };
}

function actionsFromStore(store: AcpChatSessionStoreInternal): AcpChatSessionActions {
  return {
    deleteSnapshot: store.deleteSnapshot,
    applyAcpSessionNotification: store.applyAcpSessionNotification,
    applyAcpGooseSessionNotification: store.applyAcpGooseSessionNotification,
    applyPermissionRequest: store.applyPermissionRequest,
    cancelPermissionRequest: store.cancelPermissionRequest,
    applyElicitationRequest: store.applyElicitationRequest,
    setElicitationStatus: store.setElicitationStatus,
    setSessionMetadata: store.setSessionMetadata,
    startSessionLoad: store.startSessionLoad,
    finishSessionLoad: store.finishSessionLoad,
    failSessionLoad: store.failSessionLoad,
    setSessionLoadError: store.setSessionLoadError,
    setMessages: store.setMessages,
    addPendingLocalSteerMessage: store.addPendingLocalSteerMessage,
    setChatState: store.setChatState,
    resolveUserInputRequest: store.resolveUserInputRequest,
    startPromptAttempt: store.startPromptAttempt,
    startPromptCancellation: store.startPromptCancellation,
    clearPromptCancellation: store.clearPromptCancellation,
    restorePromptCancellation: store.restorePromptCancellation,
    waitForPromptCancellation: store.waitForPromptCancellation,
    finishPromptAttemptIfCurrent: store.finishPromptAttemptIfCurrent,
    clearActivePromptAttempt: store.clearActivePromptAttempt,
    isCurrentPromptAttempt: store.isCurrentPromptAttempt,
  };
}

function applyChatStateChanges(entry: StoreEntry, changes: AcpChatStateChange[]): void {
  for (const change of changes) {
    switch (change.type) {
      case 'messages':
        entry.messages = cloneMessages(change.messages);
        retainPendingLocalSteerMessageIds(entry);
        break;
      case 'tokenState':
        entry.tokenState = { ...entry.tokenState, ...change.tokenState };
        break;
      case 'progressMessage':
        entry.progressMessage = change.message;
        break;
      case 'sessionInfo':
        if (change.name && entry.session) {
          entry.session = { ...entry.session, name: change.name };
        }
        if (change.activeRunId !== undefined) {
          entry.activeRunId = change.activeRunId;
          if (change.activeRunId && !entry.activePromptAttemptId) {
            entry.activePromptAttemptId = `server-run:${change.activeRunId}`;
            entry.chatState = ChatState.Streaming;
          } else if (change.activeRunId === null) {
            if (entry.activePromptAttemptId?.startsWith('server-run:')) {
              entry.activePromptAttemptId = null;
              entry.chatState = ChatState.Idle;
            }
            if (entry.pendingCancelPromptAttemptId?.startsWith('server-run:')) {
              entry.pendingCancelPromptAttemptId = null;
              entry.promptCancellationRestoreState = null;
            }
          }
        }
        break;
      case 'localSteerConfirmed':
        if (!entry.pendingLocalSteerMessageIds.delete(change.messageId)) {
          entry.preConfirmedSteerMessageIds.add(change.messageId);
        }
        break;
      case 'notification':
        entry.notifications = [...entry.notifications, change.notification];
        break;
    }
  }
}

function shouldClearProgressMessage(notification: SessionNotification): boolean {
  switch (notification.update.sessionUpdate) {
    case 'agent_message_chunk':
    case 'agent_thought_chunk':
    case 'tool_call':
      return true;
    default:
      return false;
  }
}

function resetReplayState(entry: StoreEntry): void {
  entry.messages = [];
  entry.tokenState = { ...initialTokenState };
  entry.notifications = [];
  entry.progressMessage = undefined;
  entry.activeRunId = null;
  entry.pendingCancelPromptAttemptId = null;
  entry.promptCancellationRestoreState = null;
  entry.pendingUserInputRequestIds.clear();
  entry.pendingLocalSteerMessageIds.clear();
  entry.preConfirmedSteerMessageIds.clear();
  entry.adapter = createAcpSessionNotificationAdapter();
}

export function acpPermissionUserInputRequestId(toolCallId: string): string {
  return `permission:${toolCallId}`;
}

export function acpElicitationUserInputRequestId(elicitationId: string): string {
  return `elicitation:${elicitationId}`;
}

function retainPendingLocalSteerMessageIds(entry: StoreEntry): void {
  if (entry.pendingLocalSteerMessageIds.size === 0) {
    return;
  }

  const messageIds = new Set(entry.messages.map((message) => message.id).filter(Boolean));
  entry.pendingLocalSteerMessageIds = new Set(
    [...entry.pendingLocalSteerMessageIds].filter((messageId) => messageIds.has(messageId))
  );
}

function discardPendingLocalSteerMessages(entry: StoreEntry): void {
  if (entry.pendingLocalSteerMessageIds.size === 0) {
    return;
  }

  entry.messages = entry.messages.filter(
    (message) => !message.id || !entry.pendingLocalSteerMessageIds.has(message.id)
  );
  entry.pendingLocalSteerMessageIds.clear();
  entry.adapter = createAdapterForEntry(entry);
}

function createAdapterForEntry(entry: StoreEntry): AcpSessionNotificationAdapter {
  return createAcpSessionNotificationAdapter(
    entry.messages,
    confirmedLocalSteerTextByMessageId(entry)
  );
}

function confirmedLocalSteerTextByMessageId(entry: StoreEntry): Map<string, string> {
  const textByMessageId = new Map<string, string>();

  for (const message of entry.messages) {
    if (
      !message.id ||
      !message.metadata.steer ||
      entry.pendingLocalSteerMessageIds.has(message.id)
    ) {
      continue;
    }

    const firstContent = message.content[0];
    if (firstContent?.type === 'text') {
      textByMessageId.set(message.id, firstContent.text);
    }
  }

  return textByMessageId;
}

function snapshotFromEntry(entry: StoreEntry): AcpChatSessionSnapshot {
  return {
    session: entry.session,
    messages: cloneMessages(entry.messages),
    tokenState: { ...entry.tokenState },
    notifications: [...entry.notifications],
    progressMessage: entry.progressMessage,
    chatState: entry.chatState,
    sessionLoadError: entry.sessionLoadError,
    activePromptAttemptId: entry.activePromptAttemptId,
    activeRunId: entry.activeRunId,
    pendingCancelPromptAttemptId: entry.pendingCancelPromptAttemptId,
  };
}

function cloneMessages(messages: Message[]): Message[] {
  return messages.map(cloneMessage);
}

import { useCallback, useEffect, useMemo, useRef } from 'react';
import { defineMessages, useIntl } from '../i18n';
import { AppEvents } from '../constants/events';
import { toastError } from '../toasts';
import { ChatState } from '../types/chatState';

import type { TokenState } from '../types/chat';
import type { Session } from '../types/session';

import {
  createUserMessage,
  type ImageData,
  type Message,
  type NotificationEvent,
  type UserInput,
} from '../types/message';
import { errorMessage } from '../utils/conversionUtils';
import type { UseChatSessionParams, UseChatSessionResult } from './useChatSessionTypes';
import { resolveAcpElicitationRequest } from '../acp/elicitationRequests';
import { acpChatSessionController } from '../acp/chatSessionController';
import {
  acpChatSessionActions,
  acpChatSessionStore,
  useAcpChatSessionSnapshot,
} from '../acp/chatSessionStore';
import { acpSteerSession } from '../acp/prompt';
import { isAcpRecovering } from '../acp/acpConnection';

const initialTokenState: TokenState = {
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  accumulatedInputTokens: 0,
  accumulatedOutputTokens: 0,
  accumulatedTotalTokens: 0,
};

function isClearCommand(message: string): boolean {
  return message.trim() === '/clear';
}

function isSlashCommand(message: string): boolean {
  return message.trim().startsWith('/');
}

const i18n = defineMessages({
  notificationTitle: {
    id: 'chat.notification.taskComplete.title',
    defaultMessage: 'Goose finished the task.',
  },
  notificationBody: {
    id: 'chat.notification.taskComplete.body',
    defaultMessage: 'Click here to bring Goose back into focus.',
  },
});

export function useChatSession({
  sessionId,
  onStreamFinish,
  onSessionLoaded,
}: UseChatSessionParams): UseChatSessionResult {
  const intl = useIntl();
  const acpSnapshot = useAcpChatSessionSnapshot(sessionId);
  const messages = acpSnapshot?.messages ?? [];
  const session = acpSnapshot?.session;
  const chatState = acpSnapshot?.chatState ?? ChatState.LoadingConversation;
  const progressMessage = acpSnapshot?.progressMessage;
  const sessionLoadError = acpSnapshot?.sessionLoadError;
  const tokenState = acpSnapshot?.tokenState ?? initialTokenState;
  const queueProcessingBlocked = acpSnapshot?.pendingCancelPromptAttemptId != null;
  const hasActiveRun = acpSnapshot?.activeRunId != null;

  const snapshotRef = useRef(acpSnapshot);
  snapshotRef.current = acpSnapshot;

  const getCurrentSnapshot = useCallback(
    () => snapshotRef.current ?? acpChatSessionStore.getSnapshot(sessionId),
    [sessionId]
  );

  useEffect(() => {
    const handleSessionRenamed = (event: Event) => {
      const {
        sessionId: renamedSessionId,
        newName,
        userInitiated,
      } = (event as CustomEvent<{ sessionId: string; newName: string; userInitiated?: boolean }>)
        .detail;

      if (renamedSessionId !== sessionId) {
        return;
      }

      const currentSession = getCurrentSnapshot()?.session;
      if (!currentSession || (currentSession.name === newName && !userInitiated)) {
        return;
      }

      const updatedSession = {
        ...currentSession,
        name: newName,
        ...(userInitiated && { user_set_name: true }),
      };
      acpChatSessionActions.setSessionMetadata(sessionId, updatedSession);
    };

    window.addEventListener(AppEvents.SESSION_RENAMED, handleSessionRenamed);
    return () => window.removeEventListener(AppEvents.SESSION_RENAMED, handleSessionRenamed);
  }, [getCurrentSnapshot, sessionId]);

  const onFinish = useCallback(
    async (error?: string): Promise<void> => {
      if (error) {
        toastError({ title: "Couldn't send message", msg: error });
      } else {
        try {
          const [notificationsEnabled, anyWindowFocused] = await Promise.all([
            window.electron.getSetting('enableNotifications'),
            window.electron.isAnyWindowFocused(),
          ]);
          if (notificationsEnabled === true && !anyWindowFocused) {
            window.electron.showNotification({
              title: intl.formatMessage(i18n.notificationTitle),
              body: intl.formatMessage(i18n.notificationBody),
            });
          }
        } catch (notifyError) {
          console.warn('Failed to show task completion notification:', notifyError);
        }
      }

      onStreamFinish();
    },
    [intl, onStreamFinish]
  );

  const submitToAcpSession = useCallback(
    async (targetSessionId: string, userMessage: Message) => {
      await acpChatSessionController.submitMessage(targetSessionId, userMessage, {
        getCurrentSnapshot: () =>
          targetSessionId === sessionId
            ? getCurrentSnapshot()
            : acpChatSessionStore.getSnapshot(targetSessionId),
        onFinish,
      });
    },
    [getCurrentSnapshot, onFinish, sessionId]
  );

  const retrySessionLoad = useCallback(
    () => acpChatSessionController.loadSession(sessionId, { onSessionLoaded }),
    [sessionId, onSessionLoaded]
  );

  // Load session on mount or sessionId change
  useEffect(() => {
    void retrySessionLoad();
  }, [retrySessionLoad]);

  const steerMessage = useCallback(
    async (input: UserInput): Promise<boolean> => {
      const { msg: userMessage, images } = input;
      const hasTextContent = userMessage.trim().length > 0;
      const hasNewMessage = hasTextContent || images.length > 0;
      if (!hasNewMessage) {
        return false;
      }

      // ACP confirms picked-up steers with user text chunks; image-only steers cannot confirm pickup.
      if (!hasTextContent) {
        return false;
      }

      if (isSlashCommand(userMessage)) {
        return false;
      }

      const activeRunId =
        acpChatSessionStore.getSnapshot(sessionId)?.activeRunId ??
        getCurrentSnapshot()?.activeRunId;
      if (!activeRunId) {
        return false;
      }

      try {
        const steeredMessage = createUserMessage(userMessage, images);
        const response = await acpSteerSession(sessionId, steeredMessage, activeRunId);
        const localSteerMessage: Message = {
          ...steeredMessage,
          id: response.messageId,
          metadata: { ...steeredMessage.metadata, steer: true },
        };
        const latestSnapshot = acpChatSessionStore.getSnapshot(sessionId) ?? getCurrentSnapshot();
        if (latestSnapshot?.activeRunId !== activeRunId) {
          return false;
        }

        const currentMessages = latestSnapshot.messages;

        if (!currentMessages.some((message) => message.id === response.messageId)) {
          acpChatSessionActions.addPendingLocalSteerMessage(sessionId, localSteerMessage);
        }

        return true;
      } catch (error) {
        console.warn('Failed to steer ACP session:', error);
        return false;
      }
    },
    [getCurrentSnapshot, sessionId]
  );

  const handleSubmit = useCallback(
    async (input: UserInput) => {
      if (isAcpRecovering()) {
        return;
      }

      const { msg: userMessage, images } = input;
      const currentSnapshot = getCurrentSnapshot();

      if (
        !currentSnapshot?.session ||
        currentSnapshot.chatState === ChatState.LoadingConversation ||
        currentSnapshot.pendingCancelPromptAttemptId !== null
      ) {
        return;
      }

      const activeRunId =
        acpChatSessionStore.getSnapshot(sessionId)?.activeRunId ?? currentSnapshot.activeRunId;
      if (activeRunId) {
        await steerMessage(input);
        return;
      }

      if (
        currentSnapshot.chatState === ChatState.Streaming ||
        currentSnapshot.chatState === ChatState.Thinking ||
        currentSnapshot.chatState === ChatState.Compacting
      ) {
        return;
      }

      const currentMessages = currentSnapshot.messages;
      const hasExistingMessages = currentMessages.length > 0;
      const hasNewMessage = userMessage.trim().length > 0 || images.length > 0;
      const clearsConversation = hasNewMessage && isClearCommand(userMessage);

      if (!hasNewMessage && !hasExistingMessages) {
        return;
      }

      // Emit session-created event for first message in a new session
      if (!hasExistingMessages && hasNewMessage) {
        window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
      }

      const newMessage = hasNewMessage
        ? createUserMessage(userMessage, images)
        : currentMessages[currentMessages.length - 1];
      const messagesForStore = clearsConversation
        ? []
        : hasNewMessage
          ? [...currentMessages, newMessage]
          : [...currentMessages];

      if (clearsConversation || hasNewMessage) {
        acpChatSessionActions.setMessages(sessionId, messagesForStore);
      }

      await submitToAcpSession(sessionId, newMessage);
    },
    [getCurrentSnapshot, sessionId, steerMessage, submitToAcpSession]
  );

  const submitElicitationResponse = useCallback(
    async (elicitationId: string, userData: Record<string, unknown>) => {
      const currentSnapshot = getCurrentSnapshot();

      if (
        !currentSnapshot?.session ||
        currentSnapshot.chatState === ChatState.LoadingConversation
      ) {
        return false;
      }

      if (!resolveAcpElicitationRequest(sessionId, elicitationId, userData)) {
        console.error('No pending ACP elicitation request found', { sessionId, elicitationId });
        return false;
      }

      return true;
    },
    [getCurrentSnapshot, sessionId]
  );

  const stopStreaming = useCallback(() => {
    acpChatSessionController.stop(sessionId);
  }, [sessionId]);

  const onMessageUpdate = useCallback(
    async (
      messageId: string,
      newContent: string,
      editType: 'fork' | 'edit',
      retainedImages: ImageData[]
    ) => {
      try {
        await acpChatSessionController.updateMessage(
          sessionId,
          messageId,
          newContent,
          editType,
          retainedImages,
          {
            getCurrentSnapshot,
            onFinish,
          }
        );
      } catch (error) {
        const errorMsg = errorMessage(error);
        console.error('Failed to edit message:', error);
        toastError({
          title: 'Failed to edit message',
          msg: errorMsg,
        });
      }
    },
    [getCurrentSnapshot, onFinish, sessionId]
  );

  const updateSession = useCallback(
    (updater: (session: Session) => Session) => {
      const currentSession = getCurrentSnapshot()?.session;
      if (!currentSession) return;

      const nextSession = updater(currentSession);
      acpChatSessionActions.setSessionMetadata(sessionId, nextSession);
    },
    [getCurrentSnapshot, sessionId]
  );
  const notificationsMap = useMemo(() => {
    return (acpSnapshot?.notifications ?? []).reduce((map, notification) => {
      const key = notification.request_id;
      if (!map.has(key)) {
        map.set(key, []);
      }
      map.get(key)!.push(notification);
      return map;
    }, new Map<string, NotificationEvent[]>());
  }, [acpSnapshot?.notifications]);

  return {
    sessionLoadError,
    messages,
    session,
    chatState,
    progressMessage,
    updateSession,
    handleSubmit,
    onSteerQueuedMessage: steerMessage,
    submitElicitationResponse,
    stopStreaming,
    retrySessionLoad,
    tokenState,
    notifications: notificationsMap,
    pauseQueueOnStop: false,
    queueProcessingBlocked,
    hasActiveRun,
    onMessageUpdate,
  };
}

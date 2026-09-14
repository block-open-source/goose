import { Fragment, memo, useEffect, useMemo, useRef, useState } from 'react';
import { isEqual } from 'lodash';
import { defineMessages, useIntl } from '../i18n';
import GooseMessage from './GooseMessage';
import UserMessage from './UserMessage';
import {
  SystemNotificationInline,
  getInlineSystemNotification,
} from './context_management/SystemNotificationInline';
import {
  CreditsExhaustedNotification,
  getCreditsExhaustedNotification,
} from './context_management/CreditsExhaustedNotification';
import type {
  ImageData,
  Message,
  NotificationEvent,
  SystemNotificationContent,
} from '../types/message';
import LoadingGoose from './LoadingGoose';
import { getModelDisplayName } from './settings/models/predefinedModelsUtils';
import { deriveMessageRowContexts, type MessageRowContext } from './messageRowContext';

const i18n = defineMessages({
  loadingMessages: {
    id: 'progressiveMessageList.loadingMessages',
    defaultMessage: 'Loading messages... ({renderedCount}/{totalCount})',
  },
  searchHint: {
    id: 'progressiveMessageList.searchHint',
    defaultMessage: 'Press Cmd/Ctrl+F to load all messages immediately for search',
  },
  modelChanged: {
    id: 'progressiveMessageList.modelChanged',
    defaultMessage: 'Model changed: {previousModel} → {currentModel}',
  },
});

const emptyToolCallNotifications = new Map<string, NotificationEvent[]>();
const emptyAppend = () => {};

function getResolvedModel(message: Message): string | null {
  if (message.role !== 'assistant' || !message.metadata.userVisible) return null;
  return message.metadata.inference?.resolvedModel ?? null;
}

function getSystemNotification(message: Message): SystemNotificationContent | undefined {
  return getCreditsExhaustedNotification(message) ?? getInlineSystemNotification(message);
}

function renderSystemNotification(notification: SystemNotificationContent) {
  switch (notification.notificationType) {
    case 'creditsExhausted':
      return <CreditsExhaustedNotification notification={notification} />;
    case 'inlineMessage':
      return <SystemNotificationInline notification={notification} />;
    default:
      return null;
  }
}

interface MessageRowProps {
  append: (value: string) => void;
  index: number;
  isStreaming: boolean;
  isUser: boolean;
  message: Message;
  modelChangeMessage: string | null;
  onMessageUpdate?: (
    messageId: string,
    newContent: string,
    editType: 'fork' | 'edit',
    retainedImages: ImageData[]
  ) => void;
  rowContext: MessageRowContext;
  sessionId: string;
  submitElicitationResponse?: (
    elicitationId: string,
    userData: Record<string, unknown>
  ) => Promise<boolean>;
  toolNotifications: readonly (NotificationEvent[] | undefined)[];
}

function MessageRowComponent({
  append,
  index,
  isStreaming,
  isUser,
  message,
  modelChangeMessage,
  onMessageUpdate,
  rowContext,
  sessionId,
  submitElicitationResponse,
  toolNotifications,
}: MessageRowProps) {
  const notification = getSystemNotification(message);

  if (notification) {
    return (
      <div
        className={`relative ${index === 0 ? 'mt-0' : 'mt-4'} assistant`}
        data-testid="message-container"
      >
        {renderSystemNotification(notification)}
      </div>
    );
  }

  const hasOnlyToolResponses = message.content.every((content) => content.type === 'toolResponse');

  return (
    <Fragment>
      {modelChangeMessage && (
        <SystemNotificationInline
          notification={{
            msg: modelChangeMessage,
            notificationType: 'inlineMessage',
          }}
        />
      )}
      <div
        className={`relative ${index === 0 ? 'mt-0' : 'mt-4'} ${isUser ? 'user' : 'assistant'} ${rowContext.isInToolCallChain ? 'in-chain' : ''}`}
        data-testid="message-container"
      >
        {isUser ? (
          !hasOnlyToolResponses && (
            <UserMessage message={message} onMessageUpdate={onMessageUpdate} />
          )
        ) : (
          <GooseMessage
            sessionId={sessionId}
            message={message}
            hideTimestamp={rowContext.hideTimestamp}
            toolStates={rowContext.toolStates}
            toolNotifications={toolNotifications}
            toolConfirmationShownInline={rowContext.toolConfirmationShownInline}
            append={append}
            isStreaming={isStreaming}
            submitElicitationResponse={submitElicitationResponse}
          />
        )}
      </div>
    </Fragment>
  );
}

const MessageRow = memo(MessageRowComponent, isEqual);

interface ProgressiveMessageListProps {
  messages: Message[];
  sessionId: string;
  toolCallNotifications?: Map<string, NotificationEvent[]>;
  append?: (value: string) => void;
  isUserMessage: (message: Message) => boolean;
  batchSize?: number;
  batchDelay?: number;
  showLoadingThreshold?: number;
  renderMessage?: (message: Message, index: number) => React.ReactNode | null;
  isStreamingMessage?: boolean;
  onMessageUpdate?: (
    messageId: string,
    newContent: string,
    editType: 'fork' | 'edit',
    retainedImages: ImageData[]
  ) => void;
  onRenderingComplete?: () => void;
  submitElicitationResponse?: (
    elicitationId: string,
    userData: Record<string, unknown>
  ) => Promise<boolean>;
}

export default function ProgressiveMessageList({
  messages,
  sessionId,
  toolCallNotifications = emptyToolCallNotifications,
  append = emptyAppend,
  isUserMessage,
  batchSize = 5,
  batchDelay = 20,
  showLoadingThreshold = 50,
  renderMessage,
  isStreamingMessage = false,
  onMessageUpdate,
  onRenderingComplete,
  submitElicitationResponse,
}: ProgressiveMessageListProps) {
  const intl = useIntl();
  const [renderedCount, setRenderedCount] = useState(() =>
    messages.length <= showLoadingThreshold ? messages.length : Math.min(batchSize, messages.length)
  );
  const completedMessageKeyRef = useRef<string | null>(null);
  const isLoading = renderedCount < messages.length;

  useEffect(() => {
    if (messages.length <= showLoadingThreshold) {
      setRenderedCount(messages.length);
      return;
    }

    if (!isLoading) return;

    const timeout = window.setTimeout(() => {
      setRenderedCount((current) => Math.min(current + batchSize, messages.length));
    }, batchDelay);

    return () => window.clearTimeout(timeout);
  }, [batchDelay, batchSize, isLoading, messages.length, renderedCount, showLoadingThreshold]);

  useEffect(() => {
    if (isLoading) return;

    const completedMessageKey = `${sessionId}:${messages.length}`;
    if (completedMessageKeyRef.current === completedMessageKey) return;

    const timeout = window.setTimeout(() => {
      completedMessageKeyRef.current = completedMessageKey;
      onRenderingComplete?.();
    }, 50);

    return () => window.clearTimeout(timeout);
  }, [isLoading, messages.length, onRenderingComplete, sessionId]);

  useEffect(() => {
    if (!isLoading) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      const isMac = window.electron.platform === 'darwin';
      const isSearchShortcut = (isMac ? event.metaKey : event.ctrlKey) && event.key === 'f';

      if (isSearchShortcut) {
        setRenderedCount(messages.length);
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isLoading, messages.length]);

  const rowContexts = useMemo(() => deriveMessageRowContexts(messages), [messages]);
  const messagesToRender = messages.slice(0, renderedCount);
  const messageRows = messagesToRender
    .map((message, index) => {
      if (!message.metadata.userVisible) return null;
      if (renderMessage) return renderMessage(message, index);

      const isUser = isUserMessage(message);
      const messageIdentifier = message.id ?? `msg-${index}-${message.created}`;
      const messageKey = getSystemNotification(message)
        ? `notification-${messageIdentifier}`
        : messageIdentifier;
      const rowContext = rowContexts[index];
      const currentResolvedModel = getResolvedModel(message);
      const modelChangeMessage =
        currentResolvedModel &&
        rowContext.previousResolvedModel &&
        currentResolvedModel !== rowContext.previousResolvedModel
          ? intl.formatMessage(i18n.modelChanged, {
              previousModel: getModelDisplayName(rowContext.previousResolvedModel),
              currentModel: getModelDisplayName(currentResolvedModel),
            })
          : null;
      const toolNotifications = rowContext.toolStates.map((toolState) =>
        toolCallNotifications.get(toolState.requestId)
      );

      return (
        <MessageRow
          key={messageKey}
          append={append}
          index={index}
          isStreaming={
            isStreamingMessage &&
            !isUser &&
            index === messagesToRender.length - 1 &&
            message.role === 'assistant'
          }
          isUser={isUser}
          message={message}
          modelChangeMessage={modelChangeMessage}
          onMessageUpdate={onMessageUpdate}
          rowContext={rowContext}
          sessionId={sessionId}
          submitElicitationResponse={submitElicitationResponse}
          toolNotifications={toolNotifications}
        />
      );
    })
    .filter(Boolean);

  return (
    <>
      {messageRows}

      {isLoading && (
        <div className="flex flex-col items-center justify-center py-8">
          <LoadingGoose
            message={intl.formatMessage(i18n.loadingMessages, {
              renderedCount,
              totalCount: messages.length,
            })}
          />
          <div className="text-xs text-text-secondary mt-2">
            {intl.formatMessage(i18n.searchHint)}
          </div>
        </div>
      )}
    </>
  );
}

import React, { useRef, useState } from 'react';
import { X, Clock, Send, GripVertical, Zap, Sparkles, ChevronDown, ChevronUp } from 'lucide-react';
import { Button } from './ui/button';
import { ImageData } from '../types/message';
import { getTextDirection } from '../utils/textDirection';
import { defineMessages, useIntl } from '../i18n';

const i18n = defineMessages({
  paused: {
    id: 'messageQueue.paused',
    defaultMessage: 'Paused',
  },
  next: {
    id: 'messageQueue.next',
    defaultMessage: 'Next',
  },
  sendNow: {
    id: 'messageQueue.sendNow',
    defaultMessage: 'Send this message now',
  },
  expandQueue: {
    id: 'messageQueue.expandQueue',
    defaultMessage: 'Expand queue',
  },
  queuePausedCompact: {
    id: 'messageQueue.queuePausedCompact',
    defaultMessage: 'Queue paused - click "Send" or add new message to resume',
  },
  queuePaused: {
    id: 'messageQueue.queuePaused',
    defaultMessage: 'Queue Paused',
  },
  messageQueue: {
    id: 'messageQueue.messageQueue',
    defaultMessage: 'Message Queue',
  },
  messageCount: {
    id: 'messageQueue.messageCount',
    defaultMessage: '{count, plural, one {# message} other {# messages}} {status}',
  },
  waiting: {
    id: 'messageQueue.waiting',
    defaultMessage: 'waiting',
  },
  queued: {
    id: 'messageQueue.queued',
    defaultMessage: 'queued',
  },
  clearAll: {
    id: 'messageQueue.clearAll',
    defaultMessage: 'Clear All',
  },
  collapseQueue: {
    id: 'messageQueue.collapseQueue',
    defaultMessage: 'Collapse queue',
  },
  queuePausedExpanded: {
    id: 'messageQueue.queuePausedExpanded',
    defaultMessage: 'Queue paused by interruption. Use "Send Now" or add a new message to resume.',
  },
  save: {
    id: 'messageQueue.save',
    defaultMessage: 'Save',
  },
  cancel: {
    id: 'messageQueue.cancel',
    defaultMessage: 'Cancel',
  },
  clickToEdit: {
    id: 'messageQueue.clickToEdit',
    defaultMessage: '{content} (Click to edit)',
  },
  cannotSendWhileEditing: {
    id: 'messageQueue.cannotSendWhileEditing',
    defaultMessage: 'Cannot send while editing',
  },
  stopAndSend: {
    id: 'messageQueue.stopAndSend',
    defaultMessage: 'Stop current processing and send this message now',
  },
  removeFromQueue: {
    id: 'messageQueue.removeFromQueue',
    defaultMessage: 'Remove this message from queue',
  },
  dragToReorder: {
    id: 'messageQueue.dragToReorder',
    defaultMessage: 'Drag messages to reorder priority',
  },
});

export interface QueuedMessage {
  id: string;
  content: string;
  timestamp: number;
  images: ImageData[];
}

interface MessageQueueProps {
  queuedMessages: QueuedMessage[];
  onRemoveMessage: (id: string) => void;
  onClearQueue: () => void;
  onStopAndSend?: (messageId: string) => void;
  onEditMessage?: (messageId: string, newContent: string) => void;
  onTriggerQueueProcessing?: () => void;
  editingMessageIdRef?: React.MutableRefObject<string | null>;
  onReorderMessages?: (reorderedMessages: QueuedMessage[]) => void;
  sendingMessageIds?: ReadonlySet<string>;
  className?: string;
  isPaused?: boolean;
}

export const MessageQueue: React.FC<MessageQueueProps> = ({
  queuedMessages,
  onRemoveMessage,
  onClearQueue,
  onStopAndSend,
  onEditMessage,
  onTriggerQueueProcessing,
  editingMessageIdRef,
  onReorderMessages,
  sendingMessageIds,
  className = '',
  isPaused = false,
}) => {
  const intl = useIntl();
  const [isExpanded, setIsExpanded] = useState(true);
  const [draggedItem, setDraggedItem] = useState<string | null>(null);
  const [dragOverItem, setDragOverItem] = useState<string | null>(null);
  const [hoveredMessage, setHoveredMessage] = useState<string | null>(null);
  const [editingMessage, setEditingMessage] = useState<string | null>(null);
  const [editContent, setEditContent] = useState<string>('');
  const onTriggerQueueProcessingRef = useRef(onTriggerQueueProcessing);
  onTriggerQueueProcessingRef.current = onTriggerQueueProcessing;
  const isSendingMessage = (messageId: string) => sendingMessageIds?.has(messageId) ?? false;

  if (queuedMessages.length === 0) {
    return null;
  }

  const handleDragStart = (e: React.DragEvent, messageId: string) => {
    if (isSendingMessage(messageId)) {
      e.preventDefault();
      return;
    }

    setDraggedItem(messageId);
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/html', messageId);
  };

  const handleDragOver = (e: React.DragEvent, messageId: string) => {
    if (isSendingMessage(messageId)) {
      return;
    }

    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    setDragOverItem(messageId);
  };

  const handleDragLeave = () => {
    setDragOverItem(null);
  };

  const handleDrop = (e: React.DragEvent, targetMessageId: string) => {
    e.preventDefault();

    if (!draggedItem || !onReorderMessages || isSendingMessage(targetMessageId)) return;

    const draggedIndex = queuedMessages.findIndex((msg) => msg.id === draggedItem);
    const targetIndex = queuedMessages.findIndex((msg) => msg.id === targetMessageId);

    if (draggedIndex === -1 || targetIndex === -1 || draggedIndex === targetIndex) {
      setDraggedItem(null);
      setDragOverItem(null);
      return;
    }

    const newMessages = [...queuedMessages];
    const [removed] = newMessages.splice(draggedIndex, 1);
    newMessages.splice(targetIndex, 0, removed);

    onReorderMessages(newMessages);
    setDraggedItem(null);
    setDragOverItem(null);
  };

  const handleDragEnd = () => {
    setDraggedItem(null);
    setDragOverItem(null);
  };

  const formatTimestamp = (timestamp: number) => {
    const now = Date.now();
    const diff = now - timestamp;
    if (diff < 60000) return 'now';
    if (diff < 3600000) return `${Math.floor(diff / 60000)}m`;
    return `${Math.floor(diff / 3600000)}h`;
  };

  const nextMessage = queuedMessages[0];
  const remainingCount = queuedMessages.length - 1;
  const nextMessageIsSending = isSendingMessage(nextMessage.id);
  const hasSendingMessages = queuedMessages.some((message) => isSendingMessage(message.id));

  // Compact View
  if (!isExpanded) {
    return (
      <div className={`relative ${className}`}>
        {/* Compact Header */}
        <div
          className="flex items-center justify-between px-4 py-2.5 bg-background border-b border-border/20 cursor-pointer hover:bg-muted/30 transition-all duration-200"
          onClick={() => setIsExpanded(true)}
        >
          <div className="flex items-center gap-3 flex-1 min-w-0">
            <div className="flex items-center gap-2">
              {isPaused ? (
                <div className="w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
              ) : (
                <div className="w-2 h-2 rounded-full bg-blue-500 animate-pulse" />
              )}
              <span className="text-sm font-medium text-foreground">
                {isPaused ? intl.formatMessage(i18n.paused) : intl.formatMessage(i18n.next)}
              </span>
            </div>

            {/* Next message preview */}
            <div className="flex-1 min-w-0">
              <p
                dir={getTextDirection(nextMessage.content) ?? undefined}
                className="text-sm text-muted-foreground truncate"
                title={nextMessage.content}
              >
                {nextMessage.content.length > 40
                  ? `${nextMessage.content.substring(0, 40)}...`
                  : nextMessage.content}
              </p>
            </div>

            {/* Queue count */}
            {remainingCount > 0 && (
              <div className="flex items-center gap-1 text-xs text-muted-foreground bg-background-secondary border border-border-primary px-2 py-1 rounded-full font-medium">
                <span>+{remainingCount}</span>
              </div>
            )}
          </div>

          <div className="flex items-center gap-2">
            {/* Quick Send Now button */}
            {onStopAndSend && (
              <Button
                variant="ghost"
                size="sm"
                onClick={(e) => {
                  e.stopPropagation();
                  if (nextMessageIsSending) return;
                  onStopAndSend(nextMessage.id);
                }}
                disabled={nextMessageIsSending}
                className="h-7 px-2 text-xs text-info hover:text-info/80 hover:bg-info/10"
                title={intl.formatMessage(i18n.sendNow)}
              >
                <Send className="w-3 h-3" />
              </Button>
            )}

            {/* Expand button */}
            <Button
              variant="ghost"
              size="sm"
              className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
              title={intl.formatMessage(i18n.expandQueue)}
            >
              <ChevronDown className="w-4 h-4" />
            </Button>
          </div>
        </div>

        {/* Paused state indicator */}
        {isPaused && (
          <div className="px-4 py-1.5 bg-amber-50/60 dark:bg-amber-900/20 border-b border-amber-200/30 dark:border-amber-800/30">
            <div className="flex items-center gap-2 text-xs text-amber-700 dark:text-amber-300">
              <Zap className="w-3 h-3" />
              <span>{intl.formatMessage(i18n.queuePausedCompact)}</span>
            </div>
          </div>
        )}
      </div>
    );
  }

  // Expanded View
  return (
    <div className={`relative ${className}`}>
      {/* Expanded Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-background border-b border-border/30">
        <div className="flex items-center gap-3">
          <div className="relative">
            {isPaused ? (
              <div className="flex items-center gap-2">
                <div className="w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
                <Clock className="w-4 h-4 text-amber-600 dark:text-amber-400" />
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <div className="w-2 h-2 rounded-full bg-blue-500 animate-pulse" />
                <Sparkles className="w-4 h-4 text-info" />
              </div>
            )}
          </div>
          <div className="flex flex-col">
            <span className="text-sm font-medium text-foreground">
              {isPaused
                ? intl.formatMessage(i18n.queuePaused)
                : intl.formatMessage(i18n.messageQueue)}
            </span>
            <span className="text-xs text-muted-foreground">
              {intl.formatMessage(i18n.messageCount, {
                count: queuedMessages.length,
                status: isPaused
                  ? intl.formatMessage(i18n.waiting)
                  : intl.formatMessage(i18n.queued),
              })}
            </span>
          </div>
        </div>

        <div className="flex items-center gap-2">
          {queuedMessages.length > 1 && (
            <Button
              variant="ghost"
              size="sm"
              onClick={onClearQueue}
              disabled={hasSendingMessages}
              className="text-xs h-7 px-3 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
            >
              {intl.formatMessage(i18n.clearAll)}
            </Button>
          )}

          {/* Collapse button */}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setIsExpanded(false)}
            className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
            title={intl.formatMessage(i18n.collapseQueue)}
          >
            <ChevronUp className="w-4 h-4" />
          </Button>
        </div>
      </div>

      {/* Status Banner for Paused State */}
      {isPaused && (
        <div className="px-4 py-2 bg-amber-50/80 dark:bg-amber-900/20 border-b border-amber-200/50 dark:border-amber-800/50">
          <div className="flex items-center gap-2 text-sm text-amber-800 dark:text-amber-200">
            <Zap className="w-4 h-4" />
            <span>{intl.formatMessage(i18n.queuePausedExpanded)}</span>
          </div>
        </div>
      )}

      {/* Message Bubbles */}
      <div className="p-4 space-y-3 bg-background max-h-80 overflow-y-auto">
        {queuedMessages.map((message, index) => {
          const isSending = isSendingMessage(message.id);
          const isEditing = editingMessage === message.id;
          return (
            <div
              key={message.id}
              className="group relative"
              draggable={Boolean(onReorderMessages && !isSending)}
              onDragStart={(e) => handleDragStart(e, message.id)}
              onDragOver={(e) => handleDragOver(e, message.id)}
              onDragLeave={handleDragLeave}
              onDrop={(e) => handleDrop(e, message.id)}
              onDragEnd={handleDragEnd}
              onMouseEnter={() => setHoveredMessage(message.id)}
              onMouseLeave={() => setHoveredMessage(null)}
            >
              {/* Main message bubble */}
              <div
                className={`relative flex items-center gap-3 rounded-xl px-4 py-3 border transition-all duration-300 ease-out ${
                  draggedItem === message.id
                    ? 'bg-info/20 border-info opacity-60 scale-105 shadow-lg rotate-2'
                    : dragOverItem === message.id
                      ? 'bg-green-100/80 border-green-400 shadow-lg dark:bg-green-950/50 dark:border-green-600 scale-102'
                      : hoveredMessage === message.id
                        ? 'bg-muted/90 border-border shadow-md scale-101'
                        : 'bg-muted/60 hover:bg-muted/80 border-border/60 hover:border-border dark:border-border/60 dark:hover:border-border'
                } ${isSending ? 'opacity-60' : ''} backdrop-blur-sm`}
              >
                {/* Priority indicator */}
                <div className="flex items-center gap-2">
                  <div
                    className={`flex items-center justify-center w-6 h-6 rounded-full text-xs font-semibold transition-colors ${
                      index === 0
                        ? 'bg-blue-500 text-white shadow-md'
                        : 'bg-muted text-muted-foreground'
                    }`}
                  >
                    {index + 1}
                  </div>

                  {/* Drag handle */}
                  {onReorderMessages && !isSending && (
                    <div
                      className={`opacity-0 group-hover:opacity-60 hover:opacity-100 transition-all duration-200 cursor-grab active:cursor-grabbing ${
                        hoveredMessage === message.id ? 'opacity-40' : ''
                      }`}
                    >
                      <GripVertical className="w-4 h-4 text-muted-foreground hover:text-foreground" />
                    </div>
                  )}
                </div>

                {/* Message content */}
                <div className="flex-1 min-w-0">
                  {isEditing ? (
                    <div className="space-y-2">
                      <textarea
                        value={editContent}
                        onChange={(e) => setEditContent(e.target.value)}
                        disabled={isSending}
                        dir={getTextDirection(editContent) ?? undefined}
                        className="w-full text-sm bg-background border border-border rounded-md px-2 py-1 resize-none focus:outline-none focus:ring-2 focus:ring-blue-500/20 focus:border-blue-500"
                        rows={Math.min(Math.ceil(editContent.length / 60), 4)}
                        autoFocus
                      />
                      <div className="flex gap-2">
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={isSending}
                          onClick={() => {
                            if (isSending) return;
                            if (onEditMessage) {
                              onEditMessage(message.id, editContent);
                            }
                            setEditingMessage(null);
                            if (editingMessageIdRef) editingMessageIdRef.current = null;
                            // Trigger queue processing if system is ready
                            if (onTriggerQueueProcessing) {
                              setTimeout(() => onTriggerQueueProcessingRef.current?.(), 100);
                            }
                            setEditContent('');
                          }}
                          className="h-6 px-2 text-xs"
                        >
                          {intl.formatMessage(i18n.save)}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => {
                            setEditingMessage(null);
                            if (editingMessageIdRef) editingMessageIdRef.current = null;
                            // Trigger queue processing if system is ready
                            if (onTriggerQueueProcessing) {
                              setTimeout(() => onTriggerQueueProcessingRef.current?.(), 100);
                            }
                            setEditContent('');
                          }}
                          className="h-6 px-2 text-xs"
                        >
                          {intl.formatMessage(i18n.cancel)}
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <p
                      dir={getTextDirection(message.content) ?? undefined}
                      className={`text-sm text-foreground leading-relaxed rounded px-1 py-0.5 transition-colors ${
                        isSending ? 'cursor-not-allowed' : 'cursor-pointer hover:bg-muted/30'
                      }`}
                      title={intl.formatMessage(i18n.clickToEdit, { content: message.content })}
                      onClick={() => {
                        if (isSending) return;
                        setEditingMessage(message.id);
                        if (editingMessageIdRef) editingMessageIdRef.current = message.id;
                        setEditContent(message.content);
                      }}
                    >
                      {message.content.length > 80
                        ? `${message.content.substring(0, 80)}...`
                        : message.content}
                    </p>
                  )}
                </div>

                {/* Right side actions */}
                <div className="flex items-center gap-2 flex-shrink-0">
                  <span className="text-xs text-muted-foreground font-mono">
                    {formatTimestamp(message.timestamp)}
                  </span>

                  {/* Send Now button - inline */}
                  {onStopAndSend && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => onStopAndSend(message.id)}
                      disabled={isEditing || isSending}
                      className={`h-7 w-7 p-0 rounded-full transition-all duration-200 ${
                        isEditing || isSending
                          ? 'opacity-30 cursor-not-allowed'
                          : 'hover:bg-muted/50'
                      }`}
                      title={
                        isEditing
                          ? intl.formatMessage(i18n.cannotSendWhileEditing)
                          : intl.formatMessage(i18n.stopAndSend)
                      }
                    >
                      <Send className="w-3 h-3" />
                    </Button>
                  )}

                  {/* Remove button */}
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={isSending}
                    onClick={() => onRemoveMessage(message.id)}
                    className="opacity-60 hover:opacity-100 transition-opacity h-6 w-6 p-0 hover:bg-destructive/20 hover:text-destructive rounded-full"
                    title={intl.formatMessage(i18n.removeFromQueue)}
                  >
                    <X className="w-3 h-3" />
                  </Button>
                </div>
              </div>

              {/* Drop indicator with enhanced visuals */}
              {dragOverItem === message.id && draggedItem !== message.id && (
                <div className="absolute inset-0 border-2 border-green-400 rounded-xl pointer-events-none animate-pulse bg-green-100/20 dark:bg-green-900/20" />
              )}

              {/* Next up indicator */}
              {index === 0 && !isPaused && (
                <div className="absolute -top-2 -right-2 bg-blue-500 text-white text-xs px-2 py-1 rounded-full font-medium shadow-md">
                  {intl.formatMessage(i18n.next)}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Drag instructions */}
      {onReorderMessages && queuedMessages.length > 1 && (
        <div className="px-4 pb-3 text-xs text-muted-foreground flex items-center gap-2 opacity-60 hover:opacity-100 transition-opacity">
          <GripVertical className="w-3 h-3" />
          <span>{intl.formatMessage(i18n.dragToReorder)}</span>
        </div>
      )}
    </div>
  );
};

export default MessageQueue;

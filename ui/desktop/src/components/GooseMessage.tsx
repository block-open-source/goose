import { memo, useMemo, useRef } from 'react';
import { AlertTriangle } from 'lucide-react';
import ImagePreview from './ImagePreview';
import { formatMessageTimestamp } from '../utils/timeUtils';
import MarkdownContent from './MarkdownContent';
import ThinkingContent from './ThinkingContent';
import ToolCallWithResponse from './ToolCallWithResponse';
import {
  getTextAndImageContent,
  getThinkingContent,
  getToolRequests,
  getToolConfirmationContent,
  getElicitationContent,
  type Message,
  type NotificationEvent,
} from '../types/message';
import ToolCallConfirmation from './ToolCallConfirmation';
import ElicitationRequest from './ElicitationRequest';
import MessageCopyLink from './MessageCopyLink';
import MessageUsageStats from './MessageUsageStats';
import { getTextDirection } from '../utils/textDirection';
import { cn } from '../utils';
import type { ToolRenderState } from './messageRowContext';
import {
  STREAMING_RENDER_COOLDOWN_MS,
  useThrottledStreamingText,
} from '../hooks/useThrottledStreamingText';

const MAX_STREAMING_MARKDOWN_LENGTH = 16_000;
const LARGE_STREAMING_RENDER_COOLDOWN_MS = 250;

interface GooseMessageProps {
  sessionId: string;
  message: Message;
  hideTimestamp: boolean;
  toolStates: readonly ToolRenderState[];
  toolNotifications: readonly (NotificationEvent[] | undefined)[];
  toolConfirmationShownInline: boolean;
  append: (value: string) => void;
  isStreaming: boolean;
  submitElicitationResponse?: (
    elicitationId: string,
    userData: Record<string, unknown>
  ) => Promise<boolean>;
}

function GooseMessage({
  sessionId,
  message,
  hideTimestamp,
  toolStates,
  toolNotifications,
  toolConfirmationShownInline,
  append,
  isStreaming,
  submitElicitationResponse,
}: GooseMessageProps) {
  const contentRef = useRef<HTMLDivElement | null>(null);

  const outputTokenLimitReached = message.metadata.outputTokenLimitReached === true;
  const isOutputTokenLimitFallback =
    outputTokenLimitReached && message.metadata.fallbackContent === true;
  const { textContent, imagePaths: allImagePaths } = getTextAndImageContent(message);
  const displayText = isOutputTokenLimitFallback ? '' : textContent;
  const imagePaths = isOutputTokenLimitFallback ? [] : allImagePaths;
  const thinkingContent = isOutputTokenLimitFallback ? null : getThinkingContent(message);

  const timestamp = useMemo(() => formatMessageTimestamp(message.created), [message.created]);
  const toolRequests = getToolRequests(message);
  const shouldThrottleStreamingText =
    isStreaming && displayText.length > 0 && toolRequests.length === 0 && imagePaths.length === 0;
  const streamingRenderCooldownMs =
    displayText.length > MAX_STREAMING_MARKDOWN_LENGTH
      ? LARGE_STREAMING_RENDER_COOLDOWN_MS
      : STREAMING_RENDER_COOLDOWN_MS;
  const markdownText = useThrottledStreamingText(
    displayText,
    shouldThrottleStreamingText,
    streamingRenderCooldownMs
  );
  const toolConfirmationContent = getToolConfirmationContent(message);
  const messageDir = useMemo(() => getTextDirection(displayText) ?? undefined, [displayText]);
  const elicitationContent = getElicitationContent(message);
  const hasToolConfirmation = toolConfirmationContent !== undefined;
  const hasElicitation = elicitationContent !== undefined;
  const outputTokenLimitNotice = isOutputTokenLimitFallback
    ? "Response reached the model's output-token limit before returning content."
    : "Response reached the model's output-token limit and may be incomplete.";
  const elicitationData =
    elicitationContent?.data.actionType === 'elicitation'
      ? (elicitationContent.data as typeof elicitationContent.data & {
          isSubmitted?: boolean;
          isCancelled?: boolean;
        })
      : undefined;

  return (
    <div className="goose-message flex w-[90%] justify-start min-w-0">
      <div className="flex flex-col w-full min-w-0">
        {thinkingContent && (
          <ThinkingContent
            content={thinkingContent}
            isExpanded={
              isStreaming &&
              !displayText.trim() &&
              imagePaths.length === 0 &&
              toolRequests.length === 0
            }
          />
        )}

        {(displayText.trim() || imagePaths.length > 0) && (
          <div className="flex flex-col group">
            {displayText.trim() && (
              <div ref={contentRef} className="agent-message-bubble w-full" dir={messageDir}>
                <MarkdownContent content={markdownText} />
              </div>
            )}

            {imagePaths.length > 0 && (
              <div className="mt-4">
                {imagePaths.map((imagePath, index) => (
                  <ImagePreview key={index} src={imagePath} />
                ))}
              </div>
            )}

            {toolRequests.length === 0 && (
              <div className="relative flex items-center justify-between">
                {!isStreaming && (
                  <div className="text-xs font-mono text-text-secondary pt-1 transition-all duration-200 group-hover:-translate-y-4 group-hover:opacity-0">
                    {timestamp}
                  </div>
                )}
                {message.content.every((content) => content.type === 'text') && !isStreaming && (
                  <div className="absolute left-0 pt-1">
                    <MessageCopyLink text={displayText} contentRef={contentRef} />
                  </div>
                )}
                {!isStreaming && message.metadata.usage && (
                  <div className="pt-1 transition-all duration-200 opacity-0 group-hover:opacity-100 -translate-y-4 group-hover:translate-y-0">
                    <MessageUsageStats usage={message.metadata.usage} />
                  </div>
                )}
              </div>
            )}
          </div>
        )}

        {toolRequests.length > 0 && (
          <div className={cn(displayText && 'mt-2')}>
            <div className="relative flex flex-col w-full group">
              <div className="flex flex-col gap-3">
                {toolRequests.map((toolRequest, toolIndex) => {
                  const toolState = toolStates[toolIndex];
                  const hasResponse = toolState.response !== undefined;
                  const isApprovalClicked = Boolean(
                    toolState.confirmation && !toolState.isPending && hasResponse
                  );
                  return (
                    <div className="goose-message-tool" key={toolRequest.id}>
                      <ToolCallWithResponse
                        sessionId={sessionId}
                        isCancelledMessage={false}
                        toolRequest={toolRequest}
                        toolResponse={toolState.response}
                        notifications={toolNotifications[toolIndex]}
                        isStreamingMessage={isStreaming}
                        isPendingApproval={toolState.isPending}
                        append={append}
                        confirmationContent={toolState.confirmation}
                        isApprovalClicked={isApprovalClicked}
                      />
                    </div>
                  );
                })}
              </div>
              <div className="flex items-center justify-between">
                <div
                  className={cn(
                    'text-xs text-text-secondary pt-1',
                    message.metadata.usage &&
                      'transition-all duration-200 group-hover:-translate-y-4 group-hover:opacity-0'
                  )}
                >
                  {!isStreaming && !hideTimestamp && timestamp}
                </div>
                {!isStreaming && message.metadata.usage && (
                  <div className="pt-1 transition-all duration-200 opacity-0 group-hover:opacity-100 -translate-y-4 group-hover:translate-y-0">
                    <MessageUsageStats usage={message.metadata.usage} />
                  </div>
                )}
              </div>
            </div>
          </div>
        )}

        {outputTokenLimitReached && (
          <div className="mt-2 flex items-start gap-1.5 text-xs text-text-secondary">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-yellow-600 dark:text-yellow-400" />
            <span>{outputTokenLimitNotice}</span>
          </div>
        )}

        {hasToolConfirmation && !toolConfirmationShownInline && (
          <ToolCallConfirmation
            sessionId={sessionId}
            isClicked={false}
            actionRequiredContent={toolConfirmationContent}
          />
        )}

        {hasElicitation && submitElicitationResponse && (
          <ElicitationRequest
            isCancelledMessage={elicitationData?.isCancelled === true}
            isClicked={elicitationData?.isSubmitted === true}
            actionRequiredContent={elicitationContent}
            onSubmit={submitElicitationResponse}
          />
        )}
      </div>
    </div>
  );
}

export default memo(GooseMessage);

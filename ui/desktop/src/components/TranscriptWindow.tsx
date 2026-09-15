import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { defineMessages, useIntl } from '../i18n';
import { deriveMessageRowContexts } from './messageRowContext';
import ProgressiveMessageList, { type ProgressiveMessageListProps } from './ProgressiveMessageList';

const HEAD_COUNT = 20;
const TAIL_COUNT = 200;
const EXPAND_CHUNK = 200;
const FULL_WINDOW_COUNT = HEAD_COUNT + TAIL_COUNT;

const i18n = defineMessages({
  hiddenMessages: {
    id: 'transcriptWindow.hiddenMessages',
    defaultMessage:
      '{count, plural, one {# message hidden} other {# messages hidden}} for performance purposes',
  },
  loadEarlier: {
    id: 'transcriptWindow.loadEarlier',
    defaultMessage: 'Load earlier messages',
  },
});

type TranscriptWindowProps = Omit<ProgressiveMessageListProps, 'insertAfter' | 'rowContexts'>;

export default function TranscriptWindow(props: TranscriptWindowProps) {
  const { messages, sessionId, showLoadingThreshold } = props;
  const intl = useIntl();
  const [extraTailCount, setExtraTailCount] = useState(0);
  const [lastSessionId, setLastSessionId] = useState(sessionId);

  if (sessionId !== lastSessionId) {
    setLastSessionId(sessionId);
    setExtraTailCount(0);
  }

  const expandHiddenMessages = useCallback(() => {
    // Expand only what is currently hidden so later growth re-arms the window.
    setExtraTailCount(
      (current) => current + Math.max(0, messages.length - FULL_WINDOW_COUNT - current)
    );
  }, [messages.length]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const isMac = window.electron.platform === 'darwin';
      const isSearchShortcut = (isMac ? event.metaKey : event.ctrlKey) && event.key === 'f';
      if (isSearchShortcut) {
        expandHiddenMessages();
      }
    };

    const handleFindCommand = () => expandHiddenMessages();

    window.addEventListener('keydown', handleKeyDown);
    window.electron.on('find-command', handleFindCommand);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.electron.off('find-command', handleFindCommand);
    };
  }, [expandHiddenMessages]);

  const hiddenCount = Math.max(0, messages.length - FULL_WINDOW_COUNT - extraTailCount);
  const isWindowed = hiddenCount > 0;

  const tailStartIndex = useMemo(
    () => Math.max(HEAD_COUNT, messages.length - TAIL_COUNT - extraTailCount),
    [messages.length, extraTailCount]
  );

  const visibleMessages = useMemo(() => {
    if (!isWindowed) return messages;
    return [...messages.slice(0, HEAD_COUNT), ...messages.slice(tailStartIndex)];
  }, [messages, isWindowed, tailStartIndex]);

  // Contexts must be derived from the full transcript: tool request/response
  // pairs and model-change chains that straddle the hidden gap would otherwise
  // render as pending or spuriously announced at the window boundary.
  const allRowContexts = useMemo(() => deriveMessageRowContexts(messages), [messages]);
  const visibleRowContexts = useMemo(() => {
    if (!isWindowed) return allRowContexts;
    return [...allRowContexts.slice(0, HEAD_COUNT), ...allRowContexts.slice(tailStartIndex)];
  }, [allRowContexts, isWindowed, tailStartIndex]);

  const anchorElementRef = useRef<HTMLElement | null>(null);
  const anchorViewportOffsetRef = useRef(0);

  useLayoutEffect(() => {
    const anchor = anchorElementRef.current;
    if (!anchor) return;
    const viewport = anchor.closest<HTMLElement>('[data-radix-scroll-area-viewport]');
    anchorElementRef.current = null;
    if (!viewport?.contains(anchor)) return;
    const anchorOffset = anchor.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
    viewport.scrollTop += anchorOffset - anchorViewportOffsetRef.current;
  }, [visibleMessages]);

  const handleLoadEarlier = (event: React.MouseEvent<HTMLButtonElement>) => {
    const viewport = event.currentTarget.closest<HTMLElement>('[data-radix-scroll-area-viewport]');
    const anchor = viewport?.querySelectorAll<HTMLElement>('[data-testid="message-container"]')[
      HEAD_COUNT
    ];
    if (viewport && anchor) {
      anchorElementRef.current = anchor;
      anchorViewportOffsetRef.current =
        anchor.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
    }
    setExtraTailCount(
      (current) =>
        current + Math.min(EXPAND_CHUNK, Math.max(0, messages.length - FULL_WINDOW_COUNT - current))
    );
  };

  // Progressive rendering indexes from the front of the array; when older rows
  // are spliced in ahead of already-mounted ones, resuming the batch count
  // would briefly unmount the tail (including any live streaming row), so any
  // expansion beyond the default window mounts the visible set immediately.
  const hasExpandedWindow = extraTailCount > 0 && messages.length > FULL_WINDOW_COUNT;

  const hiddenMessagesDivider = isWindowed ? (
    <div
      data-testid="hidden-messages-divider"
      className="my-6 flex flex-col items-center gap-1 text-xs text-text-secondary"
    >
      <span data-testid="hidden-messages-count" aria-live="polite">
        {intl.formatMessage(i18n.hiddenMessages, { count: hiddenCount })}
      </span>
      <button
        type="button"
        data-testid="load-earlier-messages"
        className="rounded underline underline-offset-2 focus-visible:outline-2 focus-visible:outline-text-primary hover:opacity-80"
        onClick={handleLoadEarlier}
      >
        {intl.formatMessage(i18n.loadEarlier)}
      </button>
    </div>
  ) : undefined;

  return (
    <ProgressiveMessageList
      {...props}
      messages={visibleMessages}
      showLoadingThreshold={hasExpandedWindow ? visibleMessages.length : showLoadingThreshold}
      rowContexts={visibleRowContexts}
      insertAfter={isWindowed ? { index: HEAD_COUNT - 1, node: hiddenMessagesDivider } : undefined}
      toRawIndex={
        isWindowed
          ? (index) => (index < HEAD_COUNT ? index : index - HEAD_COUNT + tailStartIndex)
          : undefined
      }
    />
  );
}

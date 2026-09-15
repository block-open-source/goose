/**
 * Hub Component
 *
 * The empty-chat landing screen. Visually it's "Pair with no messages yet" —
 * a large time + greeting above a centered, narrower ChatInput. Submitting
 * creates a session and navigates to /pair so the rest of the chat lifecycle
 * lives there.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { defineMessages, useIntl } from '../i18n';
import { AppEvents } from '../constants/events';
import ChatInput from './ChatInput';
import { ChatInputCard } from './ChatInputCard';
import { ChatState } from '../types/chatState';
import 'react-toastify/dist/ReactToastify.css';
import { View, ViewOptions } from '../utils/navigationUtils';
import { useConfig } from './ConfigContext';
import { getEffectiveWorkingDir, getInitialWorkingDir } from '../utils/workingDir';
import { createSession } from '../sessions';
import LoadingGoose from './LoadingGoose';
import { UserInput } from '../types/message';
import {
  createNextChatExtensionDraft,
  selectNextChatExtensions,
  type NextChatExtensionDraft,
} from '../utils/nextChatExtensions';
import { formatAcpError } from '../acp/errors';
import { toastError } from '../toasts';
import { formatClockDisplay } from '../utils/timeUtils';

const i18n = defineMessages({
  goodMorning: { id: 'hub.goodMorning', defaultMessage: 'Good morning' },
  goodAfternoon: { id: 'hub.goodAfternoon', defaultMessage: 'Good afternoon' },
  goodEvening: { id: 'hub.goodEvening', defaultMessage: 'Good evening' },
});

function useClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const interval = setInterval(() => setNow(new Date()), 30_000);
    return () => clearInterval(interval);
  }, []);

  return formatClockDisplay(now);
}

export default function Hub({
  setView,
  draftRef,
}: {
  setView: (view: View, viewOptions?: ViewOptions) => void;
  /** Unsent input of this screen, kept above the route outlet across the unmount. */
  draftRef: RefObject<string>;
}) {
  const intl = useIntl();
  const { extensionsList } = useConfig();
  const [workingDir, setWorkingDir] = useState(getInitialWorkingDir());
  const userSelectedWorkingDirRef = useRef(false);
  const [isCreatingSession, setIsCreatingSession] = useState(false);
  const [nextChatExtensionDraft, setNextChatExtensionDraft] =
    useState<NextChatExtensionDraft | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const { time, meridiem, hour } = useClock();

  // Re-resolve the working dir on mount: GOOSE_WORKING_DIR is fixed at window
  // creation, so a configured remote directory may have changed since then.
  useEffect(() => {
    let active = true;
    void getEffectiveWorkingDir().then((dir) => {
      if (active && !userSelectedWorkingDirRef.current) setWorkingDir(dir);
    });
    return () => {
      active = false;
    };
  }, []);

  const greeting = useMemo(() => {
    if (hour < 12) return intl.formatMessage(i18n.goodMorning);
    if (hour < 18) return intl.formatMessage(i18n.goodAfternoon);
    return intl.formatMessage(i18n.goodEvening);
  }, [intl, hour]);

  const draftForMenu = useMemo(
    () => nextChatExtensionDraft ?? createNextChatExtensionDraft(extensionsList),
    [extensionsList, nextChatExtensionDraft]
  );

  // rAF is more reliable than autoFocus across async render boundaries.
  useEffect(() => {
    const frameId = requestAnimationFrame(() => {
      inputRef.current?.focus();
    });
    return () => cancelAnimationFrame(frameId);
  }, []);

  const handleNextChatExtensionDraftChange = useCallback((draft: NextChatExtensionDraft) => {
    setNextChatExtensionDraft(draft);
  }, []);

  const handleWorkingDirChange = useCallback((dir: string) => {
    userSelectedWorkingDirRef.current = true;
    setWorkingDir(dir);
  }, []);

  const handleSubmit = async (input: UserInput) => {
    const { msg: userMessage, images } = input;
    if (!(images.length > 0 || userMessage.trim()) || isCreatingSession) return;

    const draftAtSubmit = draftRef.current;
    setIsCreatingSession(true);

    try {
      // A draft exists only once the user has opened the picker, so its absence is
      // "not specified" while an empty draft is "start with no extensions".
      const sessionOptions = nextChatExtensionDraft
        ? {
            extensionConfigs: selectNextChatExtensions(extensionsList, nextChatExtensionDraft),
          }
        : { allExtensions: extensionsList };

      // Resolve the effective directory at submit time: the IPC lookup may still
      // be pending when the user submits, and an explicit pick must win.
      const dir = userSelectedWorkingDirRef.current ? workingDir : await getEffectiveWorkingDir();
      const session = await createSession(dir, sessionOptions);
      setNextChatExtensionDraft(null);

      window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
      window.dispatchEvent(
        new CustomEvent(AppEvents.ADD_ACTIVE_SESSION, {
          detail: { sessionId: session.id, initialMessage: { msg: userMessage, images } },
        })
      );

      // The draft is this screen's own, so it is dropped once the session exists.
      // Comparing it against the value at submit leaves an edit made while the
      // session was starting alone, including one that emptied the input.
      if (draftRef.current === draftAtSubmit) {
        draftRef.current = '';
      }

      setView('pair', {
        disableAnimation: true,
        resumeSessionId: session.id,
        initialMessage: { msg: userMessage, images },
      });
    } catch (error) {
      console.error('Failed to create session:', error);
      toastError({ title: "Couldn't start chat", msg: formatAcpError(error) });
      setIsCreatingSession(false);
    }
  };

  return (
    <div className="flex flex-col h-full min-h-0 items-center justify-center px-6 relative">
      <div className="w-full max-w-2xl">
        <div className="flex items-baseline gap-2 mb-1">
          <span className="text-6xl font-light text-text-primary tracking-tight tabular-nums">
            {time}
          </span>
          {meridiem ? (
            <span className="text-2xl font-light text-text-secondary">{meridiem}</span>
          ) : null}
        </div>
        <p className="text-xl text-text-secondary mb-6">{greeting}</p>

        <ChatInputCard>
          <ChatInput
            sessionId={null}
            draftRef={draftRef}
            handleSubmit={handleSubmit}
            chatState={isCreatingSession ? ChatState.LoadingConversation : ChatState.Idle}
            onStop={() => {}}
            initialValue=""
            setView={setView}
            totalTokens={0}
            accumulatedInputTokens={0}
            accumulatedOutputTokens={0}
            droppedFiles={[]}
            onFilesProcessed={() => {}}
            messages={[]}
            disableAnimation={false}
            workingDir={workingDir}
            onWorkingDirChange={handleWorkingDirChange}
            inputRef={inputRef}
            nextChatExtensionDraft={draftForMenu}
            onNextChatExtensionDraftChange={handleNextChatExtensionDraftChange}
          />
        </ChatInputCard>
      </div>

      {isCreatingSession && (
        <div className="absolute bottom-4 left-4 z-20 pointer-events-none">
          <LoadingGoose chatState={ChatState.LoadingConversation} />
        </div>
      )}
    </div>
  );
}

import { StrictMode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import type { Message } from '../types/message';
import { IntlTestWrapper } from '../i18n/test-utils';
import TranscriptWindow from './TranscriptWindow';

const renderCounts = vi.hoisted(() => new Map<string, number>());

vi.mock('./GooseMessage', () => ({
  default: ({ message }: { message: Message }) => {
    const id = message.id ?? textKey(message);
    renderCounts.set(id, (renderCounts.get(id) ?? 0) + 1);
    return <div>{id}</div>;
  },
}));

vi.mock('./UserMessage', () => ({
  default: ({ message }: { message: Message }) => {
    return <div>{message.id}</div>;
  },
}));

function textKey(message: Message): string {
  const first = message.content[0];
  return first && first.type === 'text' ? first.text : 'missing-key';
}

const visibleMetadata: Message['metadata'] = { agentVisible: true, userVisible: true };
const append = vi.fn();
const isUserMessage = (message: Message) => message.role === 'user';

function makeMessage(id: string): Message {
  return {
    id,
    role: 'user',
    created: 1,
    content: [{ type: 'text', text: `body-${id}` }],
    metadata: visibleMetadata,
  };
}

function makeMessages(count: number, prefix = 'm'): Message[] {
  return Array.from({ length: count }, (_, index) => makeMessage(`${prefix}-${index}`));
}

function makeAssistantMessage(id: string, resolvedModel: string): Message {
  return {
    id,
    role: 'assistant',
    created: 1,
    content: [{ type: 'text', text: `body-${id}` }],
    metadata: {
      agentVisible: true,
      userVisible: true,
      inference: { provider: 'test', requestedModel: resolvedModel, resolvedModel },
    },
  };
}

function renderWindow(messages: Message[], sessionId = 'test-session') {
  return render(
    <StrictMode>
      <IntlTestWrapper>
        <TranscriptWindow
          messages={messages}
          sessionId={sessionId}
          append={append}
          isUserMessage={isUserMessage}
        />
      </IntlTestWrapper>
    </StrictMode>
  );
}

function rerenderWindow(
  rerender: (node: React.ReactElement) => void,
  messages: Message[],
  sessionId = 'test-session'
) {
  rerender(
    <StrictMode>
      <IntlTestWrapper>
        <TranscriptWindow
          messages={messages}
          sessionId={sessionId}
          append={append}
          isUserMessage={isUserMessage}
        />
      </IntlTestWrapper>
    </StrictMode>
  );
}

function flushRendering() {
  for (let tick = 0; tick < 300; tick++) {
    act(() => {
      vi.advanceTimersByTime(20);
    });
  }
}

describe('TranscriptWindow', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders every message without a divider for short transcripts', () => {
    renderWindow(makeMessages(100));
    flushRendering();

    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();
    expect(screen.queryByText('m-0')).not.toBeNull();
    expect(screen.queryByText('m-99')).not.toBeNull();
  });

  it('does not window a transcript that fits the window', () => {
    renderWindow(makeMessages(220));
    flushRendering();

    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();
    expect(screen.queryByText('m-219')).not.toBeNull();
  });

  it('windows a long transcript to the first 20 and last 200 messages', () => {
    renderWindow(makeMessages(241));
    flushRendering();

    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('21 messages hidden');
    expect(screen.queryByText('m-0')).not.toBeNull();
    expect(screen.queryByText('m-19')).not.toBeNull();
    expect(screen.queryByText('m-20')).toBeNull();
    expect(screen.queryByText('m-40')).toBeNull();
    expect(screen.queryByText('m-41')).not.toBeNull();
    expect(screen.queryByText('m-240')).not.toBeNull();
  });

  it('uses the singular form for a single hidden message', () => {
    renderWindow(makeMessages(221));
    flushRendering();

    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('1 message hidden');
  });

  it('expands in chunks of 200 until the transcript is fully visible', () => {
    renderWindow(makeMessages(1000));

    flushRendering();
    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('780 messages hidden');

    fireEvent.click(screen.getByTestId('load-earlier-messages'));
    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('580 messages hidden');
    expect(screen.queryByText('m-600')).not.toBeNull();
    expect(screen.queryByText('m-999')).not.toBeNull();

    fireEvent.click(screen.getByTestId('load-earlier-messages'));
    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('380 messages hidden');
    expect(screen.queryByText('m-400')).not.toBeNull();
    expect(screen.queryByText('m-999')).not.toBeNull();

    fireEvent.click(screen.getByTestId('load-earlier-messages'));
    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('180 messages hidden');
    expect(screen.queryByText('m-999')).not.toBeNull();

    fireEvent.click(screen.getByTestId('load-earlier-messages'));
    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();
    expect(screen.queryByText('m-500')).not.toBeNull();
    expect(screen.queryByText('m-999')).not.toBeNull();
  });

  it('reveals the full transcript on the search shortcut', () => {
    renderWindow(makeMessages(241));
    flushRendering();
    expect(screen.queryByText('m-30')).toBeNull();

    fireEvent.keyDown(window, { metaKey: true, key: 'f' });
    flushRendering();

    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();
    expect(screen.queryByText('m-30')).not.toBeNull();
    expect(screen.queryByText('m-240')).not.toBeNull();
  });

  it('resets expansion when the session changes', () => {
    const { rerender } = renderWindow(makeMessages(241));
    flushRendering();

    fireEvent.keyDown(window, { metaKey: true, key: 'f' });
    flushRendering();
    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();

    rerenderWindow(rerender, makeMessages(241, 'next'), 'session-two');
    flushRendering();

    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('21 messages hidden');
    expect(screen.queryByText('next-40')).toBeNull();
  });

  it('keeps the tail window on the latest messages while streaming', () => {
    const messages = makeMessages(241);
    const { rerender } = renderWindow(messages);
    flushRendering();
    expect(screen.queryByText('m-41')).not.toBeNull();

    rerenderWindow(rerender, [...messages, makeMessage('m-241')]);
    flushRendering();

    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('22 messages hidden');
    expect(screen.queryByText('m-41')).toBeNull();
    expect(screen.queryByText('m-241')).not.toBeNull();
  });

  it('derives row contexts from the full transcript, not the window', () => {
    const messages = [
      ...Array.from({ length: 20 }, (_, index) => makeAssistantMessage(`a-${index}`, 'model-a')),
      ...Array.from({ length: 221 }, (_, index) => makeAssistantMessage(`b-${index}`, 'model-b')),
    ];
    renderWindow(messages);
    flushRendering();

    expect(screen.queryByText('b-21')).not.toBeNull();
    expect(screen.queryByText(/Model changed/)).toBeNull();
  });

  it('places the divider between the head and tail sections', () => {
    renderWindow(makeMessages(241));
    flushRendering();

    const divider = screen.getByTestId('hidden-messages-divider');
    const head = screen.getByText('m-19');
    const tail = screen.getByText('m-41');
    expect(head.compareDocumentPosition(divider) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(divider.compareDocumentPosition(tail) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('expands the currently hidden messages with the non-mac search shortcut', () => {
    window.electron.platform = 'win32';
    try {
      renderWindow(makeMessages(241));
      flushRendering();
      expect(screen.queryByText('m-30')).toBeNull();

      fireEvent.keyDown(window, { ctrlKey: true, key: 'f' });
      flushRendering();

      expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();
      expect(screen.queryByText('m-30')).not.toBeNull();
    } finally {
      window.electron.platform = 'darwin';
    }
  });

  it('re-arms the window when the transcript grows after a search expansion', () => {
    const messages = makeMessages(241);
    const { rerender } = renderWindow(messages);
    flushRendering();

    fireEvent.keyDown(window, { metaKey: true, key: 'f' });
    flushRendering();
    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();

    rerenderWindow(rerender, [...messages, makeMessage('m-241')]);
    flushRendering();

    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('1 message hidden');
    expect(screen.queryByText('m-241')).not.toBeNull();
  });

  it('expands the transcript when find is opened via the renderer find command', () => {
    renderWindow(makeMessages(241));
    flushRendering();
    expect(screen.queryByText('m-30')).toBeNull();

    const onMock = window.electron.on as unknown as {
      mock: { calls: [string, () => void][] };
    };
    const findCommandCalls = onMock.mock.calls.filter(([event]) => event === 'find-command');
    const findCommandHandler = findCommandCalls[findCommandCalls.length - 1]?.[1];
    expect(findCommandHandler).toBeTypeOf('function');
    act(() => findCommandHandler());
    flushRendering();

    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();
    expect(screen.queryByText('m-30')).not.toBeNull();
  });

  it('clamps load earlier to the currently hidden count', () => {
    const messages = makeMessages(221);
    const { rerender } = renderWindow(messages);
    flushRendering();
    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('1 message hidden');

    fireEvent.click(screen.getByTestId('load-earlier-messages'));
    flushRendering();
    expect(screen.queryByTestId('hidden-messages-divider')).toBeNull();

    rerenderWindow(rerender, [...messages, makeMessage('m-221')]);
    flushRendering();

    expect(screen.getByTestId('hidden-messages-count')).toHaveTextContent('1 message hidden');
    expect(screen.queryByText('m-221')).not.toBeNull();
  });

  it('keeps stable fallback keys for id-less messages when the window slides', () => {
    renderCounts.clear();
    const idLessMessages: Message[] = Array.from({ length: 241 }, (_, index) => ({
      role: 'assistant' as const,
      created: 1,
      content: [{ type: 'text' as const, text: `body-${index}` }],
      metadata: visibleMetadata,
    }));
    const { rerender } = renderWindow(idLessMessages);
    flushRendering();
    const body100BeforeSlide = renderCounts.get('body-100');
    expect(body100BeforeSlide).toBeGreaterThan(0);

    rerenderWindow(rerender, [...idLessMessages, makeMessage('appended')]);
    flushRendering();

    expect(renderCounts.get('body-100')).toBe(body100BeforeSlide);
    expect(screen.queryByText('body-100')).not.toBeNull();
    expect(screen.queryByText('appended')).not.toBeNull();
  });
});

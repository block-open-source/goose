/**
 * @vitest-environment jsdom
 */
import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import Hub from './Hub';
import { IntlTestWrapper } from '../i18n/test-utils';
import { createSession } from '../sessions';
import { UserInput } from '../types/message';

type ChatInputCapture = {
  draftRef?: { current: string };
  handleSubmit: (input: UserInput) => void;
  onNextChatExtensionDraftChange?: (draft: { selectedNames: Set<string> }) => void;
};

type Session = Awaited<ReturnType<typeof createSession>>;

const captured = vi.hoisted(() => ({ chatInput: null as ChatInputCapture | null }));

vi.mock('./ChatInput', () => ({
  default: (props: ChatInputCapture) => {
    captured.chatInput = props;
    return <div data-testid="chat-input" />;
  },
}));

vi.mock('./LoadingGoose', () => ({ default: () => <div /> }));

vi.mock('./ConfigContext', () => ({
  useConfig: () => ({ extensionsList: [] }),
}));

vi.mock('../sessions', () => ({ createSession: vi.fn() }));

vi.mock('../utils/workingDir', () => ({
  getInitialWorkingDir: () => '/tmp/goose',
  getEffectiveWorkingDir: () => Promise.resolve('/tmp/goose'),
}));

vi.mock('../utils/nextChatExtensions', () => ({
  createNextChatExtensionDraft: () => ({}),
  selectNextChatExtensions: () => [],
}));

vi.mock('../acp/errors', () => ({ formatAcpError: (error: unknown) => String(error) }));

vi.mock('../toasts', () => ({ toastError: vi.fn() }));

const DRAFT = 'a half-written thought';
const TYPED_WHILE_STARTING = 'and one more thought';

/** Holds session creation open, so the test can edit the draft while it is pending. */
function pendingSession() {
  const settle: { started?: () => void; failed?: () => void } = {};
  vi.mocked(createSession).mockImplementation(
    () =>
      new Promise<Session>((resolve, reject) => {
        settle.started = () => resolve({ id: 'session-1' } as Session);
        settle.failed = () => reject(new Error('no agent'));
      })
  );
  return settle;
}

function renderHub(draftRef: { current: string }) {
  return render(
    <IntlTestWrapper>
      <Hub setView={vi.fn()} draftRef={draftRef} />
    </IntlTestWrapper>
  );
}

async function submit() {
  await act(async () => {
    captured.chatInput?.handleSubmit({ msg: DRAFT, images: [] });
  });
}

describe('Hub', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    captured.chatInput = null;
  });

  it('starts a chat with no extensions when the user cleared the picker', async () => {
    vi.mocked(createSession).mockResolvedValue({ id: 'session-1' } as Session);
    renderHub({ current: '' });

    // Touching the picker is what turns "not specified" into a real choice, and
    // clearing it is the case the composer already promises in a toast.
    await act(async () => {
      captured.chatInput?.onNextChatExtensionDraftChange?.({ selectedNames: new Set() });
    });
    await submit();

    expect(createSession).toHaveBeenCalledWith('/tmp/goose', { extensionConfigs: [] });
  });

  it('leaves the set unspecified when the picker was never touched', async () => {
    vi.mocked(createSession).mockResolvedValue({ id: 'session-1' } as Session);
    renderHub({ current: '' });

    await submit();

    expect(createSession).toHaveBeenCalledWith('/tmp/goose', { allExtensions: [] });
  });

  it('hands the draft to the input', () => {
    const draftRef = { current: DRAFT };
    renderHub(draftRef);

    expect(captured.chatInput?.draftRef).toBe(draftRef);
  });

  it('drops the draft once the chat starts', async () => {
    const session = pendingSession();
    const draftRef = { current: DRAFT };
    renderHub(draftRef);

    await submit();
    await act(async () => session.started?.());

    expect(draftRef.current).toBe('');
  });

  it('keeps the draft when the chat fails to start', async () => {
    const session = pendingSession();
    const draftRef = { current: DRAFT };
    renderHub(draftRef);

    await submit();
    await act(async () => session.failed?.());

    expect(draftRef.current).toBe(DRAFT);
  });

  // The input stays editable while the session is being created, so what is in the
  // draft when creation ends is not necessarily what was submitted.
  it('keeps text typed while the chat was starting', async () => {
    const session = pendingSession();
    const draftRef = { current: DRAFT };
    renderHub(draftRef);

    await submit();
    draftRef.current = TYPED_WHILE_STARTING;
    await act(async () => session.started?.());

    expect(draftRef.current).toBe(TYPED_WHILE_STARTING);
  });

  it('keeps text typed while a failing chat was starting', async () => {
    const session = pendingSession();
    const draftRef = { current: DRAFT };
    renderHub(draftRef);

    await submit();
    draftRef.current = TYPED_WHILE_STARTING;
    await act(async () => session.failed?.());

    expect(draftRef.current).toBe(TYPED_WHILE_STARTING);
  });

  it('leaves the draft empty when the input was cleared while the chat was starting', async () => {
    const session = pendingSession();
    const draftRef = { current: DRAFT };
    renderHub(draftRef);

    await submit();
    draftRef.current = '';
    await act(async () => session.failed?.());

    expect(draftRef.current).toBe('');
  });
});

import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { isAcpRecovering, subscribeToAcpRecovery } from '../acp/acpConnection';
import { acpStartLiveVoice, acpStopLiveVoice } from '../acp/liveVoice';
import { publishLiveVoiceCallEnded } from '../acp/liveVoiceNotifications';
import { LiveVoiceMediaSession } from './LiveVoiceMediaSession';
import { useLiveVoice } from './useLiveVoice';

vi.mock('../acp/liveVoice', () => ({
  acpStartLiveVoice: vi.fn(),
  acpStopLiveVoice: vi.fn(),
}));

vi.mock('../acp/acpConnection', () => ({
  isAcpRecovering: vi.fn(() => false),
  subscribeToAcpRecovery: vi.fn(),
}));

vi.mock('./LiveVoiceMediaSession', () => ({
  LiveVoiceMediaSession: vi.fn(),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe('useLiveVoice', () => {
  let mediaFailure: () => void;
  let recoveryChanged: (recovering: boolean) => void;
  const media = {
    createOffer: vi.fn(),
    applyAnswer: vi.fn(),
    setMuted: vi.fn(),
    teardown: vi.fn(),
  };
  beforeEach(() => {
    vi.clearAllMocks();
    media.createOffer.mockResolvedValue('offer');
    media.applyAnswer.mockResolvedValue(undefined);
    vi.mocked(isAcpRecovering).mockReturnValue(false);
    vi.mocked(subscribeToAcpRecovery).mockImplementation((listener) => {
      recoveryChanged = listener;
      return () => undefined;
    });
    vi.mocked(LiveVoiceMediaSession).mockImplementation(
      function LiveVoiceMediaSessionMock(onFailure) {
        mediaFailure = onFailure;
        return media as unknown as LiveVoiceMediaSession;
      }
    );
    vi.mocked(acpStartLiveVoice).mockResolvedValue({
      callId: 'live-opaque',
      answerSdp: 'answer',
    });
    vi.mocked(acpStopLiveVoice).mockResolvedValue(undefined);
  });

  it('starts media from the start response and stops the same connection', async () => {
    const { result } = renderHook(() => useLiveVoice('main-session'));

    await act(async () => result.current.start());
    expect(media.applyAnswer).toHaveBeenCalledWith('answer');
    expect(media.setMuted).toHaveBeenCalledWith(false);
    expect(result.current.phase).toBe('live');

    await act(async () => result.current.stop());
    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
    expect(result.current.phase).toBe('idle');
  });

  it('stops the server call when media setup fails after start', async () => {
    media.applyAnswer.mockRejectedValueOnce(new Error('media failed'));
    const { result } = renderHook(() => useLiveVoice('main-session'));

    await act(async () => result.current.start());

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
    expect(result.current.phase).toBe('error');
  });

  it('tears down and stops an active call when unmounted', async () => {
    const { result, unmount } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    unmount();

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
  });

  it.each(['unmount', 'stop'] as const)(
    'closes a call that finishes after %s while connecting',
    async (action) => {
      const pending = deferred<{ callId: string; answerSdp: string }>();
      vi.mocked(acpStartLiveVoice).mockReturnValueOnce(pending.promise);
      const { result, unmount } = renderHook(() => useLiveVoice('main-session'));
      act(() => {
        void result.current.start();
      });
      await waitFor(() => expect(acpStartLiveVoice).toHaveBeenCalledOnce());

      if (action === 'unmount') {
        unmount();
      } else {
        await act(async () => result.current.stop());
        expect(result.current.phase).toBe('idle');
      }
      await act(async () => {
        pending.resolve({ callId: 'late-call', answerSdp: 'late-answer' });
      });

      expect(media.teardown).toHaveBeenCalledOnce();
      expect(media.applyAnswer).not.toHaveBeenCalled();
      expect(media.setMuted).not.toHaveBeenCalled();
      expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'late-call');
    }
  );

  it('does not become live after stopping during answer setup', async () => {
    const pending = deferred<void>();
    media.applyAnswer.mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useLiveVoice('main-session'));
    act(() => {
      void result.current.start();
    });
    await waitFor(() => expect(media.applyAnswer).toHaveBeenCalledOnce());

    await act(async () => result.current.stop());
    await act(async () => pending.resolve(undefined));

    expect(media.setMuted).not.toHaveBeenCalled();
    expect(result.current.phase).toBe('idle');
  });

  it('applies rapid mute changes to the current media session', async () => {
    const { result } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    act(() => {
      result.current.toggleMute();
      result.current.toggleMute();
      result.current.toggleMute();
    });

    expect(media.setMuted.mock.calls).toEqual([[false], [true], [false], [true]]);
    expect(result.current.muted).toBe(true);
  });

  it('cleans up when the backend reports that the current call failed', async () => {
    const { result } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    act(() => {
      publishLiveVoiceCallEnded({
        sessionId: 'main-session',
        update: {
          sessionUpdate: 'live_voice_call_ended',
          callId: 'live-opaque',
          outcome: 'failed',
        },
      });
      publishLiveVoiceCallEnded({
        sessionId: 'main-session',
        update: {
          sessionUpdate: 'live_voice_call_ended',
          callId: 'live-opaque',
          outcome: 'failed',
        },
      });
    });

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).not.toHaveBeenCalled();
    expect(result.current.phase).toBe('error');
  });

  it('ignores stale session and call endings', async () => {
    const { result } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    act(() => {
      publishLiveVoiceCallEnded({
        sessionId: 'other-session',
        update: {
          sessionUpdate: 'live_voice_call_ended',
          callId: 'live-opaque',
          outcome: 'failed',
        },
      });
      publishLiveVoiceCallEnded({
        sessionId: 'main-session',
        update: {
          sessionUpdate: 'live_voice_call_ended',
          callId: 'old-call',
          outcome: 'failed',
        },
      });
    });

    expect(media.teardown).not.toHaveBeenCalled();
    expect(result.current.phase).toBe('live');
  });

  it('reconciles a terminal update that arrives before the start response', async () => {
    const pending = deferred<{ callId: string; answerSdp: string }>();
    vi.mocked(acpStartLiveVoice).mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useLiveVoice('main-session'));
    act(() => {
      void result.current.start();
    });
    await waitFor(() => expect(acpStartLiveVoice).toHaveBeenCalledOnce());

    act(() => {
      publishLiveVoiceCallEnded({
        sessionId: 'main-session',
        update: {
          sessionUpdate: 'live_voice_call_ended',
          callId: 'early-call',
          outcome: 'failed',
        },
      });
    });
    await act(async () => pending.resolve({ callId: 'early-call', answerSdp: 'answer' }));

    expect(media.applyAnswer).not.toHaveBeenCalled();
    expect(media.teardown).toHaveBeenCalledOnce();
    expect(result.current.phase).toBe('error');
  });

  it('cleans up locally without stopping through a recovering ACP connection', async () => {
    const { result } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    act(() => recoveryChanged(true));

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).not.toHaveBeenCalled();
    expect(result.current.phase).toBe('idle');
  });

  it('does not stop a late call response through a recovered ACP connection', async () => {
    const pending = deferred<{ callId: string; answerSdp: string }>();
    vi.mocked(acpStartLiveVoice).mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useLiveVoice('main-session'));
    act(() => {
      void result.current.start();
    });
    await waitFor(() => expect(acpStartLiveVoice).toHaveBeenCalledOnce());

    act(() => recoveryChanged(true));
    await act(async () => pending.resolve({ callId: 'old-call', answerSdp: 'answer' }));

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).not.toHaveBeenCalled();
    expect(result.current.phase).toBe('idle');
  });

  it('ends the matching call when active media fails', async () => {
    const { result } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    act(() => mediaFailure());

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
    expect(result.current.phase).toBe('error');
  });
});

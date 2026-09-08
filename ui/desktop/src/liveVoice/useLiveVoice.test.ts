import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { acpStartLiveVoice, acpStopLiveVoice } from '../acp/liveVoice';
import { LiveVoiceMediaSession } from './LiveVoiceMediaSession';
import { useLiveVoice } from './useLiveVoice';

vi.mock('../acp/liveVoice', () => ({
  acpStartLiveVoice: vi.fn(),
  acpStopLiveVoice: vi.fn(),
}));

vi.mock('./LiveVoiceMediaSession', () => ({
  LiveVoiceMediaSession: vi.fn(),
}));

describe('useLiveVoice', () => {
  const media = {
    createOffer: vi.fn(),
    applyAnswer: vi.fn(),
    enableMicrophone: vi.fn(),
    teardown: vi.fn(),
  };
  beforeEach(() => {
    vi.clearAllMocks();
    media.createOffer.mockResolvedValue('offer');
    media.applyAnswer.mockResolvedValue(undefined);
    vi.mocked(LiveVoiceMediaSession).mockImplementation(function LiveVoiceMediaSessionMock() {
      return media as unknown as LiveVoiceMediaSession;
    });
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
    expect(media.enableMicrophone).toHaveBeenCalledOnce();
    expect(result.current.state).toBe('live');

    await act(async () => result.current.stop());
    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
    expect(result.current.state).toBe('idle');
  });

  it('stops the server call when media setup fails after start', async () => {
    media.applyAnswer.mockRejectedValueOnce(new Error('media failed'));
    const { result } = renderHook(() => useLiveVoice('main-session'));

    await act(async () => result.current.start());

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
    expect(result.current.state).toBe('error');
  });

  it('tears down and stops an active call when unmounted', async () => {
    const { result, unmount } = renderHook(() => useLiveVoice('main-session'));
    await act(async () => result.current.start());

    unmount();

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'live-opaque');
  });

  it('stops a call that finishes starting after unmount', async () => {
    let finishStart!: (value: { callId: string; answerSdp: string }) => void;
    vi.mocked(acpStartLiveVoice).mockReturnValueOnce(
      new Promise((resolve) => {
        finishStart = resolve;
      })
    );
    const { result, unmount } = renderHook(() => useLiveVoice('main-session'));
    let start!: Promise<void>;
    act(() => {
      start = result.current.start();
    });
    await waitFor(() => expect(acpStartLiveVoice).toHaveBeenCalledOnce());

    unmount();
    await act(async () => {
      finishStart({ callId: 'late-call', answerSdp: 'late-answer' });
      await start;
    });

    expect(media.teardown).toHaveBeenCalledOnce();
    expect(media.applyAnswer).not.toHaveBeenCalled();
    expect(media.enableMicrophone).not.toHaveBeenCalled();
    expect(acpStopLiveVoice).toHaveBeenCalledWith('main-session', 'late-call');
  });
});

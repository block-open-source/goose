import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAcpClient } from '../acpConnection';
import { acpGetLiveVoiceAvailability, acpStartLiveVoice, acpStopLiveVoice } from '../liveVoice';

vi.mock('../acpConnection', () => ({ getAcpClient: vi.fn() }));

describe('ACP Live voice', () => {
  beforeEach(() => vi.clearAllMocks());

  it('uses the generated availability client for the displayed session', async () => {
    const sessionLiveVoiceAvailability = vi.fn().mockResolvedValue({
      status: 'ready',
      message: 'Start Live voice',
    });
    vi.mocked(getAcpClient).mockResolvedValue({
      goose: {
        sessionLiveVoiceAvailability_unstable: sessionLiveVoiceAvailability,
      },
    } as unknown as Awaited<ReturnType<typeof getAcpClient>>);

    await expect(acpGetLiveVoiceAvailability('main-session')).resolves.toEqual({
      status: 'ready',
      message: 'Start Live voice',
    });
    expect(sessionLiveVoiceAvailability).toHaveBeenCalledWith({ sessionId: 'main-session' });
  });

  it('uses generated start and stop clients with the call ID', async () => {
    const start = vi.fn().mockResolvedValue({
      callId: 'live-opaque',
      answerSdp: 'answer',
    });
    const stop = vi.fn().mockResolvedValue({});
    vi.mocked(getAcpClient).mockResolvedValue({
      goose: {
        sessionLiveVoiceStart_unstable: start,
        sessionLiveVoiceStop_unstable: stop,
      },
    } as unknown as Awaited<ReturnType<typeof getAcpClient>>);

    await expect(acpStartLiveVoice('main-session', 'offer')).resolves.toMatchObject({
      callId: 'live-opaque',
    });
    await acpStopLiveVoice('main-session', 'live-opaque');

    expect(start).toHaveBeenCalledWith({
      sessionId: 'main-session',
      offerSdp: 'offer',
    });
    expect(stop).toHaveBeenCalledWith({
      sessionId: 'main-session',
      callId: 'live-opaque',
    });
  });
});

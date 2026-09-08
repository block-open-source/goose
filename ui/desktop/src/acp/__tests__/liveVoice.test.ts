import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAcpClient } from '../acpConnection';
import { acpGetLiveVoiceAvailability } from '../liveVoice';

vi.mock('../acpConnection', () => ({ getAcpClient: vi.fn() }));

describe('ACP Live voice', () => {
  beforeEach(() => vi.clearAllMocks());

  it('uses the generated availability client for the displayed session', async () => {
    const sessionLiveVoiceAvailability = vi.fn().mockResolvedValue({
      status: 'ready',
    });
    vi.mocked(getAcpClient).mockResolvedValue({
      goose: {
        sessionLiveVoiceAvailability_unstable: sessionLiveVoiceAvailability,
      },
    } as unknown as Awaited<ReturnType<typeof getAcpClient>>);

    await expect(acpGetLiveVoiceAvailability('main-session')).resolves.toEqual({
      status: 'ready',
    });
    expect(sessionLiveVoiceAvailability).toHaveBeenCalledWith({ sessionId: 'main-session' });
  });
});

import type { LiveVoiceAvailabilityResponse_unstable } from '@aaif/goose-sdk';
import { getAcpClient } from './acpConnection';

export async function acpGetLiveVoiceAvailability(
  sessionId: string
): Promise<LiveVoiceAvailabilityResponse_unstable> {
  const { goose } = await getAcpClient();
  return goose.sessionLiveVoiceAvailability_unstable({ sessionId });
}

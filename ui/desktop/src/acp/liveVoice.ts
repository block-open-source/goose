import type {
  LiveVoiceAvailabilityResponse_unstable,
  LiveVoiceStartResponse_unstable,
} from '@aaif/goose-sdk';
import { getAcpClient } from './acpConnection';

export async function acpGetLiveVoiceAvailability(
  sessionId: string
): Promise<LiveVoiceAvailabilityResponse_unstable> {
  const { goose } = await getAcpClient();
  return goose.sessionLiveVoiceAvailability_unstable({ sessionId });
}

export async function acpStartLiveVoice(
  sessionId: string,
  offerSdp: string
): Promise<LiveVoiceStartResponse_unstable> {
  const { goose } = await getAcpClient();
  return goose.sessionLiveVoiceStart_unstable({ sessionId, offerSdp });
}

export async function acpStopLiveVoice(sessionId: string, callId: string): Promise<void> {
  const { goose } = await getAcpClient();
  await goose.sessionLiveVoiceStop_unstable({ sessionId, callId });
}

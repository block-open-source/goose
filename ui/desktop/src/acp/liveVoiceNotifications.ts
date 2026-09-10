import type { GooseSessionNotification_unstable } from '@aaif/goose-sdk';

export type LiveVoiceCallEndedNotification = {
  sessionId: string;
  update: Extract<
    GooseSessionNotification_unstable['update'],
    { sessionUpdate: 'live_voice_call_ended' }
  >;
};

type LiveVoiceCallEndedListener = (notification: LiveVoiceCallEndedNotification) => void;

const listeners = new Set<LiveVoiceCallEndedListener>();

export function subscribeToLiveVoiceCallEnded(listener: LiveVoiceCallEndedListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function publishLiveVoiceCallEnded(notification: LiveVoiceCallEndedNotification): void {
  for (const listener of listeners) {
    listener(notification);
  }
}

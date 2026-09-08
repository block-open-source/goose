import { useCallback, useEffect, useRef, useState } from 'react';
import { acpStartLiveVoice, acpStopLiveVoice } from '../acp/liveVoice';
import { LiveVoiceMediaSession } from './LiveVoiceMediaSession';

export type LiveVoiceUiState = 'idle' | 'connecting' | 'live' | 'stopping' | 'error';

interface LiveVoiceCall {
  sessionId: string;
  callId?: string;
  media: LiveVoiceMediaSession;
  cancelled: boolean;
}

export function useLiveVoice(sessionId: string) {
  const [state, setState] = useState<LiveVoiceUiState>('idle');
  const callRef = useRef<LiveVoiceCall | null>(null);

  useEffect(() => {
    setState('idle');
    return () => {
      const call = callRef.current;
      if (!call || call.sessionId !== sessionId) return;

      callRef.current = null;
      call.media.teardown();
      if (call.callId) {
        void acpStopLiveVoice(call.sessionId, call.callId).catch(() => undefined);
      }
    };
  }, [sessionId]);

  const start = useCallback(async () => {
    if (callRef.current) return;

    setState('connecting');
    const call: LiveVoiceCall = {
      sessionId,
      media: new LiveVoiceMediaSession(),
      cancelled: false,
    };
    callRef.current = call;
    const isCurrent = () => callRef.current === call && !call.cancelled;

    try {
      const offerSdp = await call.media.createOffer();
      if (!isCurrent()) return;

      const response = await acpStartLiveVoice(sessionId, offerSdp);
      call.callId = response.callId;
      if (!isCurrent()) {
        void acpStopLiveVoice(call.sessionId, call.callId).catch(() => undefined);
        return;
      }

      await call.media.applyAnswer(response.answerSdp);
      if (!isCurrent()) return;

      call.media.enableMicrophone();
      setState('live');
    } catch {
      call.media.teardown();
      if (!isCurrent()) return;

      callRef.current = null;
      if (call.callId) {
        void acpStopLiveVoice(call.sessionId, call.callId).catch(() => undefined);
      }
      setState('error');
    }
  }, [sessionId]);

  const stop = useCallback(async () => {
    const call = callRef.current;
    if (!call || call.cancelled) return;

    call.cancelled = true;
    call.media.teardown();
    if (!call.callId) {
      callRef.current = null;
      setState('idle');
      return;
    }

    setState('stopping');
    try {
      await acpStopLiveVoice(call.sessionId, call.callId);
      if (callRef.current !== call) return;

      callRef.current = null;
      setState('idle');
    } catch {
      if (callRef.current === call) {
        callRef.current = null;
        setState('error');
      }
    }
  }, []);

  return { state, start, stop };
}

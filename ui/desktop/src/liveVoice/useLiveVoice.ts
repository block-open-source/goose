import { useCallback, useEffect, useRef, useState } from 'react';
import { acpStartLiveVoice, acpStopLiveVoice } from '../acp/liveVoice';
import { LiveVoiceMediaSession } from './LiveVoiceMediaSession';

export type LiveVoicePhase = 'idle' | 'connecting' | 'live' | 'stopping' | 'error';

export interface LiveVoiceController {
  phase: LiveVoicePhase;
  muted: boolean;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  toggleMute: () => void;
}

interface LiveVoiceCall {
  sessionId: string;
  callId?: string;
  media: LiveVoiceMediaSession;
  mediaReady: boolean;
  cancelled: boolean;
}

export function useLiveVoice(sessionId: string): LiveVoiceController {
  const [phase, setPhase] = useState<LiveVoicePhase>('idle');
  const [muted, setMuted] = useState(false);
  const mutedRef = useRef(false);
  const callRef = useRef<LiveVoiceCall | null>(null);

  useEffect(() => {
    setPhase('idle');
    mutedRef.current = false;
    setMuted(false);
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

    mutedRef.current = false;
    setMuted(false);
    setPhase('connecting');
    const call: LiveVoiceCall = {
      sessionId,
      media: new LiveVoiceMediaSession(),
      mediaReady: false,
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

      call.mediaReady = true;
      call.media.setMuted(mutedRef.current);
      setPhase('live');
    } catch {
      call.media.teardown();
      if (!isCurrent()) return;

      callRef.current = null;
      mutedRef.current = false;
      setMuted(false);
      if (call.callId) {
        void acpStopLiveVoice(call.sessionId, call.callId).catch(() => undefined);
      }
      setPhase('error');
    }
  }, [sessionId]);

  const toggleMute = useCallback(() => {
    const call = callRef.current;
    if (!call || call.cancelled || !call.mediaReady) return;

    mutedRef.current = !mutedRef.current;
    call.media.setMuted(mutedRef.current);
    setMuted(mutedRef.current);
  }, []);

  const stop = useCallback(async () => {
    const call = callRef.current;
    if (!call || call.cancelled) return;

    call.cancelled = true;
    call.media.teardown();
    mutedRef.current = false;
    setMuted(false);
    if (!call.callId) {
      callRef.current = null;
      setPhase('idle');
      return;
    }

    setPhase('stopping');
    try {
      await acpStopLiveVoice(call.sessionId, call.callId);
      if (callRef.current !== call) return;

      callRef.current = null;
      setPhase('idle');
    } catch {
      if (callRef.current === call) {
        callRef.current = null;
        setPhase('error');
      }
    }
  }, []);

  return { phase, muted, start, stop, toggleMute };
}

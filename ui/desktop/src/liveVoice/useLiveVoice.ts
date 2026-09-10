import { useCallback, useEffect, useRef, useState } from 'react';
import { isAcpRecovering, subscribeToAcpRecovery } from '../acp/acpConnection';
import { acpStartLiveVoice, acpStopLiveVoice } from '../acp/liveVoice';
import {
  subscribeToLiveVoiceCallEnded,
  type LiveVoiceCallEndedNotification,
} from '../acp/liveVoiceNotifications';
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
  invalidated: boolean;
  acpConnectionLost: boolean;
  pendingOutcomesByCallId: Map<string, LiveVoiceCallEndedNotification['update']['outcome']>;
}

function requestRemoteStop(call: LiveVoiceCall): void {
  if (!call.callId || call.acpConnectionLost) return;
  void acpStopLiveVoice(call.sessionId, call.callId).catch(() => undefined);
}

export function useLiveVoice(sessionId: string): LiveVoiceController {
  const [phase, setPhase] = useState<LiveVoicePhase>('idle');
  const [muted, setMuted] = useState(false);
  const mutedRef = useRef(false);
  const callRef = useRef<LiveVoiceCall | null>(null);

  const invalidateCallAndReleaseMedia = useCallback((call: LiveVoiceCall) => {
    if (call.invalidated) return;
    call.invalidated = true;
    call.media.teardown();
    mutedRef.current = false;
  }, []);

  const finishCurrentCall = useCallback(
    (call: LiveVoiceCall, outcome: LiveVoiceCallEndedNotification['update']['outcome']) => {
      if (callRef.current !== call) return false;

      callRef.current = null;
      invalidateCallAndReleaseMedia(call);
      setMuted(false);
      setPhase(outcome === 'failed' ? 'error' : 'idle');
      return true;
    },
    [invalidateCallAndReleaseMedia]
  );

  useEffect(() => {
    setPhase('idle');
    mutedRef.current = false;
    setMuted(false);
    return () => {
      const call = callRef.current;
      if (!call || call.sessionId !== sessionId) return;

      callRef.current = null;
      invalidateCallAndReleaseMedia(call);
      requestRemoteStop(call);
    };
  }, [invalidateCallAndReleaseMedia, sessionId]);

  useEffect(() => {
    return subscribeToLiveVoiceCallEnded((notification) => {
      const call = callRef.current;
      if (!call || call.sessionId !== notification.sessionId || call.invalidated) return;

      if (!call.callId) {
        call.pendingOutcomesByCallId.set(notification.update.callId, notification.update.outcome);
        return;
      }
      if (call.callId === notification.update.callId) {
        finishCurrentCall(call, notification.update.outcome);
      }
    });
  }, [finishCurrentCall]);

  useEffect(() => {
    return subscribeToAcpRecovery((recovering) => {
      if (!recovering) return;

      const call = callRef.current;
      if (call?.sessionId === sessionId) {
        call.acpConnectionLost = true;
        finishCurrentCall(call, 'stopped');
      }
    });
  }, [finishCurrentCall, sessionId]);

  const start = useCallback(async () => {
    if (callRef.current || isAcpRecovering()) return;

    mutedRef.current = false;
    setMuted(false);
    setPhase('connecting');
    let call: LiveVoiceCall;
    const media = new LiveVoiceMediaSession(() => {
      if (!finishCurrentCall(call, 'failed')) return;
      requestRemoteStop(call);
    });
    call = {
      sessionId,
      media,
      mediaReady: false,
      invalidated: false,
      acpConnectionLost: false,
      pendingOutcomesByCallId: new Map(),
    };
    callRef.current = call;
    const isCurrent = () => callRef.current === call && !call.invalidated;

    try {
      const offerSdp = await call.media.createOffer();
      if (!isCurrent()) return;

      const response = await acpStartLiveVoice(sessionId, offerSdp);
      call.callId = response.callId;
      const pendingOutcome = call.pendingOutcomesByCallId.get(call.callId);
      call.pendingOutcomesByCallId.clear();
      if (pendingOutcome) {
        finishCurrentCall(call, pendingOutcome);
        return;
      }
      if (!isCurrent()) {
        requestRemoteStop(call);
        return;
      }

      await call.media.applyAnswer(response.answerSdp);
      if (!isCurrent()) return;

      call.mediaReady = true;
      call.media.setMuted(mutedRef.current);
      setPhase('live');
    } catch {
      if (!isCurrent()) {
        invalidateCallAndReleaseMedia(call);
        return;
      }

      finishCurrentCall(call, 'failed');
      requestRemoteStop(call);
    }
  }, [finishCurrentCall, invalidateCallAndReleaseMedia, sessionId]);

  const toggleMute = useCallback(() => {
    const call = callRef.current;
    if (!call || call.invalidated || !call.mediaReady) return;

    mutedRef.current = !mutedRef.current;
    call.media.setMuted(mutedRef.current);
    setMuted(mutedRef.current);
  }, []);

  const stop = useCallback(async () => {
    const call = callRef.current;
    if (!call || call.invalidated) return;

    invalidateCallAndReleaseMedia(call);
    setMuted(false);
    if (!call.callId) {
      callRef.current = null;
      setPhase('idle');
      return;
    }

    setPhase('stopping');
    try {
      await acpStopLiveVoice(call.sessionId, call.callId);
      finishCurrentCall(call, 'stopped');
    } catch {
      finishCurrentCall(call, 'failed');
    }
  }, [finishCurrentCall, invalidateCallAndReleaseMedia]);

  return { phase, muted, start, stop, toggleMute };
}

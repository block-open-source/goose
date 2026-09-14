import { useState, useRef, useCallback, useEffect } from 'react';
import { getDictationConfig, transcribeDictation } from '../acp/dictation';
import { useConfig } from '../components/ConfigContext';
import type { DictationProvider } from '../types/dictation';
import { errorMessage } from '../utils/conversionUtils';

interface UseAudioRecorderOptions {
  onTranscription: (text: string) => void;
  onError: (message: string) => void;
}

const SAMPLE_RATE = 16000;
const SILENCE_MS = 800;
const MIN_SPEECH_MS = 200;
// RMS threshold for speech detection. Audio samples are Float32 in [-1, 1] range.
// 0.015 (~1.5% of full-scale) distinguishes normal speech from background noise
// without clipping early speech onsets. Determined empirically for 16kHz mono input.
const RMS_THRESHOLD = 0.015;

interface RecorderGeneration {
  audioContext: AudioContext | null;
  cancelled: boolean;
  completesAfterTranscription: boolean;
  pendingTranscriptions: number;
  stream: MediaStream | null;
  worklet: AudioWorkletNode | null;
}

function cleanupGeneration(generation: RecorderGeneration) {
  generation.cancelled = true;

  const worklet = generation.worklet;
  generation.worklet = null;
  if (worklet) {
    worklet.port.onmessage = null;
    worklet.disconnect();
  }

  const audioContext = generation.audioContext;
  generation.audioContext = null;
  if (audioContext) {
    void audioContext.close();
  }

  const stream = generation.stream;
  generation.stream = null;
  stream?.getTracks().forEach((track) => track.stop());
}

// Resolve worklet URL at runtime from window.location so it works under both
// the dev server (http://localhost) and packaged builds (file://).
const WORKLET_URL = new URL('audio-capture-worklet.js', window.location.href.split('#')[0]).href;

function encodeWav(samples: Float32Array, sampleRate: number): ArrayBuffer {
  const buf = new ArrayBuffer(44 + samples.length * 2);
  const v = new DataView(buf);
  const w = (o: number, s: string) => {
    for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i));
  };
  w(0, 'RIFF');
  v.setUint32(4, 36 + samples.length * 2, true);
  w(8, 'WAVE');
  w(12, 'fmt ');
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, sampleRate, true);
  v.setUint32(28, sampleRate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  w(36, 'data');
  v.setUint32(40, samples.length * 2, true);
  let o = 44;
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    v.setInt16(o, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    o += 2;
  }
  return buf;
}

function rms(samples: Float32Array): number {
  let sum = 0;
  for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
  return Math.sqrt(sum / samples.length);
}

function mergeSamples(chunks: Float32Array[]): Float32Array {
  const total = chunks.reduce((length, chunk) => length + chunk.length, 0);
  const merged = new Float32Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }
  return merged;
}

function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onloadend = () => resolve((r.result as string).split(',')[1]);
    r.onerror = reject;
    r.readAsDataURL(blob);
  });
}

export const useAudioRecorder = ({ onTranscription, onError }: UseAudioRecorderOptions) => {
  const [isRecording, setIsRecording] = useState(false);
  const [isTranscribing, setIsTranscribing] = useState(false);
  const [isEnabled, setIsEnabled] = useState(false);
  const [provider, setProvider] = useState<DictationProvider | null>(null);

  const { read, config } = useConfig();

  const activeGenerationRef = useRef<RecorderGeneration | null>(null);
  const mountedRef = useRef(true);

  // VAD state (all refs to avoid re-render/stale closure issues)
  const samplesRef = useRef<Float32Array[]>([]);
  const isSpeakingRef = useRef(false);
  const silenceStartRef = useRef(0);
  const speechStartRef = useRef(0);
  const providerRef = useRef(provider);
  providerRef.current = provider;

  // Keep callback refs fresh
  const onTranscriptionRef = useRef(onTranscription);
  onTranscriptionRef.current = onTranscription;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  useEffect(() => {
    const check = async () => {
      try {
        const val = await read('voice_dictation_provider', false);
        const pref = (val as DictationProvider) || null;
        if (!pref) {
          setIsEnabled(false);
          setProvider(null);
          return;
        }
        const providers = await getDictationConfig();
        setIsEnabled(!!providers[pref]?.configured);
        setProvider(pref);
      } catch (error) {
        console.error('Failed to check dictation config:', error);
        setIsEnabled(false);
        setProvider(null);
      }
    };
    check();
  }, [read, config]);

  const resetSpeech = useCallback(() => {
    samplesRef.current = [];
    isSpeakingRef.current = false;
    silenceStartRef.current = 0;
    speechStartRef.current = 0;
  }, []);

  const isActiveGeneration = useCallback(
    (generation: RecorderGeneration) =>
      mountedRef.current && activeGenerationRef.current === generation && !generation.cancelled,
    []
  );

  const transcribeChunk = useCallback(
    async (samples: Float32Array, generation: RecorderGeneration) => {
      const prov = providerRef.current;
      if (!prov || !isActiveGeneration(generation)) return;

      generation.pendingTranscriptions++;
      setIsTranscribing(true);

      try {
        const wav = new Blob([encodeWav(samples, SAMPLE_RATE)], { type: 'audio/wav' });
        const base64 = await blobToBase64(wav);
        if (!isActiveGeneration(generation)) return;

        const text = await transcribeDictation(base64, 'audio/wav', prov);
        if (text && isActiveGeneration(generation)) {
          onTranscriptionRef.current(text);
        }
      } catch (error) {
        if (isActiveGeneration(generation)) {
          onErrorRef.current(errorMessage(error));
        }
      } finally {
        generation.pendingTranscriptions--;
        if (generation.pendingTranscriptions === 0 && isActiveGeneration(generation)) {
          setIsTranscribing(false);
          if (generation.completesAfterTranscription) {
            activeGenerationRef.current = null;
            generation.cancelled = true;
          }
        }
      }
    },
    [isActiveGeneration]
  );

  const flush = useCallback(
    (generation: RecorderGeneration) => {
      if (!isActiveGeneration(generation)) return;

      const chunks = samplesRef.current;
      if (chunks.length === 0) return;

      samplesRef.current = [];
      void transcribeChunk(mergeSamples(chunks), generation);
    },
    [isActiveGeneration, transcribeChunk]
  );

  const handleSamples = useCallback(
    (samples: Float32Array, generation: RecorderGeneration) => {
      if (!isActiveGeneration(generation)) return;

      const now = Date.now();

      if (rms(samples) > RMS_THRESHOLD) {
        if (!isSpeakingRef.current) {
          isSpeakingRef.current = true;
          speechStartRef.current = now;
        }
        silenceStartRef.current = 0;
        samplesRef.current.push(new Float32Array(samples));
      } else if (isSpeakingRef.current) {
        samplesRef.current.push(new Float32Array(samples));

        if (silenceStartRef.current === 0) {
          silenceStartRef.current = now;
        } else if (now - silenceStartRef.current > SILENCE_MS) {
          if (now - speechStartRef.current > MIN_SPEECH_MS) {
            flush(generation);
          } else {
            samplesRef.current = [];
          }
          isSpeakingRef.current = false;
          silenceStartRef.current = 0;
        }
      }
    },
    [flush, isActiveGeneration]
  );

  const cancelActiveGeneration = useCallback(() => {
    const generation = activeGenerationRef.current;
    activeGenerationRef.current = null;
    if (generation) cleanupGeneration(generation);
    resetSpeech();

    if (mountedRef.current) {
      setIsRecording(false);
      setIsTranscribing(false);
    }
  }, [resetSpeech]);

  const stopRecording = useCallback(() => {
    const finalChunks = isSpeakingRef.current ? samplesRef.current : [];
    cancelActiveGeneration();

    if (mountedRef.current && finalChunks.length > 0) {
      const finalGeneration: RecorderGeneration = {
        audioContext: null,
        cancelled: false,
        completesAfterTranscription: true,
        pendingTranscriptions: 0,
        stream: null,
        worklet: null,
      };
      activeGenerationRef.current = finalGeneration;
      void transcribeChunk(mergeSamples(finalChunks), finalGeneration);
    }
  }, [cancelActiveGeneration, transcribeChunk]);

  const startRecording = useCallback(async () => {
    if (!isEnabled) {
      onErrorRef.current('Voice dictation is not enabled');
      return;
    }

    cancelActiveGeneration();
    const generation: RecorderGeneration = {
      audioContext: null,
      cancelled: false,
      completesAfterTranscription: false,
      pendingTranscriptions: 0,
      stream: null,
      worklet: null,
    };
    activeGenerationRef.current = generation;

    try {
      const preferredMic = await read('voice_dictation_preferred_mic', false);
      if (!isActiveGeneration(generation)) return;

      const audioConstraints: MediaTrackConstraints = {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      };
      if (preferredMic && typeof preferredMic === 'string') {
        audioConstraints.deviceId = { exact: preferredMic };
      }

      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: audioConstraints });
      } catch (e) {
        if (!isActiveGeneration(generation)) return;
        if (
          preferredMic &&
          e instanceof DOMException &&
          (e.name === 'NotFoundError' || e.name === 'OverconstrainedError')
        ) {
          delete audioConstraints.deviceId;
          stream = await navigator.mediaDevices.getUserMedia({ audio: audioConstraints });
        } else {
          throw e;
        }
      }
      generation.stream = stream;
      if (!isActiveGeneration(generation)) {
        cleanupGeneration(generation);
        return;
      }

      const ctx = new AudioContext({ sampleRate: SAMPLE_RATE });
      generation.audioContext = ctx;

      await ctx.audioWorklet.addModule(WORKLET_URL);
      if (!isActiveGeneration(generation)) {
        cleanupGeneration(generation);
        return;
      }

      const source = ctx.createMediaStreamSource(stream);
      const worklet = new AudioWorkletNode(ctx, 'audio-capture');
      generation.worklet = worklet;

      worklet.port.onmessage = (e: MessageEvent<Float32Array>) => handleSamples(e.data, generation);

      // Connect through silent gain to keep worklet processing alive
      const silence = ctx.createGain();
      silence.gain.value = 0;
      source.connect(worklet);
      worklet.connect(silence);
      silence.connect(ctx.destination);

      if (isActiveGeneration(generation)) {
        setIsRecording(true);
      }
    } catch (error) {
      const isCurrent = isActiveGeneration(generation);
      if (activeGenerationRef.current === generation) {
        activeGenerationRef.current = null;
      }
      cleanupGeneration(generation);
      if (isCurrent) {
        resetSpeech();
        setIsRecording(false);
        setIsTranscribing(false);
        onErrorRef.current(errorMessage(error));
      }
    }
  }, [cancelActiveGeneration, handleSamples, isActiveGeneration, isEnabled, read, resetSpeech]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      cancelActiveGeneration();
    };
  }, [cancelActiveGeneration]);

  return {
    isEnabled,
    dictationProvider: provider,
    isRecording,
    isTranscribing,
    startRecording,
    stopRecording,
  };
};

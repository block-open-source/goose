import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  config: {},
  getDictationConfig: vi.fn(),
  onError: vi.fn(),
  onTranscription: vi.fn(),
  read: vi.fn(),
  transcribeDictation: vi.fn(),
}));

vi.mock('../components/ConfigContext', () => ({
  useConfig: () => ({ config: mocks.config, read: mocks.read }),
}));

vi.mock('../acp/dictation', () => ({
  getDictationConfig: mocks.getDictationConfig,
  transcribeDictation: mocks.transcribeDictation,
}));

import { useAudioRecorder } from './useAudioRecorder';

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
};

type FakeStream = MediaStream & {
  track: { stop: ReturnType<typeof vi.fn> };
};

const deferred = <T,>(): Deferred<T> => {
  let resolve = (_value: T) => {};
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
};

const createStream = (): FakeStream => {
  const track = { stop: vi.fn() };
  return {
    track,
    getTracks: () => [track],
  } as unknown as FakeStream;
};

let moduleLoads: Deferred<void>[];
let audioContexts: MockAudioContext[];
let worklets: MockAudioWorkletNode[];

class MockAudioContext {
  audioWorklet: { addModule: ReturnType<typeof vi.fn> };
  close = vi.fn(() => Promise.resolve());
  createGain = vi.fn(() => ({
    connect: vi.fn(),
    gain: { value: 1 },
  }));
  createMediaStreamSource = vi.fn(() => ({ connect: vi.fn() }));
  destination = {};

  constructor() {
    const moduleLoad = moduleLoads.shift();
    this.audioWorklet = {
      addModule: vi.fn(() => moduleLoad?.promise ?? Promise.resolve()),
    };
    audioContexts.push(this);
  }
}

class MockAudioWorkletNode {
  connect = vi.fn();
  disconnect = vi.fn();
  port: { onmessage: ((event: MessageEvent<Float32Array>) => void) | null } = {
    onmessage: null,
  };

  constructor() {
    worklets.push(this);
  }

  emit(samples: Float32Array) {
    this.port.onmessage?.({ data: samples } as MessageEvent<Float32Array>);
  }
}

const renderRecorder = async () => {
  const hook = renderHook(() =>
    useAudioRecorder({
      onError: mocks.onError,
      onTranscription: mocks.onTranscription,
    })
  );
  await waitFor(() => expect(hook.result.current.isEnabled).toBe(true));
  return hook;
};

const startRecorder = async (startRecording: () => Promise<void>) => {
  await act(async () => {
    await startRecording();
  });
};

const emitSpeechChunk = (worklet: MockAudioWorkletNode) => {
  let now = 100;
  vi.spyOn(Date, 'now').mockImplementation(() => now);
  const speech = new Float32Array(3200).fill(0.1);
  const silence = new Float32Array(3200);

  act(() => {
    worklet.emit(speech);
    now = 400;
    worklet.emit(silence);
    now = 1301;
    worklet.emit(silence);
  });
};

describe('useAudioRecorder lifecycle', () => {
  let getUserMedia: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    moduleLoads = [];
    audioContexts = [];
    worklets = [];
    getUserMedia = vi.fn();

    mocks.getDictationConfig.mockReset().mockResolvedValue({
      openai: { configured: true },
    });
    mocks.onError.mockReset();
    mocks.onTranscription.mockReset();
    mocks.read
      .mockReset()
      .mockImplementation((key: string) =>
        Promise.resolve(key === 'voice_dictation_provider' ? 'openai' : null)
      );
    mocks.transcribeDictation.mockReset();

    vi.stubGlobal('AudioContext', MockAudioContext);
    vi.stubGlobal('AudioWorkletNode', MockAudioWorkletNode);
    Object.defineProperty(navigator, 'mediaDevices', {
      configurable: true,
      value: { getUserMedia },
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('cleans the superseded generation when starts overlap', async () => {
    const firstStream = createStream();
    const secondStream = createStream();
    const firstModuleLoad = deferred<void>();
    moduleLoads.push(firstModuleLoad);
    getUserMedia.mockResolvedValueOnce(firstStream).mockResolvedValueOnce(secondStream);
    const { result } = await renderRecorder();

    let firstStart = Promise.resolve();
    act(() => {
      firstStart = result.current.startRecording();
    });
    await waitFor(() => expect(audioContexts).toHaveLength(1));

    await startRecorder(result.current.startRecording);

    expect(firstStream.track.stop).toHaveBeenCalledOnce();
    expect(audioContexts[0].close).toHaveBeenCalledOnce();
    expect(worklets).toHaveLength(1);
    expect(result.current.isRecording).toBe(true);

    await act(async () => {
      firstModuleLoad.resolve();
      await firstStart;
    });

    expect(worklets).toHaveLength(1);
    act(() => result.current.stopRecording());
    expect(secondStream.track.stop).toHaveBeenCalledOnce();
    expect(audioContexts[1].close).toHaveBeenCalledOnce();
    expect(worklets[0].disconnect).toHaveBeenCalledOnce();
  });

  it('stops a stream that arrives after recording is stopped', async () => {
    const pendingStream = deferred<MediaStream>();
    const stream = createStream();
    getUserMedia.mockReturnValueOnce(pendingStream.promise);
    const { result } = await renderRecorder();

    let start = Promise.resolve();
    act(() => {
      start = result.current.startRecording();
    });
    await waitFor(() => expect(getUserMedia).toHaveBeenCalledOnce());
    act(() => result.current.stopRecording());

    await act(async () => {
      pendingStream.resolve(stream);
      await start;
    });

    expect(stream.track.stop).toHaveBeenCalledOnce();
    expect(audioContexts).toHaveLength(0);
    expect(worklets).toHaveLength(0);
    expect(result.current.isRecording).toBe(false);
    expect(mocks.onError).not.toHaveBeenCalled();
  });

  it('does not finish startup after unmount', async () => {
    const stream = createStream();
    const moduleLoad = deferred<void>();
    moduleLoads.push(moduleLoad);
    getUserMedia.mockResolvedValueOnce(stream);
    const { result, unmount } = await renderRecorder();

    let start = Promise.resolve();
    act(() => {
      start = result.current.startRecording();
    });
    await waitFor(() => expect(audioContexts).toHaveLength(1));
    unmount();

    expect(stream.track.stop).toHaveBeenCalledOnce();
    expect(audioContexts[0].close).toHaveBeenCalledOnce();

    await act(async () => {
      moduleLoad.resolve();
      await start;
    });

    expect(worklets).toHaveLength(0);
    expect(mocks.onError).not.toHaveBeenCalled();
  });

  it('suppresses an in-flight transcription after stop', async () => {
    const stream = createStream();
    const transcription = deferred<string>();
    getUserMedia.mockResolvedValueOnce(stream);
    mocks.transcribeDictation.mockReturnValueOnce(transcription.promise);
    const { result } = await renderRecorder();
    await startRecorder(result.current.startRecording);

    emitSpeechChunk(worklets[0]);
    await waitFor(() => expect(mocks.transcribeDictation).toHaveBeenCalledOnce());
    act(() => result.current.stopRecording());

    await act(async () => {
      transcription.resolve('stale transcription');
      await transcription.promise;
      await Promise.resolve();
    });

    expect(mocks.onTranscription).not.toHaveBeenCalled();
  });

  it('transcribes the buffered final phrase when recording is stopped', async () => {
    const stream = createStream();
    getUserMedia.mockResolvedValueOnce(stream);
    mocks.transcribeDictation.mockResolvedValueOnce('final phrase');
    const { result } = await renderRecorder();
    await startRecorder(result.current.startRecording);

    act(() => {
      worklets[0].emit(new Float32Array(3200).fill(0.1));
      result.current.stopRecording();
    });

    await waitFor(() => expect(mocks.onTranscription).toHaveBeenCalledWith('final phrase'));
    await waitFor(() => expect(result.current.isTranscribing).toBe(false));
    expect(stream.track.stop).toHaveBeenCalledOnce();
    expect(audioContexts[0].close).toHaveBeenCalledOnce();
    expect(worklets[0].disconnect).toHaveBeenCalledOnce();
  });

  it('cancels final-phrase transcription when a new recording starts', async () => {
    const firstStream = createStream();
    const secondStream = createStream();
    const transcription = deferred<string>();
    getUserMedia.mockResolvedValueOnce(firstStream).mockResolvedValueOnce(secondStream);
    mocks.transcribeDictation.mockReturnValueOnce(transcription.promise);
    const { result } = await renderRecorder();
    await startRecorder(result.current.startRecording);

    act(() => {
      worklets[0].emit(new Float32Array(3200).fill(0.1));
      result.current.stopRecording();
    });
    await waitFor(() => expect(mocks.transcribeDictation).toHaveBeenCalledOnce());

    await startRecorder(result.current.startRecording);
    await act(async () => {
      transcription.resolve('stale final phrase');
      await transcription.promise;
      await Promise.resolve();
    });

    expect(mocks.onTranscription).not.toHaveBeenCalled();
    expect(result.current.isRecording).toBe(true);
    act(() => result.current.stopRecording());
  });

  it('cancels final-phrase transcription when unmounted', async () => {
    const stream = createStream();
    const transcription = deferred<string>();
    getUserMedia.mockResolvedValueOnce(stream);
    mocks.transcribeDictation.mockReturnValueOnce(transcription.promise);
    const { result, unmount } = await renderRecorder();
    await startRecorder(result.current.startRecording);

    act(() => {
      worklets[0].emit(new Float32Array(3200).fill(0.1));
      result.current.stopRecording();
    });
    await waitFor(() => expect(mocks.transcribeDictation).toHaveBeenCalledOnce());
    unmount();

    await act(async () => {
      transcription.resolve('stale final phrase');
      await transcription.promise;
      await Promise.resolve();
    });

    expect(mocks.onTranscription).not.toHaveBeenCalled();
  });

  it('records and transcribes a normal generation', async () => {
    const stream = createStream();
    getUserMedia.mockResolvedValueOnce(stream);
    mocks.transcribeDictation.mockResolvedValueOnce('hello world');
    const { result } = await renderRecorder();

    await startRecorder(result.current.startRecording);
    expect(result.current.isRecording).toBe(true);

    emitSpeechChunk(worklets[0]);
    await waitFor(() => expect(mocks.onTranscription).toHaveBeenCalledWith('hello world'));
    await waitFor(() => expect(result.current.isTranscribing).toBe(false));

    act(() => result.current.stopRecording());
    expect(result.current.isRecording).toBe(false);
    expect(stream.track.stop).toHaveBeenCalledOnce();
    expect(audioContexts[0].close).toHaveBeenCalledOnce();
    expect(worklets[0].disconnect).toHaveBeenCalledOnce();
  });
});

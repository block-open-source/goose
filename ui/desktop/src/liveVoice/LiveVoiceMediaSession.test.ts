import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LiveVoiceMediaSession } from './LiveVoiceMediaSession';

const createElement = document.createElement.bind(document);

class FakeTrack {
  enabled = true;
  stop = vi.fn();
}

class FakeStream {
  constructor(private readonly tracks: FakeTrack[]) {}
  getTracks = () => this.tracks;
  getAudioTracks = () => this.tracks;
}

class FakeDataChannel extends EventTarget {
  readyState: RTCDataChannelState = 'open';
  close = vi.fn();
}

class FakePeerConnection extends EventTarget {
  iceGatheringState: RTCIceGatheringState = 'complete';
  connectionState: RTCPeerConnectionState = 'connected';
  localDescription: RTCSessionDescriptionInit | null = null;
  ontrack: ((event: RTCTrackEvent) => void) | null = null;
  dataChannel = new FakeDataChannel();
  close = vi.fn();
  addTrack = vi.fn();
  createDataChannel = vi.fn(() => this.dataChannel as unknown as RTCDataChannel);
  createOffer = vi.fn(async () => ({ type: 'offer' as const, sdp: 'bounded-offer' }));
  setLocalDescription = vi.fn(async (description: RTCSessionDescriptionInit) => {
    this.localDescription = description;
  });
  setRemoteDescription = vi.fn(async () => {
    this.ontrack?.({
      streams: [new FakeStream([new FakeTrack()])],
      track: new FakeTrack(),
    } as unknown as RTCTrackEvent);
  });
}

describe('LiveVoiceMediaSession', () => {
  let localTrack: FakeTrack;
  let peerConnection: FakePeerConnection;

  beforeEach(() => {
    localTrack = new FakeTrack();
    peerConnection = new FakePeerConnection();
    vi.stubGlobal('MediaStream', FakeStream);
    vi.stubGlobal(
      'RTCPeerConnection',
      vi.fn(function RTCPeerConnectionMock() {
        return peerConnection;
      })
    );
    Object.defineProperty(navigator, 'mediaDevices', {
      configurable: true,
      value: {
        getUserMedia: vi.fn(async () => new FakeStream([localTrack])),
      },
    });
    vi.spyOn(document, 'createElement').mockImplementation(((tagName: string) => {
      const element = createElement(tagName);
      if (tagName === 'audio') {
        vi.spyOn(element as HTMLAudioElement, 'play').mockResolvedValue(undefined);
        vi.spyOn(element as HTMLAudioElement, 'pause').mockImplementation(() => undefined);
      }
      return element;
    }) as typeof document.createElement);
  });

  it('keeps the microphone disabled until media is connected, then tears down once', async () => {
    const media = new LiveVoiceMediaSession();

    await expect(media.createOffer()).resolves.toBe('bounded-offer');
    expect(localTrack.enabled).toBe(false);
    expect(peerConnection.createDataChannel).toHaveBeenCalledWith('oai-events');

    await media.applyAnswer('bounded-answer');
    media.setMuted(false);
    expect(localTrack.enabled).toBe(true);

    media.setMuted(true);
    media.setMuted(false);
    media.setMuted(true);
    expect(localTrack.enabled).toBe(false);

    media.teardown();
    media.teardown();
    expect(localTrack.stop).toHaveBeenCalledOnce();
    expect(peerConnection.dataChannel.close).toHaveBeenCalledOnce();
    expect(peerConnection.close).toHaveBeenCalledOnce();
  });

  it('returns a product-safe error when microphone acquisition fails', async () => {
    vi.mocked(navigator.mediaDevices.getUserMedia).mockRejectedValueOnce(
      new Error('native device identifier')
    );

    await expect(new LiveVoiceMediaSession().createOffer()).rejects.toThrow(
      'Microphone or media setup failed'
    );
  });

  it('stops a microphone stream acquired after teardown', async () => {
    let provideStream!: (stream: MediaStream) => void;
    vi.mocked(navigator.mediaDevices.getUserMedia).mockReturnValueOnce(
      new Promise((resolve) => {
        provideStream = resolve;
      })
    );
    const lateTrack = new FakeTrack();
    const media = new LiveVoiceMediaSession();

    const offer = media.createOffer();
    media.teardown();
    provideStream(new FakeStream([lateTrack]) as unknown as MediaStream);

    await expect(offer).rejects.toThrow('Microphone or media setup failed');
    expect(lateTrack.stop).toHaveBeenCalledOnce();
    expect(peerConnection.createOffer).not.toHaveBeenCalled();
  });
});

const MEDIA_SETUP_TIMEOUT_MS = 15_000;

export class LiveVoiceMediaSession {
  private peerConnection: RTCPeerConnection | null = null;
  private localStream: MediaStream | null = null;
  private remoteStream: MediaStream | null = null;
  private dataChannel: RTCDataChannel | null = null;
  private audioElement: HTMLAudioElement | null = null;
  private playbackStarted: Promise<void> | null = null;
  private tornDown = false;

  constructor(private readonly onMediaFailure: () => void) {}

  async createOffer(): Promise<string> {
    try {
      const localStream = await navigator.mediaDevices.getUserMedia({ audio: true });
      if (this.tornDown) {
        for (const track of localStream.getTracks()) {
          track.enabled = false;
          track.stop();
        }
        throw new Error('Live voice media is unavailable');
      }

      this.localStream = localStream;
      for (const track of localStream.getAudioTracks()) {
        track.enabled = false;
      }

      const peerConnection = new RTCPeerConnection();
      this.peerConnection = peerConnection;
      this.audioElement = document.createElement('audio');
      this.audioElement.autoplay = true;
      this.dataChannel = peerConnection.createDataChannel('oai-events');
      peerConnection.ontrack = (event) => {
        const stream = event.streams[0] ?? new MediaStream([event.track]);
        this.remoteStream = stream;
        if (this.audioElement) {
          this.audioElement.srcObject = stream;
          this.playbackStarted = this.audioElement.play();
          void this.playbackStarted.catch(() => undefined);
        }
      };
      for (const track of localStream.getTracks()) {
        peerConnection.addTrack(track, localStream);
      }

      const offer = await peerConnection.createOffer();
      await peerConnection.setLocalDescription(offer);
      await waitForIceGathering(peerConnection);
      const offerSdp = peerConnection.localDescription?.sdp;
      if (!offerSdp) {
        throw new Error('Live voice could not create a media offer');
      }
      return offerSdp;
    } catch {
      this.teardown();
      throw new Error('Microphone or media setup failed');
    }
  }

  async applyAnswer(answerSdp: string): Promise<void> {
    const peerConnection = this.peerConnection;
    const dataChannel = this.dataChannel;
    if (!peerConnection || !dataChannel || this.tornDown) {
      throw new Error('Live voice media is unavailable');
    }

    try {
      const sessionStarted = waitForSessionStarted(dataChannel);
      try {
        await peerConnection.setRemoteDescription({ type: 'answer', sdp: answerSdp });
        await Promise.all([
          waitForPeerConnection(peerConnection),
          waitForDataChannel(dataChannel),
          waitForRemoteTrack(this),
          sessionStarted.promise,
        ]);
        await this.playbackStarted;
        this.observeMediaFailures(peerConnection);
      } finally {
        sessionStarted.cancel();
      }
    } catch {
      this.teardown();
      throw new Error('Live voice could not connect media');
    }
  }

  setMuted(muted: boolean): void {
    if (this.tornDown) return;
    for (const track of this.localStream?.getAudioTracks() ?? []) {
      track.enabled = !muted;
    }
  }

  sendCommentary(text: string): void {
    const content = text.trim();
    if (this.tornDown || !content || this.dataChannel?.readyState !== 'open') return;

    try {
      this.dataChannel.send(
        JSON.stringify({
          type: 'session.commentary.append',
          event_id: `event_${crypto.randomUUID()}`,
          delegation_id: null,
          content,
        })
      );
    } catch {
      // Greeting failure must not end an otherwise healthy Live call.
    }
  }

  hasRemoteTrack(): boolean {
    return this.remoteStream !== null;
  }

  teardown(): void {
    if (this.tornDown) return;
    this.tornDown = true;
    for (const track of this.localStream?.getTracks() ?? []) {
      track.enabled = false;
      track.stop();
    }
    for (const track of this.remoteStream?.getTracks() ?? []) {
      track.stop();
    }
    this.dataChannel?.close();
    this.peerConnection?.close();
    if (this.audioElement) {
      this.audioElement.pause();
      this.audioElement.srcObject = null;
    }
    this.localStream = null;
    this.remoteStream = null;
    this.dataChannel = null;
    this.peerConnection = null;
    this.audioElement = null;
    this.playbackStarted = null;
  }

  private observeMediaFailures(peerConnection: RTCPeerConnection): void {
    for (const track of this.localStream?.getAudioTracks() ?? []) {
      track.addEventListener('ended', this.reportMediaFailure, { once: true });
    }
    peerConnection.addEventListener('connectionstatechange', () => {
      if (peerConnection.connectionState === 'failed') {
        this.reportMediaFailure();
      }
    });
  }

  private reportMediaFailure = (): void => {
    if (this.tornDown) return;
    this.onMediaFailure();
  };
}

function waitForIceGathering(peerConnection: RTCPeerConnection): Promise<void> {
  if (peerConnection.iceGatheringState === 'complete') return Promise.resolve();
  return waitForEvent(
    peerConnection,
    'icegatheringstatechange',
    () => peerConnection.iceGatheringState === 'complete'
  );
}

function waitForPeerConnection(peerConnection: RTCPeerConnection): Promise<void> {
  if (peerConnection.connectionState === 'connected') return Promise.resolve();
  return waitForEvent(peerConnection, 'connectionstatechange', () => {
    if (
      peerConnection.connectionState === 'failed' ||
      peerConnection.connectionState === 'closed'
    ) {
      throw new Error('Peer connection failed');
    }
    return peerConnection.connectionState === 'connected';
  });
}

function waitForDataChannel(dataChannel: RTCDataChannel): Promise<void> {
  if (dataChannel.readyState === 'open') return Promise.resolve();
  return waitForEvent(dataChannel, 'open', () => true);
}

function waitForSessionStarted(dataChannel: RTCDataChannel): {
  promise: Promise<void>;
  cancel: () => void;
} {
  let cancel: () => void = () => undefined;
  const promise = new Promise<void>((resolve, reject) => {
    const timeoutId = setTimeout(
      () => finish(new Error('Live session startup timed out')),
      MEDIA_SETUP_TIMEOUT_MS
    );
    const listener = (event: MessageEvent) => {
      if (typeof event.data !== 'string') return;
      let message: { type?: unknown };
      try {
        message = JSON.parse(event.data) as { type?: unknown };
      } catch {
        return;
      }
      if (message.type === 'session.started') {
        finish();
      } else if (message.type === 'error') {
        finish(new Error('Live session failed to start'));
      }
    };
    const finish = (error?: Error) => {
      clearTimeout(timeoutId);
      dataChannel.removeEventListener('message', listener);
      if (error) {
        reject(error);
      } else {
        resolve();
      }
    };
    cancel = finish;
    dataChannel.addEventListener('message', listener);
  });
  return { promise, cancel };
}

function waitForRemoteTrack(media: LiveVoiceMediaSession): Promise<void> {
  if (media.hasRemoteTrack()) return Promise.resolve();
  return pollUntil(() => media.hasRemoteTrack());
}

function waitForEvent(
  target: EventTarget,
  eventName: string,
  isReady: () => boolean
): Promise<void> {
  return new Promise((resolve, reject) => {
    const timeoutId = setTimeout(
      () => finish(new Error('Media setup timed out')),
      MEDIA_SETUP_TIMEOUT_MS
    );
    const listener = () => {
      try {
        if (isReady()) finish();
      } catch (error) {
        finish(error instanceof Error ? error : new Error('Media setup failed'));
      }
    };
    const finish = (error?: Error) => {
      clearTimeout(timeoutId);
      target.removeEventListener(eventName, listener);
      if (error) {
        reject(error);
      } else {
        resolve();
      }
    };
    target.addEventListener(eventName, listener);
  });
}

function pollUntil(isReady: () => boolean): Promise<void> {
  return new Promise((resolve, reject) => {
    const startedAt = Date.now();
    const poll = () => {
      if (isReady()) {
        resolve();
      } else if (Date.now() - startedAt >= MEDIA_SETUP_TIMEOUT_MS) {
        reject(new Error('Media setup timed out'));
      } else {
        setTimeout(poll, 25);
      }
    };
    poll();
  });
}

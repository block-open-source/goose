import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GOOSE_SERVE_EXITED_USER_MESSAGE } from '../../gooseServeLeaseRegistry';

const mockClientFactory = vi.hoisted(() => {
  const initialize = vi.fn();
  type MockStream = object;
  type MockClient = {
    connection: {
      agent: { request: typeof initialize };
      closed: Promise<void>;
      close: ReturnType<typeof vi.fn>;
    };
    goose: Record<string, never>;
  };
  const instances: Array<{ client: MockClient; resolveClosed: () => void }> = [];
  const connectGooseAcpClient = vi.fn((_stream: MockStream): MockClient => {
    let resolveClosed: () => void = () => undefined;
    const closed = new Promise<void>((resolve) => {
      resolveClosed = resolve;
    });
    const client: MockClient = {
      connection: {
        agent: { request: initialize },
        closed,
        close: vi.fn(),
      },
      goose: {},
    };
    instances.push({ client, resolveClosed });
    return client;
  });

  return { connectGooseAcpClient, initialize, instances };
});

const transport = vi.hoisted(() => ({
  createWebSocketStream: vi.fn(),
}));

vi.mock('@aaif/goose-acp-client', () => ({
  DEFAULT_GOOSE_MCP_HOST_CAPABILITIES: {},
}));

vi.mock('../gooseAcpClient', () => ({
  connectGooseAcpClient: mockClientFactory.connectGooseAcpClient,
}));

vi.mock('@agentclientprotocol/sdk/experimental/ws-client', () => ({
  createWebSocketStream: transport.createWebSocketStream,
}));

describe('ACP connection ownership', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetModules();
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    mockClientFactory.initialize.mockReset().mockResolvedValue({});
    mockClientFactory.instances.length = 0;
    transport.createWebSocketStream.mockReset().mockImplementation(() => ({
      readable: {},
      writable: {},
    }));
    window.electron.getAcpUrl = vi.fn().mockResolvedValue('ws://localhost/acp');
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('shares one initialization between concurrent callers', async () => {
    const { getAcpClient } = await import('../acpConnection');

    const [first, second] = await Promise.all([getAcpClient(), getAcpClient()]);

    expect(first).toBe(second);
    expect(mockClientFactory.instances).toHaveLength(1);
    expect(mockClientFactory.initialize).toHaveBeenCalledTimes(1);
    expect(mockClientFactory.initialize).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ protocolVersion: 1 })
    );
    expect(transport.createWebSocketStream).toHaveBeenCalledWith('ws://localhost/acp', {
      protocols: [],
    });
  });

  it('automatically reconnects after close and shares the result between callers', async () => {
    const { getAcpClient } = await import('../acpConnection');
    const firstClient = await getAcpClient();
    mockClientFactory.instances[0].resolveClosed();
    await Promise.resolve();
    const firstCaller = getAcpClient();
    const secondCaller = getAcpClient();

    await vi.advanceTimersByTimeAsync(249);
    expect(mockClientFactory.instances).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(1);
    const [firstResult, secondResult] = await Promise.all([firstCaller, secondCaller]);

    expect(mockClientFactory.instances[0].client.connection.close).toHaveBeenCalledOnce();
    expect(firstResult).toBe(secondResult);
    expect(firstResult).not.toBe(firstClient);
    expect(mockClientFactory.instances).toHaveLength(2);
    expect(transport.createWebSocketStream).toHaveBeenCalledTimes(2);
  });

  it('increases the backoff after a failed reconnect attempt', async () => {
    mockClientFactory.initialize
      .mockResolvedValueOnce({})
      .mockRejectedValueOnce(new Error('server unavailable'))
      .mockResolvedValueOnce({});
    const { getAcpClient } = await import('../acpConnection');
    await getAcpClient();

    mockClientFactory.instances[0].resolveClosed();
    await Promise.resolve();
    const reconnected = getAcpClient();

    await vi.advanceTimersByTimeAsync(250);
    expect(mockClientFactory.instances).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(499);
    expect(mockClientFactory.instances).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(1);
    await reconnected;

    expect(mockClientFactory.instances).toHaveLength(3);
  });

  it('stops reconnecting when the Goose backend has exited', async () => {
    const { getAcpClient, subscribeToAcpRecovery } = await import('../acpConnection');
    const listener = vi.fn();
    subscribeToAcpRecovery(listener);
    await getAcpClient();

    const getAcpUrl = vi
      .fn()
      .mockRejectedValue(
        new Error(`Error invoking remote method 'get-acp-url': ${GOOSE_SERVE_EXITED_USER_MESSAGE}`)
      );
    window.electron.getAcpUrl = getAcpUrl;
    mockClientFactory.instances[0].resolveClosed();
    await Promise.resolve();

    const connection = expect(getAcpClient()).rejects.toThrow(GOOSE_SERVE_EXITED_USER_MESSAGE);
    await vi.advanceTimersByTimeAsync(250);
    await connection;

    expect(listener.mock.calls).toEqual([[true], [false]]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(getAcpUrl).toHaveBeenCalledOnce();
  });

  it('does not cache terminal recovery failures across caller retries', async () => {
    const { getAcpClient } = await import('../acpConnection');
    await getAcpClient();

    const getAcpUrl = vi
      .fn()
      .mockRejectedValue(
        new Error(`Error invoking remote method 'get-acp-url': ${GOOSE_SERVE_EXITED_USER_MESSAGE}`)
      );
    window.electron.getAcpUrl = getAcpUrl;
    mockClientFactory.instances[0].resolveClosed();
    await Promise.resolve();

    const failedRecovery = expect(getAcpClient()).rejects.toThrow(GOOSE_SERVE_EXITED_USER_MESSAGE);
    await vi.advanceTimersByTimeAsync(250);
    await failedRecovery;

    await expect(getAcpClient()).rejects.toThrow(GOOSE_SERVE_EXITED_USER_MESSAGE);

    expect(getAcpUrl).toHaveBeenCalledTimes(2);
    expect(mockClientFactory.instances).toHaveLength(1);
  });

  it('reconnects immediately after system resume', async () => {
    const { getAcpClient, reconnectAcpAfterSystemResume } = await import('../acpConnection');
    await getAcpClient();
    reconnectAcpAfterSystemResume();
    const reconnected = getAcpClient();
    await reconnected;

    expect(mockClientFactory.instances[0].client.connection.close).toHaveBeenCalledOnce();
    expect(mockClientFactory.instances).toHaveLength(2);
  });

  it('does nothing on system resume before ACP has been used', async () => {
    const { reconnectAcpAfterSystemResume } = await import('../acpConnection');

    reconnectAcpAfterSystemResume();
    await Promise.resolve();

    expect(mockClientFactory.instances).toHaveLength(0);
    expect(transport.createWebSocketStream).not.toHaveBeenCalled();
  });

  it('uses normal backoff when the immediate resume attempt fails', async () => {
    mockClientFactory.initialize
      .mockResolvedValueOnce({})
      .mockRejectedValueOnce(new Error('network is not ready'))
      .mockResolvedValueOnce({});
    const { getAcpClient, reconnectAcpAfterSystemResume } = await import('../acpConnection');
    await getAcpClient();

    reconnectAcpAfterSystemResume();
    const reconnected = getAcpClient();
    await Promise.resolve();
    expect(mockClientFactory.instances).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(249);
    expect(mockClientFactory.instances).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(1);
    await reconnected;

    expect(mockClientFactory.instances).toHaveLength(3);
  });

  it('supersedes an older retry loop after system resume', async () => {
    const { getAcpClient, reconnectAcpAfterSystemResume } = await import('../acpConnection');
    await getAcpClient();

    mockClientFactory.instances[0].resolveClosed();
    await Promise.resolve();
    reconnectAcpAfterSystemResume();
    await getAcpClient();
    expect(mockClientFactory.instances).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(250);

    expect(mockClientFactory.instances).toHaveLength(2);
  });

  it('notifies subscribers while reconnecting and after recovery', async () => {
    const { getAcpClient, subscribeToAcpRecovery } = await import('../acpConnection');
    const listener = vi.fn();
    subscribeToAcpRecovery(listener);
    await getAcpClient();

    mockClientFactory.instances[0].resolveClosed();
    await Promise.resolve();
    expect(listener).toHaveBeenCalledWith(true);

    await vi.advanceTimersByTimeAsync(250);
    await getAcpClient();

    expect(listener).toHaveBeenLastCalledWith(false);
  });
});

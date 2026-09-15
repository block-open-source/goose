import { methods, type InitializeResponse, type SessionInfo } from '@agentclientprotocol/sdk';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAcpClient, getAcpConnection } from '../acpConnection';
import {
  acpGetSessionListItem,
  acpListSessions,
  acpLoadSession,
  acpNewSession,
  sessionInfoToSession,
} from '../sessions';

vi.mock('../acpConnection', () => ({
  getAcpClient: vi.fn(),
  getAcpConnection: vi.fn(),
}));

function connectionWithSelectionSupport(client: unknown, supported = true) {
  return {
    client: client as Awaited<ReturnType<typeof getAcpClient>>,
    initializeResponse: {
      protocolVersion: 1,
      agentCapabilities: { _meta: { goose: supported ? { emptyExtensionSelection: {} } : {} } },
    } as InitializeResponse,
  };
}

function sessionInfo(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    sessionId: 'session-1',
    cwd: '/tmp',
    title: 'Scheduled session',
    updatedAt: '2026-01-01T00:00:00Z',
    _meta: {
      createdAt: '2026-01-01T00:00:00Z',
      messageCount: 0,
      sessionType: 'scheduled',
    },
    ...overrides,
  } as unknown as SessionInfo;
}

describe('ACP sessions', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it.each([undefined, [], [{ type: 'builtin' as const, name: 'developer' }]])(
    'preserves the extension selection on the wire: %j',
    async (selection) => {
      const request = vi.fn().mockResolvedValue({ sessionId: 'session-1' });
      vi.mocked(getAcpConnection).mockResolvedValue(
        connectionWithSelectionSupport({
          connection: { agent: { request } },
          goose: { sessionInfo_unstable: vi.fn().mockResolvedValue({ session: sessionInfo() }) },
        })
      );
      await acpNewSession('/tmp', selection);
      const sent = request.mock.calls[0][1];
      expect(Object.hasOwn(sent._meta, 'enabledExtensions')).toBe(selection !== undefined);
      expect(sent._meta.enabledExtensions).toEqual(selection);
      expect(getAcpConnection).toHaveBeenCalledOnce();
    }
  );

  it('rejects empty selections on older servers before creating a session', async () => {
    const request = vi.fn();
    vi.mocked(getAcpConnection).mockResolvedValue(
      connectionWithSelectionSupport(
        {
          connection: { agent: { request } },
        },
        false
      )
    );
    await expect(acpNewSession('/tmp', [])).rejects.toThrow('Update the server');
    expect(request).not.toHaveBeenCalled();
    expect(getAcpClient).not.toHaveBeenCalled();
  });

  it.each([false, true])(
    'does not switch to a replacement connection after checking capabilities (closed=%s)',
    async (closed) => {
      const request = closed
        ? vi.fn().mockRejectedValue(new Error('connection closed'))
        : vi.fn().mockResolvedValue({ sessionId: 'session-1' });
      const captured = connectionWithSelectionSupport({
        connection: { agent: { request } },
        goose: { sessionInfo_unstable: vi.fn().mockResolvedValue({ session: sessionInfo() }) },
      });
      const replacementRequest = vi.fn();
      const replacement = connectionWithSelectionSupport(
        {
          connection: { agent: { request: replacementRequest } },
        },
        false
      );
      vi.mocked(getAcpConnection).mockResolvedValueOnce(captured).mockResolvedValue(replacement);
      vi.mocked(getAcpClient).mockResolvedValue(replacement.client);
      if (closed) {
        await expect(acpNewSession('/tmp', [])).rejects.toThrow('connection closed');
      } else {
        await expect(acpNewSession('/tmp', [])).resolves.toMatchObject({ sessionId: 'session-1' });
      }
      expect(request).toHaveBeenCalledOnce();
      expect(getAcpConnection).toHaveBeenCalledOnce();
      expect(getAcpClient).not.toHaveBeenCalled();
      expect(replacementRequest).not.toHaveBeenCalled();
    }
  );

  it('preserves session type from ACP session info metadata', () => {
    const session = sessionInfoToSession(sessionInfo());

    expect(session.session_type).toBe('scheduled');
  });

  it('does not synthesize a title when ACP omits one', () => {
    const session = sessionInfoToSession(sessionInfo({ title: undefined }));

    expect(session.name).toBe('');
  });

  it('only requests acp session types when explicitly included', async () => {
    const client = {
      connection: {
        agent: {
          request: vi.fn().mockResolvedValue({ sessions: [] }),
        },
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    await acpListSessions();
    expect(client.connection.agent.request).toHaveBeenLastCalledWith(methods.agent.session.list, {
      _meta: { types: ['user', 'scheduled'] },
    });

    await acpListSessions(undefined, { includeAcp: true });
    expect(client.connection.agent.request).toHaveBeenLastCalledWith(methods.agent.session.list, {
      _meta: { types: ['user', 'scheduled', 'acp'] },
    });
  });

  it('returns session info refreshed after loading the ACP session', async () => {
    const loadedSessionInfo = sessionInfo({
      _meta: {
        createdAt: '2026-01-01T00:00:00Z',
        messageCount: 0,
        providerId: 'anthropic',
        modelId: 'claude-sonnet-4-5',
      },
    });
    const client = {
      connection: {
        agent: {
          request: vi.fn().mockResolvedValue({}),
        },
      },
      goose: {
        sessionInfo_unstable: vi
          .fn()
          .mockResolvedValueOnce({ session: sessionInfo() })
          .mockResolvedValueOnce({ session: loadedSessionInfo }),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const result = await acpLoadSession('session-1');

    expect(client.connection.agent.request).toHaveBeenCalledWith(methods.agent.session.load, {
      sessionId: 'session-1',
      cwd: '/tmp',
      mcpServers: [],
    });
    expect(client.goose.sessionInfo_unstable).toHaveBeenCalledTimes(2);
    expect(result.sessionInfo).toBe(loadedSessionInfo);
    expect(sessionInfoToSession(result.sessionInfo).provider_name).toBe('anthropic');
    expect(sessionInfoToSession(result.sessionInfo).model_config?.model_name).toBe(
      'claude-sonnet-4-5'
    );
  });

  it('carries the recipe parameter scope id in new-session metadata', async () => {
    const createdSessionInfo = sessionInfo();
    const client = {
      connection: {
        agent: {
          request: vi.fn().mockResolvedValue({ sessionId: 'session-1' }),
        },
      },
      goose: {
        sessionInfo_unstable: vi.fn().mockResolvedValue({ session: createdSessionInfo }),
      },
    };
    vi.mocked(getAcpConnection).mockResolvedValue(connectionWithSelectionSupport(client));

    await acpNewSession('/tmp', undefined, {
      recipeDeeplink: 'goose://recipe?url=example',
      recipeParameterScopeId: 'scope-1',
    });

    expect(client.connection.agent.request).toHaveBeenCalledWith(methods.agent.session.new, {
      cwd: '/tmp',
      mcpServers: [],
      _meta: {
        client: 'goose-desktop',
        recipeDeeplink: 'goose://recipe?url=example',
        recipeParameterScopeId: 'scope-1',
      },
    });
  });

  it('returns a list item from ACP session info', async () => {
    const client = {
      goose: {
        sessionInfo_unstable: vi.fn().mockResolvedValue({
          session: sessionInfo({
            title: 'Subagent session',
            _meta: {
              createdAt: '2026-01-01T00:00:00Z',
              lastMessageAt: '2026-01-01T00:01:00Z',
              messageCount: 3,
              sessionType: 'sub_agent',
              providerId: 'anthropic',
              modelId: 'claude-sonnet-4-5',
            },
          }),
        }),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const item = await acpGetSessionListItem('session-1');

    expect(client.goose.sessionInfo_unstable).toHaveBeenCalledWith({ sessionId: 'session-1' });
    expect(item).toMatchObject({
      id: 'session-1',
      name: 'Subagent session',
      workingDir: '/tmp',
      messageCount: 3,
      lastMessageAt: '2026-01-01T00:01:00Z',
      providerId: 'anthropic',
      modelId: 'claude-sonnet-4-5',
    });
  });
});

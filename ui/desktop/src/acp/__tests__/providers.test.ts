import { methods } from '@agentclientprotocol/sdk';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAcpClient } from '../acpConnection';
import {
  acpEnableProvider,
  acpGetProviderDetails,
  acpListProviderDetails,
  acpListSettingsProviderDetails,
  acpListSetupProviderDetails,
  acpRefreshProviderDetails,
  acpSetSessionProviderModel,
} from '../providers';

vi.mock('../acpConnection', () => ({
  getAcpClient: vi.fn(),
}));

function selectConfigOption(id: string, currentValue: string) {
  return {
    id,
    name: id,
    type: 'select',
    currentValue,
    options: [],
  };
}

describe('ACP providers', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('sets thinking effort after provider and model, then returns the final config response', async () => {
    const client = {
      connection: {
        agent: {
          request: vi
            .fn()
            .mockResolvedValueOnce({
              configOptions: [
                selectConfigOption('provider', 'anthropic'),
                selectConfigOption('model', 'provider-default-model'),
              ],
            })
            .mockResolvedValueOnce({
              configOptions: [
                selectConfigOption('provider', 'anthropic'),
                selectConfigOption('model', 'claude-sonnet-4-5'),
              ],
            })
            .mockResolvedValueOnce({
              configOptions: [
                selectConfigOption('provider', 'anthropic'),
                selectConfigOption('model', 'claude-sonnet-4-5'),
                selectConfigOption('thinking_effort', 'high'),
              ],
            }),
        },
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const applied = await acpSetSessionProviderModel(
      'session-1',
      'anthropic',
      'claude-sonnet-4-5',
      'high'
    );

    expect(client.connection.agent.request).toHaveBeenCalledTimes(3);
    expect(client.connection.agent.request).toHaveBeenNthCalledWith(
      1,
      methods.agent.session.setConfigOption,
      {
        sessionId: 'session-1',
        configId: 'provider',
        value: 'anthropic',
      }
    );
    expect(client.connection.agent.request).toHaveBeenNthCalledWith(
      2,
      methods.agent.session.setConfigOption,
      {
        sessionId: 'session-1',
        configId: 'model',
        value: 'claude-sonnet-4-5',
      }
    );
    expect(client.connection.agent.request).toHaveBeenNthCalledWith(
      3,
      methods.agent.session.setConfigOption,
      {
        sessionId: 'session-1',
        configId: 'thinking_effort',
        value: 'high',
      }
    );
    expect(applied).toEqual({
      providerId: 'anthropic',
      modelId: 'claude-sonnet-4-5',
    });
  });

  it('rechecks an uninstalled ACP adapter without trying to start it', async () => {
    const entry = providerEntry({ configured: false, available: false });
    const client = {
      goose: {
        providersList_unstable: vi.fn().mockResolvedValue({ entries: [entry] }),
        providersReadinessCheck_unstable: vi.fn(),
        providersInventoryRefresh_unstable: vi.fn(),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const result = await acpRefreshProviderDetails('claude-acp');

    expect(result.provider.is_configured).toBe(false);
    expect(result.connectionChecked).toBe(false);
    expect(client.goose.providersInventoryRefresh_unstable).not.toHaveBeenCalled();
    expect(client.goose.providersReadinessCheck_unstable).not.toHaveBeenCalled();
  });

  it('keeps compatibility providers in inventory but omits them from setup lists', async () => {
    const replacement = providerEntry();
    const deprecated = providerEntry({
      providerId: 'claude-code',
      visibleInSetup: false,
      deprecated: true,
      replacement: 'claude-acp',
    });
    const hidden = providerEntry({
      providerId: 'internal-provider',
      visibleInSetup: false,
      configured: false,
    });
    const client = {
      goose: {
        providersList_unstable: vi
          .fn()
          .mockImplementation(({ providerIds }: { providerIds?: string[] }) => ({
            entries: providerIds?.length ? [deprecated] : [replacement, deprecated, hidden],
          })),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    expect((await acpListProviderDetails()).map((provider) => provider.name)).toEqual([
      'claude-acp',
      'claude-code',
      'internal-provider',
    ]);
    expect((await acpListSetupProviderDetails()).map((provider) => provider.name)).toEqual([
      'claude-acp',
    ]);
    expect((await acpListSettingsProviderDetails()).map((provider) => provider.name)).toEqual([
      'claude-acp',
      'claude-code',
    ]);
    expect((await acpGetProviderDetails('claude-code')).replacement).toBe('claude-acp');
  });

  it('uses the explicit ACP capability instead of provider id', async () => {
    const custom = providerEntry({
      providerId: 'custom_example-acp',
      providerType: 'Custom',
      acp: false,
    });
    const agent = providerEntry({ providerId: 'cursor-agent', acp: false });
    const acp = providerEntry({ providerId: 'pi-acp', acp: true });
    const client = {
      goose: {
        providersList_unstable: vi.fn().mockResolvedValue({ entries: [custom, agent, acp] }),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const providers = await acpListProviderDetails();

    expect(providers.map((provider) => provider.uses_acp)).toEqual([false, false, true]);
  });

  it('probes an installed ACP adapter and returns its refreshed models', async () => {
    const installed = providerEntry({ configured: true, refreshing: false });
    const refreshed = providerEntry({
      configured: true,
      refreshing: false,
      models: [{ id: 'claude-sonnet', name: 'Claude Sonnet', recommended: true }],
    });
    const client = {
      goose: {
        providersList_unstable: vi
          .fn()
          .mockResolvedValueOnce({ entries: [installed] })
          .mockResolvedValueOnce({ entries: [refreshed] }),
        providersReadinessCheck_unstable: vi.fn().mockResolvedValue({
          providerId: 'claude-acp',
          ready: true,
        }),
        providersInventoryRefresh_unstable: vi.fn().mockResolvedValue({
          started: ['claude-acp'],
          skipped: [],
        }),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const result = await acpRefreshProviderDetails('claude-acp');

    expect(result.connectionChecked).toBe(true);
    expect(result.provider.metadata.known_models).toEqual([
      { name: 'claude-sonnet', context_limit: undefined, reasoning: undefined },
    ]);
  });

  it('waits for first-time ACP model discovery after enabling the provider', async () => {
    const available = providerEntry({ configured: false });
    const refreshed = providerEntry({
      configured: true,
      models: [{ id: 'claude-sonnet', name: 'Claude Sonnet', recommended: true }],
    });
    const client = {
      goose: {
        providersConfigSave_unstable: vi.fn().mockResolvedValue({
          status: {},
          refresh: { started: ['claude-acp'], skipped: [] },
        }),
        providersList_unstable: vi
          .fn()
          .mockResolvedValueOnce({ entries: [available] })
          .mockResolvedValueOnce({ entries: [available] })
          .mockResolvedValueOnce({ entries: [refreshed] }),
        providersReadinessCheck_unstable: vi.fn().mockResolvedValue({
          providerId: 'claude-acp',
          ready: true,
        }),
        providersInventoryRefresh_unstable: vi.fn().mockResolvedValue({
          started: [],
          skipped: [{ providerId: 'claude-acp', reason: 'not_configured' }],
        }),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const checked = await acpRefreshProviderDetails('claude-acp');
    const enabled = await acpEnableProvider('claude-acp');

    expect(checked.provider.is_configured).toBe(false);
    expect(checked.provider.metadata.known_models).toEqual([]);
    expect(client.goose.providersConfigSave_unstable).toHaveBeenCalledWith({
      providerId: 'claude-acp',
      fields: [],
    });
    expect(client.goose.providersList_unstable).toHaveBeenCalledWith({
      providerIds: ['claude-acp'],
    });
    expect(enabled.is_configured).toBe(true);
    expect(enabled.metadata.known_models).toEqual([
      { name: 'claude-sonnet', context_limit: undefined, reasoning: undefined },
    ]);
  });

  it('surfaces an ACP authentication failure without using model refresh as readiness', async () => {
    const installed = providerEntry({ configured: true });
    const client = {
      goose: {
        providersList_unstable: vi.fn().mockResolvedValue({ entries: [installed] }),
        providersReadinessCheck_unstable: vi.fn().mockResolvedValue({
          providerId: 'claude-acp',
          ready: false,
          error: 'OAuth session expired',
        }),
        providersInventoryRefresh_unstable: vi.fn(),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );

    const result = await acpRefreshProviderDetails('claude-acp');

    expect(result.connectionChecked).toBe(true);
    expect(result.readinessError).toBe('OAuth session expired');
    expect(client.goose.providersInventoryRefresh_unstable).not.toHaveBeenCalled();
  });

  it('stops polling provider inventory when the setup screen closes', async () => {
    const installed = providerEntry({ configured: true });
    const refreshing = providerEntry({ configured: true, refreshing: true });
    const client = {
      goose: {
        providersList_unstable: vi
          .fn()
          .mockResolvedValueOnce({ entries: [installed] })
          .mockResolvedValue({ entries: [refreshing] }),
        providersReadinessCheck_unstable: vi.fn().mockResolvedValue({
          providerId: 'claude-acp',
          ready: true,
        }),
        providersInventoryRefresh_unstable: vi.fn().mockResolvedValue({
          started: ['claude-acp'],
          skipped: [],
        }),
      },
    };
    vi.mocked(getAcpClient).mockResolvedValue(
      client as unknown as Awaited<ReturnType<typeof getAcpClient>>
    );
    const controller = new AbortController();

    const refresh = acpRefreshProviderDetails('claude-acp', controller.signal);
    await vi.waitFor(() => expect(client.goose.providersList_unstable).toHaveBeenCalledTimes(2));
    controller.abort();

    await expect(refresh).rejects.toMatchObject({ name: 'AbortError' });
    expect(client.goose.providersList_unstable).toHaveBeenCalledTimes(2);
  });
});

function providerEntry(overrides: Record<string, unknown> = {}) {
  return {
    providerId: 'claude-acp',
    providerName: 'Claude Code',
    description: 'Use Claude Code through ACP',
    defaultModel: 'current',
    configured: true,
    available: true,
    providerType: 'Builtin',
    acp: true,
    visibleInSetup: true,
    deprecated: false,
    configKeys: [],
    setupSteps: [],
    supportsRefresh: true,
    refreshing: false,
    models: [],
    stale: false,
    ...overrides,
  };
}

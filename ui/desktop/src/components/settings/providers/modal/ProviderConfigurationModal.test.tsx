import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { acpEnableProvider, acpRefreshProviderDetails } from '../../../../acp/providers';
import { IntlTestWrapper } from '../../../../i18n/test-utils';
import type { ProviderDetails } from '../../../../types/providers';
import ProviderConfigurationModal from './ProviderConfigurationModal';

vi.mock('../../../ModelAndProviderContext', () => ({
  useModelAndProvider: () => ({
    getCurrentModelAndProvider: vi.fn().mockResolvedValue({
      provider: 'another-provider',
      model: 'model',
    }),
  }),
}));

vi.mock('../../../../acp/providers', () => ({
  acpAuthenticateProvider: vi.fn(),
  acpDeleteCustomProvider: vi.fn(),
  acpDeleteProviderConfig: vi.fn(),
  acpEnableProvider: vi.fn(),
  acpRefreshProviderDetails: vi.fn(),
  acpSaveProviderConfig: vi.fn(),
}));

const oauthProvider: ProviderDetails = {
  name: 'github_copilot',
  is_configured: true,
  is_available: true,
  visible_in_setup: true,
  deprecated: false,
  provider_type: 'Builtin',
  uses_acp: false,
  metadata: {
    name: 'github_copilot',
    display_name: 'GitHub Copilot',
    description: 'GitHub Copilot models',
    default_model: 'current',
    known_models: [],
    model_doc_link: '',
    config_keys: [
      {
        name: 'GITHUB_COPILOT_OAUTH',
        required: true,
        secret: true,
        oauth_flow: true,
      },
    ],
  },
};

describe('ProviderConfigurationModal', () => {
  it('offers to remove an existing OAuth configuration without an ACP readiness check', () => {
    render(<ProviderConfigurationModal provider={oauthProvider} onClose={vi.fn()} />, {
      wrapper: IntlTestWrapper,
    });

    expect(screen.getByRole('button', { name: 'Remove Configuration' })).toBeInTheDocument();
  });

  it('invalidates ACP readiness while checking again', async () => {
    const user = userEvent.setup();
    const acpProvider: ProviderDetails = {
      ...oauthProvider,
      name: 'claude-acp',
      uses_acp: true,
      metadata: {
        ...oauthProvider.metadata,
        name: 'claude-acp',
        config_keys: [],
        setup_steps: ['Install and authenticate Claude Code'],
      },
    };
    let finishSecondCheck:
      | ((value: Awaited<ReturnType<typeof acpRefreshProviderDetails>>) => void)
      | undefined;
    vi.mocked(acpRefreshProviderDetails)
      .mockResolvedValueOnce({
        provider: acpProvider,
        connectionChecked: true,
        readinessError: null,
      })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishSecondCheck = resolve;
          })
      );

    render(
      <ProviderConfigurationModal
        provider={acpProvider}
        onClose={vi.fn()}
        onConfigured={vi.fn()}
      />,
      { wrapper: IntlTestWrapper }
    );

    expect(await screen.findByRole('button', { name: 'Choose model' })).toBeInTheDocument();
    expect(acpRefreshProviderDetails).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole('button', { name: 'Check again' }));
    expect(screen.queryByRole('button', { name: 'Choose model' })).not.toBeInTheDocument();

    finishSecondCheck?.({
      provider: acpProvider,
      connectionChecked: true,
      readinessError: 'Authentication expired',
    });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Check again' })).not.toBeDisabled()
    );
  });

  it('enables and removes an available ACP provider without changing the adapter', async () => {
    const user = userEvent.setup();
    const onConfigured = vi.fn();
    const acpProvider: ProviderDetails = {
      ...oauthProvider,
      name: 'codex-acp',
      is_configured: false,
      uses_acp: true,
      metadata: {
        ...oauthProvider.metadata,
        name: 'codex-acp',
        config_keys: [],
        setup_steps: ['Install Codex ACP'],
      },
    };
    vi.mocked(acpRefreshProviderDetails).mockResolvedValue({
      provider: acpProvider,
      connectionChecked: true,
      readinessError: null,
    });
    const configuredProvider = {
      ...acpProvider,
      is_configured: true,
      metadata: {
        ...acpProvider.metadata,
        known_models: [{ name: 'gpt-5', context_limit: 0 }],
      },
    };
    let finishEnable: ((provider: ProviderDetails) => void) | undefined;
    vi.mocked(acpEnableProvider).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishEnable = resolve;
        })
    );

    const view = render(
      <ProviderConfigurationModal
        provider={acpProvider}
        onClose={vi.fn()}
        onConfigured={onConfigured}
      />,
      { wrapper: IntlTestWrapper }
    );

    await user.click(await screen.findByRole('button', { name: 'Choose model' }));
    expect(acpEnableProvider).toHaveBeenCalledWith('codex-acp', expect.any(globalThis.AbortSignal));
    expect(onConfigured).not.toHaveBeenCalled();

    finishEnable?.(configuredProvider);
    await waitFor(() =>
      expect(onConfigured).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'codex-acp',
          is_configured: true,
          metadata: expect.objectContaining({
            known_models: [{ name: 'gpt-5', context_limit: 0 }],
          }),
        })
      )
    );

    view.unmount();
    vi.mocked(acpRefreshProviderDetails).mockResolvedValue({
      provider: { ...acpProvider, is_configured: true },
      connectionChecked: true,
      readinessError: null,
    });
    render(
      <ProviderConfigurationModal
        provider={{ ...acpProvider, is_configured: true }}
        onClose={vi.fn()}
        onConfigured={onConfigured}
      />,
      { wrapper: IntlTestWrapper }
    );
    expect(await screen.findByRole('button', { name: 'Remove Configuration' })).toBeInTheDocument();
  });
});

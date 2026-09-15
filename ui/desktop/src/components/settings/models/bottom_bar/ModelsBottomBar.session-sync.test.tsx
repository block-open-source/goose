import React from 'react';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ModelsBottomBar from './ModelsBottomBar';
import { SwitchModelModal } from '../subcomponents/SwitchModelModal';
import { IntlTestWrapper } from '../../../../i18n/test-utils';
import type { ProviderDetails } from '../../../../types/providers';

const mocks = vi.hoisted(() => ({
  changeModel: vi.fn(),
  listProviders: vi.fn(),
  getProvider: vi.fn(),
  listModels: vi.fn(),
  unexpectedFetch: vi.fn(),
}));

vi.mock('../../../ModelAndProviderContext', () => ({
  useModelAndProvider: () => ({
    changeModel: mocks.changeModel,
    currentModel: 'remote-default-model',
    currentProvider: 'configured-remote',
  }),
}));

vi.mock('../../../../acp/providers', () => ({
  acpListProviderDetails: mocks.listProviders,
  acpGetProviderDetails: mocks.getProvider,
  acpListProviderModels: mocks.listModels,
  acpReadThinkingEffort: vi.fn().mockResolvedValue(null),
  acpSaveThinkingEffort: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../../../../utils/analytics', () => ({
  trackModelChanged: vi.fn(),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function provider(name: string, displayName: string, model: string): ProviderDetails {
  return {
    name,
    provider_type: 'Custom',
    is_configured: true,
    is_available: true,
    visible_in_setup: true,
    deprecated: false,
    uses_acp: false,
    metadata: {
      name,
      display_name: displayName,
      description: 'Synthetic configured provider; no network implementation.',
      default_model: model,
      known_models: [{ name: model, reasoning: false }],
      config_keys: [],
      model_doc_link: '',
    },
  };
}

const configuredProviders = [
  provider('configured-remote', 'Configured Remote', 'remote-default-model'),
  provider('trusted-local', 'Trusted Local', 'trusted-session-model'),
];

const remoteModels = [{ id: 'remote-default-model', reasoning: false }];
const trustedModels = [{ id: 'trusted-session-model', reasoning: false }];

function caller(sessionLoaded: boolean, model: string | null, providerName: string | null) {
  return (
    <ModelsBottomBar
      sessionId="session-under-test"
      dropdownRef={React.createRef<HTMLDivElement>() as React.RefObject<HTMLDivElement>}
      setView={vi.fn()}
      sessionLoaded={sessionLoaded}
      sessionModel={model}
      sessionProvider={providerName}
      onModelChanged={vi.fn()}
    />
  );
}

async function openPicker(user: ReturnType<typeof userEvent.setup>) {
  const trigger = screen.getByRole('button', {
    name: /Loading model|trusted-session-model|remote-default-model/,
  });
  await user.click(trigger);
  await user.click(await screen.findByRole('menuitem', { name: 'Change Model' }));
  return screen.findByRole('dialog');
}

describe('Model picker initial selection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.changeModel.mockResolvedValue(false);
    mocks.getProvider.mockImplementation(async (name: string) => {
      const found = configuredProviders.find((entry) => entry.name === name);
      if (!found) throw new Error('Unexpected synthetic provider');
      return found;
    });
    mocks.listProviders.mockResolvedValue(configuredProviders);
    mocks.listModels.mockImplementation(async (name: string) =>
      name === 'trusted-local' ? trustedModels : remoteModels
    );
    mocks.unexpectedFetch.mockRejectedValue(new Error('Network disabled for validation'));
    vi.stubGlobal('fetch', mocks.unexpectedFetch);
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      }
    );
    Object.defineProperty(window, 'appConfig', {
      configurable: true,
      value: { get: () => undefined },
    });
    Element.prototype.scrollIntoView = vi.fn();
  });

  afterEach(() => {
    expect(mocks.unexpectedFetch).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it('waits for visible providers and synchronizes late session metadata', async () => {
    const inventory = deferred<ProviderDetails[]>();
    mocks.listProviders.mockReturnValue(inventory.promise);
    const user = userEvent.setup();
    const view = render(caller(false, null, null), { wrapper: IntlTestWrapper });
    const dialog = await openPicker(user);
    await waitFor(() => expect(mocks.listProviders).toHaveBeenCalledOnce());

    expect(within(dialog).getByText('Provider, type to search')).toBeVisible();
    expect(within(dialog).queryByText('Configured Remote')).not.toBeInTheDocument();
    expect(within(dialog).getByText('remote-default-model')).toBeVisible();
    const submit = within(dialog).getByRole('button', { name: 'Select model' });
    expect(submit).toBeDisabled();
    await user.click(submit);
    expect(mocks.changeModel).not.toHaveBeenCalled();

    view.rerender(caller(true, 'trusted-session-model', 'trusted-local'));
    await waitFor(() => expect(mocks.getProvider).toHaveBeenCalledWith('trusted-local'));
    expect(within(dialog).queryByText('Trusted Local')).not.toBeInTheDocument();
    expect(within(dialog).getByText('Provider, type to search')).toBeVisible();
    expect(submit).toBeDisabled();
    await act(async () => inventory.resolve(configuredProviders));
    await waitFor(() => expect(within(dialog).getByText('Trusted Local')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText('trusted-session-model')).toBeVisible());
    await waitFor(() => expect(submit).toBeEnabled());
    await user.click(submit);

    await waitFor(() =>
      expect(mocks.changeModel).toHaveBeenCalledWith(
        'session-under-test',
        expect.objectContaining({
          provider: 'trusted-local',
          name: 'trusted-session-model',
        })
      )
    );
    expect(mocks.changeModel).toHaveBeenCalledOnce();
  });

  it('waits for visible models before submitting the synchronized session pair', async () => {
    const models = deferred<typeof remoteModels>();
    mocks.listModels.mockReturnValue(models.promise);
    const user = userEvent.setup();
    const view = render(caller(false, null, null), { wrapper: IntlTestWrapper });
    const dialog = await openPicker(user);
    await waitFor(() => expect(within(dialog).getByText('Configured Remote')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText(/Loading models/)).toBeVisible());
    expect(dialog.querySelector('input[disabled]')).not.toBeNull();
    expect(within(dialog).queryByText('remote-default-model')).not.toBeInTheDocument();
    const submit = within(dialog).getByRole('button', { name: 'Select model' });
    expect(submit).toBeDisabled();
    await user.click(submit);
    expect(mocks.changeModel).not.toHaveBeenCalled();

    view.rerender(caller(true, 'trusted-session-model', 'trusted-local'));
    await waitFor(() => expect(mocks.getProvider).toHaveBeenCalledWith('trusted-local'));
    expect(within(dialog).getByText('Trusted Local')).toBeVisible();
    expect(within(dialog).queryByText('trusted-session-model')).not.toBeInTheDocument();
    expect(submit).toBeDisabled();
    await act(async () => models.resolve(trustedModels));
    await waitFor(() => expect(within(dialog).getByText('trusted-session-model')).toBeVisible());
    await waitFor(() => expect(submit).toBeEnabled());
    await user.click(submit);

    await waitFor(() =>
      expect(mocks.changeModel).toHaveBeenCalledWith(
        'session-under-test',
        expect.objectContaining({
          provider: 'trusted-local',
          name: 'trusted-session-model',
        })
      )
    );
  });

  it('synchronizes late session metadata even after the fallback controls load', async () => {
    const user = userEvent.setup();
    const view = render(caller(false, null, null), { wrapper: IntlTestWrapper });
    const dialog = await openPicker(user);
    await waitFor(() => expect(within(dialog).getByText('Configured Remote')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText('remote-default-model')).toBeVisible());

    view.rerender(caller(true, 'trusted-session-model', 'trusted-local'));
    await waitFor(() => expect(mocks.getProvider).toHaveBeenCalledWith('trusted-local'));
    await waitFor(() => expect(within(dialog).getByText('Trusted Local')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText('trusted-session-model')).toBeVisible());
    await user.click(within(dialog).getByRole('button', { name: 'Select model' }));

    await waitFor(() =>
      expect(mocks.changeModel).toHaveBeenCalledWith(
        'session-under-test',
        expect.objectContaining({
          provider: 'trusted-local',
          name: 'trusted-session-model',
        })
      )
    );
  });

  it('preserves the trusted pair when authoritative metadata exists before opening', async () => {
    const user = userEvent.setup();
    render(caller(true, 'trusted-session-model', 'trusted-local'), {
      wrapper: IntlTestWrapper,
    });
    const dialog = await openPicker(user);
    await waitFor(() => expect(within(dialog).getByText('Trusted Local')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText('trusted-session-model')).toBeVisible());
    expect(within(dialog).queryByText('Configured Remote')).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Select model' }));

    await waitFor(() =>
      expect(mocks.changeModel).toHaveBeenCalledWith(
        'session-under-test',
        expect.objectContaining({
          provider: 'trusted-local',
          name: 'trusted-session-model',
        })
      )
    );
  });

  it('preserves an explicit provider/model edit when late session metadata arrives', async () => {
    const user = userEvent.setup();
    const view = render(caller(false, null, null), { wrapper: IntlTestWrapper });
    const dialog = await openPicker(user);
    await waitFor(() => expect(within(dialog).getByText('Configured Remote')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText('remote-default-model')).toBeVisible());

    await user.click(within(dialog).getAllByRole('combobox')[0]);
    await user.click(await screen.findByRole('option', { name: 'Trusted Local' }));
    await waitFor(() => expect(within(dialog).getAllByRole('combobox')[1]).toBeEnabled());
    await user.click(within(dialog).getAllByRole('combobox')[1]);
    await user.click(await screen.findByRole('option', { name: 'trusted-session-model' }));

    await act(async () => {
      view.rerender(caller(true, 'remote-default-model', 'configured-remote'));
    });
    expect(within(dialog).getByText('Trusted Local')).toBeVisible();
    expect(within(dialog).getByText('trusted-session-model')).toBeVisible();
    await user.click(within(dialog).getByRole('button', { name: 'Select model' }));

    await waitFor(() =>
      expect(mocks.changeModel).toHaveBeenCalledWith(
        'session-under-test',
        expect.objectContaining({
          provider: 'trusted-local',
          name: 'trusted-session-model',
        })
      )
    );
  });

  it.each([false, true])(
    'preserves settings-mode selection with initialProvider=%s',
    async (useInitialProvider) => {
      const user = userEvent.setup();
      render(
        <SwitchModelModal
          sessionId={null}
          onClose={vi.fn()}
          setView={vi.fn()}
          initialProvider={useInitialProvider ? 'trusted-local' : undefined}
        />,
        { wrapper: IntlTestWrapper }
      );
      const dialog = await screen.findByRole('dialog');
      const expectedProvider = useInitialProvider ? 'trusted-local' : 'configured-remote';
      const expectedModel = useInitialProvider ? 'trusted-session-model' : 'remote-default-model';
      await waitFor(() => expect(within(dialog).getByText(expectedModel)).toBeVisible());
      const submit = within(dialog).getByRole('button', { name: 'Select model' });
      await waitFor(() => expect(submit).toBeEnabled());
      await user.click(submit);
      await waitFor(() =>
        expect(mocks.changeModel).toHaveBeenCalledWith(
          null,
          expect.objectContaining({ provider: expectedProvider, name: expectedModel })
        )
      );
    }
  );

  it('allows cancel while loading and a visible fallback choice after reopening', async () => {
    const inventory = deferred<ProviderDetails[]>();
    mocks.listProviders.mockReturnValue(inventory.promise);
    const user = userEvent.setup();
    render(caller(false, null, null), { wrapper: IntlTestWrapper });
    const loadingDialog = await openPicker(user);
    await user.click(within(loadingDialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(mocks.changeModel).not.toHaveBeenCalled();
    await act(async () => inventory.resolve(configuredProviders));
    const dialog = await openPicker(user);
    await waitFor(() => expect(within(dialog).getByText('Configured Remote')).toBeVisible());
    await waitFor(() => expect(within(dialog).getByText('remote-default-model')).toBeVisible());
    const submit = within(dialog).getByRole('button', { name: 'Select model' });
    await waitFor(() => expect(submit).toBeEnabled());
    await user.click(submit);
    await waitFor(() =>
      expect(mocks.changeModel).toHaveBeenCalledWith(
        'session-under-test',
        expect.objectContaining({ provider: 'configured-remote', name: 'remote-default-model' })
      )
    );
  });

  it.each([false, true])(
    'preserves custom model edits after late metadata with provider error=%s',
    async (providerError) => {
      mocks.listProviders.mockResolvedValue([
        { ...configuredProviders[0], provider_type: 'Builtin' },
        configuredProviders[1],
      ]);
      if (providerError)
        mocks.listModels.mockRejectedValue(new Error('Synthetic model-list failure'));
      const user = userEvent.setup();
      const view = render(caller(false, null, null), { wrapper: IntlTestWrapper });
      const dialog = await openPicker(user);
      await waitFor(() => expect(within(dialog).getByText('Configured Remote')).toBeVisible());
      if (!providerError) {
        await waitFor(() => expect(within(dialog).getAllByRole('combobox')[1]).toBeEnabled());
        await user.click(within(dialog).getAllByRole('combobox')[1]);
        await user.click(await screen.findByRole('option', { name: /Enter.*not listed/i }));
      }
      const customInput = await within(dialog).findByRole('textbox');
      await user.clear(customInput);
      await user.type(customInput, 'explicit-custom-model');
      view.rerender(caller(true, 'trusted-session-model', 'trusted-local'));
      await waitFor(() => expect(mocks.getProvider).toHaveBeenCalledWith('trusted-local'));
      expect(customInput).toHaveValue('explicit-custom-model');
      expect(within(dialog).getByText('Configured Remote')).toBeVisible();
      await user.click(within(dialog).getByRole('button', { name: 'Select model' }));
      await waitFor(() =>
        expect(mocks.changeModel).toHaveBeenCalledWith(
          'session-under-test',
          expect.objectContaining({ provider: 'configured-remote', name: 'explicit-custom-model' })
        )
      );
    }
  );

  it('does not refill an explicitly cleared model when session metadata arrives', async () => {
    const user = userEvent.setup();
    const view = render(caller(false, null, null), { wrapper: IntlTestWrapper });
    const dialog = await openPicker(user);
    await waitFor(() => expect(within(dialog).getByText('remote-default-model')).toBeVisible());
    await waitFor(() => expect(within(dialog).getAllByRole('combobox')[1]).toBeEnabled());
    await user.click(within(dialog).getAllByRole('combobox')[1]);
    await user.keyboard('{Backspace}{Escape}');
    view.rerender(caller(true, 'trusted-session-model', 'trusted-local'));
    await waitFor(() => expect(mocks.getProvider).toHaveBeenCalledWith('trusted-local'));
    expect(within(dialog).queryByText('trusted-session-model')).not.toBeInTheDocument();
    expect(within(dialog).queryByText('remote-default-model')).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Select model' }));
    expect(mocks.changeModel).not.toHaveBeenCalled();
  });
});

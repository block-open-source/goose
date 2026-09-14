import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../../../../../../i18n/test-utils';
import CustomProviderForm from './CustomProviderForm';

const templates = vi.hoisted(() => ({
  a: {
    providerId: 'template-a',
    name: 'Template A',
    format: 'openai',
    apiUrl: 'https://a.example.com',
    models: [
      {
        id: 'model-a',
        name: 'Model A',
        contextLimit: 8192,
        capabilities: {
          toolCall: false,
          reasoning: false,
          attachment: false,
          temperature: true,
        },
        deprecated: false,
      },
    ],
    supportsStreaming: true,
    envVar: 'TEMPLATE_A_API_KEY',
    docUrl: '',
  },
  b: {
    providerId: 'template-b',
    name: 'Template B',
    format: 'anthropic',
    apiUrl: 'https://b.example.com',
    models: [
      {
        id: 'model-b',
        name: 'Model B',
        contextLimit: 8192,
        capabilities: {
          toolCall: false,
          reasoning: false,
          attachment: false,
          temperature: true,
        },
        deprecated: false,
      },
    ],
    supportsStreaming: true,
    envVar: 'TEMPLATE_B_API_KEY',
    docUrl: '',
  },
}));

vi.mock('../ProviderCatalogPicker', () => ({
  default: ({ onSelect }: { onSelect: (template: typeof templates.a) => void }) => (
    <div>
      <button onClick={() => onSelect(templates.a)}>Use Template A</button>
      <button onClick={() => onSelect(templates.b)}>Use Template B</button>
    </div>
  ),
}));

const renderForm = (onSubmit = vi.fn()) => {
  render(
    <CustomProviderForm initialData={null} isEditable onSubmit={onSubmit} onCancel={vi.fn()} />,
    { wrapper: IntlTestWrapper }
  );
  return onSubmit;
};

const openTemplateCatalog = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(screen.getByText('Start from a provider template'));
};

const addHeader = async (user: ReturnType<typeof userEvent.setup>, name: string, value: string) => {
  const nameInputs = screen.getAllByPlaceholderText('Header name');
  const valueInputs = screen.getAllByPlaceholderText('Value');
  await user.type(nameInputs[nameInputs.length - 1], name);
  await user.type(valueInputs[valueInputs.length - 1], value);
  await user.click(screen.getByRole('button', { name: 'Add' }));
};

describe('CustomProviderForm transitions', () => {
  it('does not carry credentials from a cleared template into the next template', async () => {
    const user = userEvent.setup();
    const onSubmit = renderForm();
    await openTemplateCatalog(user);
    await user.click(screen.getByRole('button', { name: 'Use Template A' }));

    await user.type(screen.getByLabelText(/API Key/), 'template-a-secret');
    await addHeader(user, 'Authorization', 'Bearer template-a');

    const pendingNames = screen.getAllByPlaceholderText('Header name');
    const pendingValues = screen.getAllByPlaceholderText('Value');
    await user.type(pendingNames[pendingNames.length - 1], 'Authorization');
    await user.type(pendingValues[pendingValues.length - 1], 'Bearer pending-template-a');
    await user.click(screen.getByRole('button', { name: 'Add' }));
    expect(screen.getByText('A header with this name already exists')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Clear' }));
    await openTemplateCatalog(user);
    await user.click(screen.getByRole('button', { name: 'Use Template B' }));

    expect(screen.getByLabelText(/API Key/)).toHaveValue('');
    expect(screen.queryByDisplayValue('Bearer template-a')).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue('Bearer pending-template-a')).not.toBeInTheDocument();
    expect(screen.queryByText('A header with this name already exists')).not.toBeInTheDocument();

    await user.type(screen.getByLabelText(/API Key/), 'template-b-secret');
    await addHeader(user, 'X-Template-B', 'template-b-header');
    await user.click(screen.getByRole('button', { name: 'Create Provider' }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        api_key: 'template-b-secret',
        catalog_provider_id: 'template-b',
        headers: { 'X-Template-B': 'template-b-header' },
      })
    );
  });

  it('clears secrets and submit state when returning to the setup choice', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const user = userEvent.setup();
    let rejectSubmit: ((reason: Error) => void) | undefined;
    const onSubmit = vi.fn(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSubmit = reject;
        })
    );
    renderForm(onSubmit);
    await user.click(screen.getByText('Configure manually'));

    await user.type(screen.getByLabelText(/Display Name/), 'Manual Provider');
    await user.type(screen.getByLabelText(/API URL/), 'https://manual.example.com');
    await user.type(screen.getByLabelText(/Available Models/), 'model-a');
    await user.click(screen.getByLabelText('This provider requires an API key'));
    await user.type(screen.getByLabelText(/API Key/), 'manual-secret');
    await addHeader(user, 'Authorization', 'Bearer manual-secret');

    const pendingNames = screen.getAllByPlaceholderText('Header name');
    const pendingValues = screen.getAllByPlaceholderText('Value');
    await user.type(pendingNames[pendingNames.length - 1], 'Authorization');
    await user.type(pendingValues[pendingValues.length - 1], 'Bearer pending-secret');
    await user.click(screen.getByRole('button', { name: 'Add' }));
    await user.click(screen.getByRole('button', { name: 'Create Provider' }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());

    await user.click(screen.getByRole('button', { name: '← Back' }));
    await user.click(screen.getByText('Configure manually'));

    await act(async () => {
      rejectSubmit?.(new Error('save failed'));
      await Promise.resolve();
    });

    expect(screen.getByLabelText(/API Key/)).toHaveValue('');
    expect(screen.queryByDisplayValue('Bearer manual-secret')).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue('Bearer pending-secret')).not.toBeInTheDocument();
    expect(screen.queryByText('A header with this name already exists')).not.toBeInTheDocument();
    expect(screen.queryByText(/Failed to save provider/)).not.toBeInTheDocument();
  });

  it.each([
    ['anthropic_compatible', 'anthropic_compatible'],
    ['ollama_compatible', 'ollama_compatible'],
    ['openai_compatible', 'openai_compatible'],
    ['anthropic', 'anthropic_compatible'],
    ['ollama', 'ollama_compatible'],
    ['openai', 'openai_compatible'],
  ])('saves a provider stored as %s with engine %s', async (engine, expectedEngine) => {
    const user = userEvent.setup();
    const onSubmit = vi.fn();
    render(
      <CustomProviderForm
        initialData={{
          engine,
          display_name: 'Existing Provider',
          api_url: 'https://existing.example.com',
          api_key: '',
          models: ['model-a'],
          supports_streaming: true,
          requires_auth: true,
          toolshim: false,
        }}
        isEditable
        onSubmit={onSubmit}
        onCancel={vi.fn()}
      />,
      { wrapper: IntlTestWrapper }
    );

    await user.click(screen.getByRole('button', { name: 'Update Provider' }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ engine: expectedEngine }));
  });

  it('clears form validation when returning to the setup choice', async () => {
    const user = userEvent.setup();
    renderForm();
    await user.click(screen.getByText('Configure manually'));
    await user.click(screen.getByRole('button', { name: 'Create Provider' }));
    expect(screen.getByText('Display name is required')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '← Back' }));
    await user.click(screen.getByText('Configure manually'));

    expect(screen.queryByText('Display name is required')).not.toBeInTheDocument();
  });
});

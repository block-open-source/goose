import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  fireEvent,
  render,
  type RenderOptions,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import ModelsBottomBar from './ModelsBottomBar';
import { IntlTestWrapper } from '../../../../i18n/test-utils';

const renderWithIntl = (ui: React.ReactElement, options?: RenderOptions) =>
  render(ui, { wrapper: IntlTestWrapper, ...options });

const createDropdownRef = (): React.RefObject<HTMLDivElement> =>
  ({ current: document.createElement('div') }) as React.RefObject<HTMLDivElement>;

let mockCurrentModel: string | null = 'config-model';
let mockCurrentProvider: string | null = 'config-provider';
const mockGetProviders = vi.fn();
const mockOnModelChanged = vi.fn();
const mockPreventCloseAutoFocus = vi.fn();
const mockChangeModel = vi.fn();

vi.mock('../../../ModelAndProviderContext', () => ({
  useModelAndProvider: () => ({
    currentModel: mockCurrentModel,
    currentProvider: mockCurrentProvider,
    changeModel: mockChangeModel,
  }),
}));

vi.mock('../../../ConfigContext', () => ({
  useConfig: () => ({
    getProviders: mockGetProviders,
  }),
}));

vi.mock('../modelInterface', () => ({
  getProviderMetadata: vi.fn().mockResolvedValue({ display_name: 'Config Provider' }),
  fetchModelReasoning: vi.fn().mockResolvedValue(null),
}));

vi.mock('../predefinedModelsUtils', () => ({
  getModelDisplayName: (model: string) => `Display ${model}`,
}));

vi.mock('../../../../acp/providers', () => ({
  acpReadThinkingEffort: vi.fn().mockResolvedValue(null),
}));

vi.mock('../../../bottom_menu/BottomMenuAlertPopover', () => ({
  default: () => null,
}));

vi.mock('../../../ui/dropdown-menu', () => ({
  DropdownMenu: ({
    children,
    open,
    onOpenChange,
  }: {
    children: React.ReactNode;
    open: boolean;
    onOpenChange: (open: boolean) => void;
  }) => (
    <div data-testid="model-menu" data-open={open}>
      <button onClick={() => onOpenChange(true)}>Open model menu</button>
      {children}
    </div>
  ),
  DropdownMenuTrigger: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DropdownMenuContent: ({
    children,
    onCloseAutoFocus,
  }: {
    children: React.ReactNode;
    onCloseAutoFocus?: (event: Pick<Event, 'preventDefault'>) => void;
  }) => (
    <div>
      <button onClick={() => onCloseAutoFocus?.({ preventDefault: mockPreventCloseAutoFocus })}>
        Complete model menu close
      </button>
      {children}
    </div>
  ),
  DropdownMenuItem: ({
    children,
    onSelect,
    onClick,
  }: {
    children: React.ReactNode;
    onSelect?: () => void;
    onClick?: () => void;
  }) => <button onClick={onClick ?? onSelect}>{children}</button>,
  DropdownMenuSeparator: () => null,
}));

vi.mock('../subcomponents/SwitchModelModal', () => ({
  SwitchModelModal: () => <div data-testid="switch-model-modal" />,
}));

vi.mock('../../localInference/ModelSettingsPanel', () => ({
  ModelSettingsPanel: () => null,
}));

vi.mock('../../../ui/scroll-area', () => ({
  ScrollArea: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

describe('ModelsBottomBar', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockCurrentModel = 'config-model';
    mockCurrentProvider = 'config-provider';
    mockGetProviders.mockResolvedValue([]);
    mockChangeModel.mockResolvedValue(true);
    vi.mocked(window.electron.getSetting).mockResolvedValue(undefined);
  });

  it('shows a loading placeholder while the active session model is still loading', async () => {
    renderWithIntl(
      <ModelsBottomBar
        sessionId="session-123"
        dropdownRef={createDropdownRef()}
        setView={vi.fn()}
        onModelChanged={mockOnModelChanged}
        sessionLoaded={false}
      />
    );

    expect(screen.getByTestId('model-loading-state')).toHaveTextContent('Loading model...');
  });

  it('shows the active session model once the session has loaded', async () => {
    renderWithIntl(
      <ModelsBottomBar
        sessionId="session-123"
        dropdownRef={createDropdownRef()}
        setView={vi.fn()}
        sessionModel="session-model"
        sessionProvider="session-provider"
        onModelChanged={mockOnModelChanged}
        sessionLoaded={true}
      />
    );

    expect(screen.getByText('session-model')).toBeInTheDocument();
    expect(screen.queryByTestId('model-loading-state')).not.toBeInTheDocument();
  });

  it('shows the configured model when there is no active session', async () => {
    renderWithIntl(
      <ModelsBottomBar
        sessionId={null}
        dropdownRef={createDropdownRef()}
        setView={vi.fn()}
        onModelChanged={mockOnModelChanged}
      />
    );

    expect(screen.getByText('config-model')).toBeInTheDocument();
    expect(screen.queryByTestId('model-loading-state')).not.toBeInTheDocument();
  });

  it('opens model overlays after the menu closes with the appropriate focus behavior', () => {
    renderWithIntl(
      <ModelsBottomBar
        sessionId="session-123"
        dropdownRef={createDropdownRef()}
        setView={vi.fn()}
        sessionModel="local-model"
        sessionProvider="local"
        onModelChanged={mockOnModelChanged}
        sessionLoaded={true}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Open model menu' }));
    expect(screen.getByTestId('model-menu')).toHaveAttribute('data-open', 'true');

    fireEvent.click(screen.getByRole('button', { name: 'Local Model Settings' }));
    expect(screen.getByTestId('model-menu')).toHaveAttribute('data-open', 'false');
    expect(
      screen.queryByRole('heading', { name: 'Local Model Settings — Display local-model' })
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Complete model menu close' }));
    expect(
      screen.getByRole('heading', { name: 'Local Model Settings — Display local-model' })
    ).toBeInTheDocument();
    expect(mockPreventCloseAutoFocus).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: '×' }));
    fireEvent.click(screen.getByRole('button', { name: 'Open model menu' }));
    fireEvent.click(screen.getByRole('button', { name: 'Change Model' }));
    expect(screen.getByTestId('model-menu')).toHaveAttribute('data-open', 'false');
    expect(screen.queryByTestId('switch-model-modal')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Complete model menu close' }));
    expect(screen.getByTestId('switch-model-modal')).toBeInTheDocument();
    expect(mockPreventCloseAutoFocus).toHaveBeenCalledOnce();
  });

  it('isolates the current model alias from its provider', async () => {
    const model = '\u202elacoL';
    renderWithIntl(
      <ModelsBottomBar
        sessionId="session-123"
        dropdownRef={createDropdownRef()}
        setView={vi.fn()}
        sessionModel={model}
        sessionProvider="session-provider"
        onModelChanged={mockOnModelChanged}
        sessionLoaded={true}
      />
    );

    const provider = await screen.findByText('Config Provider', { exact: true });
    const label = screen.getByTitle(`Display ${model}`);
    expect(label.tagName).toBe('BDI');
    expect(label).toHaveAttribute('dir', 'auto');
    expect(label).toHaveTextContent(`Display ${model}`);
    expect(provider.tagName).toBe('BDI');
    expect(provider).toHaveAttribute('dir', 'auto');
    expect(provider.parentElement).toHaveClass('flex-shrink-0');
    expect(label).not.toContainElement(provider);
  });

  it.each([
    ['ordinary', 'remote-model'],
    ['RTL', 'نموذج محلي'],
    ['bidi controls', '\u202elacoL'],
    ['long', 'Trusted Local Model — local ' + 'padding-'.repeat(80)],
  ])('keeps the provider separate and routes the raw %s recent model', async (_, model) => {
    vi.mocked(window.electron.getSetting).mockResolvedValue([
      { model, provider: 'attacker-cloud' },
    ]);
    renderWithIntl(
      <ModelsBottomBar
        sessionId="session-123"
        dropdownRef={createDropdownRef()}
        setView={vi.fn()}
        sessionModel="safe-model"
        sessionProvider="safe-provider"
        onModelChanged={mockOnModelChanged}
        sessionLoaded={true}
      />
    );

    const provider = await screen.findByText('attacker-cloud', { exact: true });
    const label = screen.getByTitle(`Display ${model}`);
    expect(label.tagName).toBe('BDI');
    expect(label).toHaveAttribute('dir', 'auto');
    expect(label).toHaveClass('min-w-0', 'truncate');
    expect(label).toHaveTextContent(`Display ${model}`);
    expect(provider.tagName).toBe('BDI');
    expect(provider).toHaveAttribute('dir', 'auto');
    expect(provider.parentElement).toHaveClass('flex-shrink-0');
    expect(provider.parentElement).not.toHaveClass('truncate');
    expect(label).not.toContainElement(provider);

    const item = provider.closest('button')!;
    expect(within(item).getByTitle(`Display ${model}`)).toBe(label);
    expect(item).toHaveAccessibleName(`Display ${model}— attacker-cloud`);
    fireEvent.click(item);
    await waitFor(() =>
      expect(mockChangeModel).toHaveBeenCalledWith('session-123', {
        name: model,
        provider: 'attacker-cloud',
      })
    );
    expect(mockOnModelChanged).toHaveBeenCalledWith({ model, provider: 'attacker-cloud' });
  });
});

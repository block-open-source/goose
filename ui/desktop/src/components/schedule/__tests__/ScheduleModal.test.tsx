import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, type RenderOptions, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ScheduledJobDto } from '@aaif/goose-acp-client';
import { ScheduleModal } from '../ScheduleModal';
import { IntlTestWrapper } from '../../../i18n/test-utils';
import { listSavedRecipes } from '../../../recipe/recipe_management';
import type { RecipeManifest } from '../../../recipe';

vi.mock('../../../recipe/recipe_management', () => ({
  listSavedRecipes: vi.fn(),
  getStorageDirectory: vi.fn(() => ''),
}));

const renderWithIntl = (ui: React.ReactElement, options?: RenderOptions) =>
  render(ui, { wrapper: IntlTestWrapper, ...options });

const existingSchedule = {
  id: 'daily-summary-job',
  cron: '0 0 14 * * *',
} as ScheduledJobDto;

const baseProps = {
  onClose: vi.fn(),
  onSubmit: vi.fn().mockResolvedValue(undefined),
  isLoadingExternally: false,
  apiErrorExternally: null,
  initialDeepLink: null,
};

const savedRecipeManifest = {
  id: 'my-recipe',
  recipe: { title: 'My Recipe', description: 'A test recipe' },
  file_path: '/recipes/my-recipe.yaml',
  last_modified: '',
} as unknown as RecipeManifest;

const alreadyScheduledManifest = {
  id: 'already-scheduled',
  recipe: { title: 'Already Scheduled', description: 'Has a cron' },
  file_path: '/recipes/already-scheduled.yaml',
  last_modified: '',
  schedule_cron: '0 0 9 * * *',
} as unknown as RecipeManifest;

describe('ScheduleModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(listSavedRecipes).mockResolvedValue([savedRecipeManifest]);
  });

  it('preserves the form when the recipe picker is cancelled', async () => {
    const user = userEvent.setup();
    const selectRecipeFile = vi.fn().mockResolvedValue(null);
    window.electron.selectRecipeFile = selectRecipeFile;
    renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.click(screen.getByRole('button', { name: 'Browse for YAML file...' }));

    expect(selectRecipeFile).toHaveBeenCalledOnce();
    expect(screen.queryByText(/Failed to read|Invalid file type/)).not.toBeInTheDocument();
  });

  it('clears a validation error from create mode when reopened to edit a schedule', async () => {
    const user = userEvent.setup();
    const { rerender } = renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.type(screen.getByLabelText(/name/i), 'my-job');
    await user.click(screen.getByRole('button', { name: 'Create Schedule' }));
    await waitFor(() => {
      expect(screen.getByText('Please provide a valid recipe source.')).toBeInTheDocument();
    });

    rerender(<ScheduleModal {...baseProps} isOpen={false} schedule={null} />);
    rerender(<ScheduleModal {...baseProps} isOpen schedule={existingSchedule} />);

    expect(screen.getByText('Edit Schedule')).toBeInTheDocument();
    expect(screen.queryByText('Please provide a valid recipe source.')).not.toBeInTheDocument();
  });

  it('loads saved recipes into a picker and creates a schedule from the selected one', async () => {
    const user = userEvent.setup();
    renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.click(screen.getByRole('button', { name: 'Saved recipes' }));

    await waitFor(() => {
      expect(listSavedRecipes).toHaveBeenCalledTimes(1);
    });

    const picker = within(screen.getByTestId('saved-recipe-picker'));
    await user.click(await picker.findByRole('combobox'));
    const option = await picker.findByRole('option', { name: 'My Recipe' });
    await user.click(option);

    await waitFor(() => {
      expect(screen.getByText('Title: My Recipe')).toBeInTheDocument();
    });

    const createButton = screen.getByRole('button', { name: 'Create Schedule' });
    expect(createButton).not.toBeDisabled();
    await user.click(createButton);

    expect(screen.queryByLabelText(/name/i)).not.toBeInTheDocument();

    await waitFor(() => {
      expect(baseProps.onSubmit).toHaveBeenCalledWith({
        sourceType: 'saved',
        recipeId: 'my-recipe',
        cron: expect.any(String),
      });
    });
  });

  it('shows an empty state when there are no saved recipes', async () => {
    vi.mocked(listSavedRecipes).mockResolvedValue([]);
    const user = userEvent.setup();
    renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.click(screen.getByRole('button', { name: 'Saved recipes' }));

    await waitFor(() => {
      expect(screen.getByText('No saved recipes found.')).toBeInTheDocument();
    });
  });

  it('surfaces recipe list failures instead of an empty state', async () => {
    vi.mocked(listSavedRecipes).mockRejectedValue(new Error('ACP down'));
    const user = userEvent.setup();
    renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.click(screen.getByRole('button', { name: 'Saved recipes' }));

    await waitFor(() => {
      expect(screen.getByText('Failed to load recipes.')).toBeInTheDocument();
    });
    expect(screen.queryByText('No saved recipes found.')).not.toBeInTheDocument();
  });

  it('blocks creating a schedule from an already-scheduled recipe', async () => {
    vi.mocked(listSavedRecipes).mockResolvedValue([alreadyScheduledManifest]);
    const user = userEvent.setup();
    renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.click(screen.getByRole('button', { name: 'Saved recipes' }));
    await waitFor(() => {
      expect(listSavedRecipes).toHaveBeenCalled();
    });

    const picker = within(screen.getByTestId('saved-recipe-picker'));
    await user.click(await picker.findByRole('combobox'));
    await user.click(
      await picker.findByRole('option', { name: /Already Scheduled \(already scheduled\)/i })
    );

    await waitFor(() => {
      expect(
        screen.getByText('This recipe already has a schedule. Edit it from the Recipes list.')
      ).toBeInTheDocument();
    });
    expect(baseProps.onSubmit).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Create Schedule' }));
    expect(baseProps.onSubmit).not.toHaveBeenCalled();
  });

  it('clears a previously parsed recipe when switching source tabs', async () => {
    const user = userEvent.setup();
    renderWithIntl(<ScheduleModal {...baseProps} isOpen schedule={null} />);

    await user.click(screen.getByRole('button', { name: 'Saved recipes' }));
    await waitFor(() => {
      expect(listSavedRecipes).toHaveBeenCalled();
    });

    const picker = within(screen.getByTestId('saved-recipe-picker'));
    await user.click(await picker.findByRole('combobox'));
    await user.click(await picker.findByRole('option', { name: 'My Recipe' }));

    await waitFor(() => {
      expect(screen.getByText('Title: My Recipe')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: 'YAML' }));
    expect(screen.queryByText('Title: My Recipe')).not.toBeInTheDocument();

    await user.type(screen.getByLabelText(/name/i), 'stale-job');
    await user.click(screen.getByRole('button', { name: 'Create Schedule' }));

    await waitFor(() => {
      expect(screen.getByText('Please provide a valid recipe source.')).toBeInTheDocument();
    });
    expect(baseProps.onSubmit).not.toHaveBeenCalled();
  });
});

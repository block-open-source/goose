import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../../../i18n/test-utils';
import ImportRecipeForm from '../ImportRecipeForm';

const recipeMocks = vi.hoisted(() => ({
  parseDeeplink: vi.fn(),
  parseRecipeFromFile: vi.fn(),
}));

vi.mock('../../../recipe', () => recipeMocks);
vi.mock('../../../recipe/recipe_management', () => ({ saveRecipe: vi.fn() }));
vi.mock('../../../toasts', () => ({ toastError: vi.fn(), toastSuccess: vi.fn() }));

const MAX_RECIPE_FILE_SIZE_BYTES = 1024 * 1024;

function renderForm() {
  return render(<ImportRecipeForm isOpen onClose={vi.fn()} onSuccess={vi.fn()} />, {
    wrapper: IntlTestWrapper,
  });
}

function uploadFile(file: File) {
  fireEvent.change(screen.getByLabelText('Recipe File'), {
    target: { files: [file] },
  });
}

function fileWithText(content: string | ArrayBuffer, name: string, text: string) {
  const file = new File([content], name);
  const readText = vi.fn().mockResolvedValue(text);
  Object.defineProperty(file, 'text', { value: readText });
  return { file, readText };
}

beforeEach(() => {
  vi.clearAllMocks();
  recipeMocks.parseRecipeFromFile.mockResolvedValue({ title: 'Imported recipe' });
});

describe('ImportRecipeForm file size enforcement', () => {
  it('rejects an oversized file before reading or parsing it', async () => {
    const { file, readText } = fileWithText(
      new ArrayBuffer(MAX_RECIPE_FILE_SIZE_BYTES + 1),
      'oversized.yaml',
      'title: oversized'
    );
    renderForm();

    uploadFile(file);
    await act(async () => {});

    expect(readText).not.toHaveBeenCalled();
    expect(recipeMocks.parseRecipeFromFile).not.toHaveBeenCalled();
    expect(screen.getByText('File is too large, max size is 1MB')).toBeInTheDocument();
  });

  it('reads and parses a valid YAML file below the limit', async () => {
    const content = 'title: Valid recipe';
    const { file, readText } = fileWithText(content, 'recipe.yaml', content);
    renderForm();

    uploadFile(file);

    await waitFor(() => expect(recipeMocks.parseRecipeFromFile).toHaveBeenCalledWith(content));
    expect(readText).toHaveBeenCalledOnce();
  });

  it('reads and parses a JSON file exactly at the limit', async () => {
    const content = `{"value":"${'x'.repeat(MAX_RECIPE_FILE_SIZE_BYTES - 12)}"}`;
    expect(new Blob([content]).size).toBe(MAX_RECIPE_FILE_SIZE_BYTES);
    const { file, readText } = fileWithText(content, 'recipe.json', content);
    renderForm();

    uploadFile(file);

    await waitFor(() => expect(recipeMocks.parseRecipeFromFile).toHaveBeenCalledWith(content));
    expect(readText).toHaveBeenCalledOnce();
  });
});

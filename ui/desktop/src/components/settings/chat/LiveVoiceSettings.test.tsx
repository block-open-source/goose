import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../../../i18n/test-utils';
import { useConfig } from '../../ConfigContext';
import { LiveVoiceSettings } from './LiveVoiceSettings';

vi.mock('../../ConfigContext', () => ({ useConfig: vi.fn() }));

describe('LiveVoiceSettings', () => {
  const read = vi.fn();
  const upsert = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    read.mockResolvedValue(true);
    vi.mocked(useConfig).mockReturnValue({ read, upsert } as unknown as ReturnType<
      typeof useConfig
    >);
  });

  it('loads and persists the Live voice toggle', async () => {
    render(<LiveVoiceSettings />, { wrapper: IntlTestWrapper });

    const toggle = screen.getByRole('switch', { name: 'Live voice' });
    await waitFor(() => expect(toggle).toBeChecked());
    expect(read).toHaveBeenCalledWith('GOOSE_LIVE_VOICE_ENABLED', false);

    fireEvent.click(toggle);

    await waitFor(() =>
      expect(upsert).toHaveBeenCalledWith('GOOSE_LIVE_VOICE_ENABLED', false, false)
    );
    await waitFor(() => expect(toggle).not.toBeChecked());
  });
});

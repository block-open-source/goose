import { render, screen, type RenderOptions, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { ScheduledJobDto } from '@aaif/goose-acp-client';
import { IntlTestWrapper } from '../../../i18n/test-utils';
import { CronPicker } from '../CronPicker';

const renderWithIntl = (ui: React.ReactElement, options?: RenderOptions) =>
  render(ui, { wrapper: IntlTestWrapper, ...options });

const getLastCron = (onChange: ReturnType<typeof vi.fn>) => {
  const calls = onChange.mock.calls;
  return calls[calls.length - 1]?.[0];
};

const scheduledJob = (cron: string): ScheduledJobDto => ({
  id: 'quarterly-report',
  source: 'dummy.yaml',
  cron,
  currentlyRunning: false,
  paused: false,
});

describe('CronPicker', () => {
  it('generates quarterly cron expressions from the quarter preset', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();

    renderWithIntl(<CronPicker schedule={null} onChange={onChange} isValid={vi.fn()} />);

    await user.selectOptions(screen.getAllByRole('combobox')[0], 'quarter');

    await waitFor(() => {
      expect(getLastCron(onChange)).toBe('0 0 14 1 1,4,7,10 *');
    });

    const dayInput = screen.getAllByRole('spinbutton')[0];
    await user.clear(dayInput);
    await user.type(dayInput, '31');
    await user.selectOptions(screen.getAllByRole('combobox')[1], '2');

    await waitFor(() => {
      expect(dayInput).toHaveValue(28);
      expect(getLastCron(onChange)).toBe('0 0 14 28 2,5,8,11 *');
    });
  });

  it('marks invalid quarter day input as invalid instead of silently clamping to day one', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const isValid = vi.fn();

    renderWithIntl(<CronPicker schedule={null} onChange={onChange} isValid={isValid} />);

    await user.selectOptions(screen.getAllByRole('combobox')[0], 'quarter');
    const dayInput = screen.getAllByRole('spinbutton')[0];
    await user.clear(dayInput);
    await user.type(dayInput, '0');

    await waitFor(() => {
      expect(dayInput).toHaveValue(0);
      expect(isValid).toHaveBeenLastCalledWith(false);
      expect(getLastCron(onChange)).toBe('0 0 14 0 1,4,7,10 *');
    });
  });

  it('uses custom cron for cron expressions that presets cannot represent', async () => {
    const onChange = vi.fn();

    renderWithIntl(
      <CronPicker
        schedule={scheduledJob('0 0 14 31 1,4,7,10 *')}
        onChange={onChange}
        isValid={vi.fn()}
      />
    );

    const [periodSelect] = screen.getAllByRole('combobox');

    await waitFor(() => {
      expect(periodSelect).toHaveValue('custom');
      expect(screen.getByLabelText('Cron expression')).toHaveValue('0 0 14 31 1,4,7,10 *');
      expect(getLastCron(onChange)).toBe('0 0 14 31 1,4,7,10 *');
    });
  });

  it('uses custom cron when seconds cannot be represented by presets', async () => {
    const onChange = vi.fn();

    renderWithIntl(
      <CronPicker schedule={scheduledJob('* 0 14 * * *')} onChange={onChange} isValid={vi.fn()} />
    );

    const [periodSelect] = screen.getAllByRole('combobox');

    await waitFor(() => {
      expect(periodSelect).toHaveValue('custom');
      expect(screen.getByLabelText('Cron expression')).toHaveValue('* 0 14 * * *');
      expect(getLastCron(onChange)).toBe('* 0 14 * * *');
    });
  });

  it('generates cron from custom cron input', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();

    renderWithIntl(<CronPicker schedule={null} onChange={onChange} isValid={vi.fn()} />);

    await user.selectOptions(screen.getAllByRole('combobox')[0], 'custom');
    const customCronInput = screen.getByLabelText('Cron expression');
    await user.clear(customCronInput);
    await user.type(customCronInput, '0 9 31 1,4,7,10 *');

    await waitFor(() => {
      expect(getLastCron(onChange)).toBe('0 9 31 1,4,7,10 *');
    });
  });

  it('uses the current preset cron when switching to custom cron', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();

    renderWithIntl(<CronPicker schedule={null} onChange={onChange} isValid={vi.fn()} />);

    const [periodSelect] = screen.getAllByRole('combobox');
    await user.selectOptions(periodSelect, 'quarter');
    await user.selectOptions(screen.getAllByRole('combobox')[1], '2');

    const dayInput = screen.getAllByRole('spinbutton')[0];
    await user.clear(dayInput);
    await user.type(dayInput, '15');
    await user.selectOptions(periodSelect, 'custom');

    await waitFor(() => {
      expect(screen.getByLabelText('Cron expression')).toHaveValue('0 0 14 15 2,5,8,11 *');
      expect(getLastCron(onChange)).toBe('0 0 14 15 2,5,8,11 *');
    });
  });

  it('marks invalid custom cron input as invalid', async () => {
    const user = userEvent.setup();
    const isValid = vi.fn();

    renderWithIntl(<CronPicker schedule={null} onChange={vi.fn()} isValid={isValid} />);

    await user.selectOptions(screen.getAllByRole('combobox')[0], 'custom');
    const customCronInput = screen.getByLabelText('Cron expression');
    await user.clear(customCronInput);
    await user.type(customCronInput, '99 0 14 * * *');

    await waitFor(() => {
      expect(isValid).toHaveBeenLastCalledWith(false);
    });
  });
});

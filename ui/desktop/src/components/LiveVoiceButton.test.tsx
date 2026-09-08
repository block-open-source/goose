import { render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { describe, expect, it } from 'vitest';
import { IntlTestWrapper } from '../i18n/test-utils';
import { LiveVoiceButton } from './LiveVoiceButton';

const baseProps: ComponentProps<typeof LiveVoiceButton> = {
  status: 'ready',
  composerEmpty: true,
};

function renderButton(props: Partial<typeof baseProps> = {}) {
  return render(<LiveVoiceButton {...baseProps} {...props} />, { wrapper: IntlTestWrapper });
}

describe('LiveVoiceButton', () => {
  it('shows an eligible state for an empty composer', () => {
    renderButton();

    const button = screen.getByTestId('live-voice-button');
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('data-eligible', 'true');
    expect(button).toHaveAccessibleName('Start Live voice');
  });

  it.each([
    ['feature_disabled', 'Live voice is disabled'],
    ['provider_unavailable', 'Live voice provider is not configured'],
    ['session_busy', 'Live voice is unavailable while this chat is busy'],
    ['requires_autonomous_mode', 'Live voice requires Autonomous mode'],
  ] as const)('shows the %s reason', (status, label) => {
    renderButton({ status });

    const button = screen.getByTestId('live-voice-button');
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('data-eligible', 'false');
    expect(button).toHaveAccessibleName(label);
  });

  it('applies the Desktop empty-composer gate', () => {
    renderButton({ composerEmpty: false });

    const button = screen.getByTestId('live-voice-button');
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('data-eligible', 'false');
    expect(button).toHaveAccessibleName('Clear the message and attachments to use Live voice');
  });

  it('does not render without eligibility for the displayed session', () => {
    renderButton({ status: null });
    expect(screen.queryByTestId('live-voice-button')).not.toBeInTheDocument();
  });
});

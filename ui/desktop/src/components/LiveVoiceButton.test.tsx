import { render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../i18n/test-utils';
import { LiveVoiceButton } from './LiveVoiceButton';

const baseProps: ComponentProps<typeof LiveVoiceButton> = {
  availability: 'ready',
  composerEmpty: true,
  phase: 'idle',
  muted: false,
  onStart: vi.fn(),
  onStop: vi.fn(),
  onToggleMute: vi.fn(),
};

function renderButton(props: Partial<typeof baseProps> = {}) {
  return render(<LiveVoiceButton {...baseProps} {...props} />, { wrapper: IntlTestWrapper });
}

describe('LiveVoiceButton', () => {
  it('shows an eligible state for an empty composer', () => {
    renderButton();

    const button = screen.getByTestId('live-voice-button');
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute('data-eligible', 'true');
    expect(button).toHaveAccessibleName('Start Live voice');
  });

  it.each([
    ['feature_disabled', 'Live voice is disabled'],
    ['provider_unavailable', 'Live voice provider is not configured'],
    ['session_busy', 'Live voice is unavailable while this chat is busy'],
    ['requires_autonomous_mode', 'Live voice requires Autonomous mode'],
  ] as const)('shows the %s reason', (availability, label) => {
    renderButton({ availability });

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
    renderButton({ availability: null });
    expect(screen.queryByTestId('live-voice-button')).not.toBeInTheDocument();
  });

  it('starts when idle and stops while connecting or live', () => {
    const onStart = vi.fn();
    const onStop = vi.fn();
    const { container, rerender } = renderButton({ onStart, onStop });

    screen.getByTestId('live-voice-button').click();
    expect(onStart).toHaveBeenCalledOnce();

    rerender(
      <LiveVoiceButton {...baseProps} phase="connecting" onStart={onStart} onStop={onStop} />
    );
    expect(container.querySelector('.animate-spin')).toBeInTheDocument();
    screen.getByTestId('live-voice-button').click();
    expect(onStop).toHaveBeenCalledOnce();

    rerender(<LiveVoiceButton {...baseProps} phase="live" onStart={onStart} onStop={onStop} />);
    screen.getByTestId('live-voice-button').click();
    expect(onStop).toHaveBeenCalledTimes(2);
  });

  it('disables the stopping state', () => {
    renderButton({ phase: 'stopping' });
    expect(screen.getByTestId('live-voice-button')).toBeDisabled();
  });

  it('toggles microphone mute while live', () => {
    const onToggleMute = vi.fn();
    const { rerender } = renderButton({ phase: 'live', onToggleMute });

    screen.getByTestId('live-voice-mute-button').click();
    expect(onToggleMute).toHaveBeenCalledOnce();
    expect(screen.getByTestId('live-voice-mute-button')).toHaveAccessibleName('Mute microphone');

    rerender(<LiveVoiceButton {...baseProps} phase="live" muted onToggleMute={onToggleMute} />);
    expect(screen.getByTestId('live-voice-mute-button')).toHaveAccessibleName('Unmute microphone');
  });
});

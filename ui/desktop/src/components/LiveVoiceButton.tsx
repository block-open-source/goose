import type { LiveVoiceStatus as LiveVoiceAvailability } from '@aaif/goose-sdk';
import { AudioLines, LoaderCircle, Mic, MicOff, Square } from 'lucide-react';
import { defineMessages, useIntl } from '../i18n';
import { cn } from '../utils';
import { Button } from './ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/Tooltip';
import type { LiveVoicePhase } from '../liveVoice/useLiveVoice';

const i18n = defineMessages({
  ready: {
    id: 'liveVoice.ready',
    defaultMessage: 'Start Live voice',
  },
  featureDisabled: {
    id: 'liveVoice.featureDisabled',
    defaultMessage: 'Live voice is disabled',
  },
  providerUnavailable: {
    id: 'liveVoice.providerUnavailable',
    defaultMessage: 'Live voice provider is not configured',
  },
  sessionBusy: {
    id: 'liveVoice.sessionBusy',
    defaultMessage: 'Live voice is unavailable while this chat is busy',
  },
  requiresAutonomousMode: {
    id: 'liveVoice.requiresAutonomousMode',
    defaultMessage: 'Live voice requires Autonomous mode',
  },
  emptyComposerRequired: {
    id: 'liveVoice.emptyComposerRequired',
    defaultMessage: 'Clear the message and attachments to use Live voice',
  },
  connecting: {
    id: 'liveVoice.connecting',
    defaultMessage: 'Connecting Live voice',
  },
  live: {
    id: 'liveVoice.live',
    defaultMessage: 'Stop Live voice',
  },
  stopping: {
    id: 'liveVoice.stopping',
    defaultMessage: 'Stopping Live voice',
  },
  error: {
    id: 'liveVoice.error',
    defaultMessage: 'Live voice failed. Try again',
  },
  mute: {
    id: 'liveVoice.mute',
    defaultMessage: 'Mute microphone',
  },
  unmute: {
    id: 'liveVoice.unmute',
    defaultMessage: 'Unmute microphone',
  },
});

const availabilityMessages = {
  ready: i18n.ready,
  feature_disabled: i18n.featureDisabled,
  provider_unavailable: i18n.providerUnavailable,
  session_busy: i18n.sessionBusy,
  requires_autonomous_mode: i18n.requiresAutonomousMode,
} satisfies Record<LiveVoiceAvailability, (typeof i18n)[keyof typeof i18n]>;

interface LiveVoiceButtonProps {
  availability: LiveVoiceAvailability | null;
  composerEmpty: boolean;
  phase: LiveVoicePhase;
  muted: boolean;
  onStart: () => void;
  onStop: () => void;
  onToggleMute: () => void;
}

export function LiveVoiceButton({
  availability,
  composerEmpty,
  phase,
  muted,
  onStart,
  onStop,
  onToggleMute,
}: LiveVoiceButtonProps) {
  const intl = useIntl();
  if (availability === null) return null;

  const eligible = availability === 'ready' && composerEmpty;
  const stopping = phase === 'stopping';
  const message =
    phase === 'connecting'
      ? i18n.connecting
      : phase === 'live'
        ? i18n.live
        : phase === 'stopping'
          ? i18n.stopping
          : phase === 'error'
            ? i18n.error
            : composerEmpty
              ? availabilityMessages[availability]
              : i18n.emptyComposerRequired;
  const label = intl.formatMessage(message);
  const disabled = phase === 'idle' ? !eligible : stopping;
  const canStop = phase === 'connecting' || phase === 'live';

  return (
    <>
      {phase === 'live' && (
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              shape="round"
              onClick={onToggleMute}
              aria-label={intl.formatMessage(muted ? i18n.unmute : i18n.mute)}
              data-testid="live-voice-mute-button"
              aria-pressed={muted}
            >
              {muted ? <MicOff className="w-4 h-4" /> : <Mic className="w-4 h-4" />}
            </Button>
          </TooltipTrigger>
          <TooltipContent>{intl.formatMessage(muted ? i18n.unmute : i18n.mute)}</TooltipContent>
        </Tooltip>
      )}
      <Tooltip>
        <TooltipTrigger asChild>
          <span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              shape="round"
              disabled={disabled}
              onClick={canStop ? onStop : onStart}
              aria-label={label}
              data-testid="live-voice-button"
              data-eligible={eligible}
              data-phase={phase}
              className={cn(
                'transition-colors',
                canStop && 'text-red-500 hover:text-red-600 cursor-pointer',
                phase === 'error' && 'text-red-500 hover:text-red-600 cursor-pointer',
                phase === 'idle' && eligible && 'text-text-primary/70 cursor-pointer',
                disabled && 'text-text-secondary opacity-50'
              )}
            >
              {phase === 'connecting' || stopping ? (
                <LoaderCircle className="w-4 h-4 animate-spin" />
              ) : canStop ? (
                <Square className="w-4 h-4" />
              ) : (
                <AudioLines className="w-4 h-4" />
              )}
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
    </>
  );
}

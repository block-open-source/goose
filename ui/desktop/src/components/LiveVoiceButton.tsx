import type { LiveVoiceStatus } from '@aaif/goose-sdk';
import { AudioLines, LoaderCircle, Square } from 'lucide-react';
import { defineMessages, useIntl } from '../i18n';
import { cn } from '../utils';
import { Button } from './ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/Tooltip';
import type { LiveVoiceUiState } from '../liveVoice/useLiveVoice';

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
});

const statusMessages = {
  ready: i18n.ready,
  feature_disabled: i18n.featureDisabled,
  provider_unavailable: i18n.providerUnavailable,
  session_busy: i18n.sessionBusy,
  requires_autonomous_mode: i18n.requiresAutonomousMode,
} satisfies Record<LiveVoiceStatus, (typeof i18n)[keyof typeof i18n]>;

interface LiveVoiceButtonProps {
  status: LiveVoiceStatus | null;
  composerEmpty: boolean;
  state: LiveVoiceUiState;
  onStart: () => void;
  onStop: () => void;
}

export function LiveVoiceButton({
  status,
  composerEmpty,
  state,
  onStart,
  onStop,
}: LiveVoiceButtonProps) {
  const intl = useIntl();
  if (status === null) return null;

  const eligible = status === 'ready' && composerEmpty;
  const busy = state === 'connecting' || state === 'stopping';
  const message =
    state === 'connecting'
      ? i18n.connecting
      : state === 'live'
        ? i18n.live
        : state === 'stopping'
          ? i18n.stopping
          : state === 'error'
            ? i18n.error
            : composerEmpty
              ? statusMessages[status]
              : i18n.emptyComposerRequired;
  const label = intl.formatMessage(message);
  const disabled = state === 'idle' ? !eligible : busy;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            shape="round"
            disabled={disabled}
            onClick={state === 'live' ? onStop : onStart}
            aria-label={label}
            data-testid="live-voice-button"
            data-eligible={eligible}
            data-state={state}
            className={cn(
              'transition-colors',
              state === 'live' && 'text-red-500 hover:text-red-600 cursor-pointer',
              state === 'error' && 'text-red-500 hover:text-red-600 cursor-pointer',
              state === 'idle' && eligible && 'text-text-primary/70 cursor-pointer',
              disabled && 'text-text-secondary opacity-50'
            )}
          >
            {busy ? (
              <LoaderCircle className="w-4 h-4 animate-spin" />
            ) : state === 'live' ? (
              <Square className="w-4 h-4" />
            ) : (
              <AudioLines className="w-4 h-4" />
            )}
          </Button>
        </span>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

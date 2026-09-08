import type { LiveVoiceStatus } from '@aaif/goose-sdk';
import { AudioLines } from 'lucide-react';
import { defineMessages, useIntl } from '../i18n';
import { cn } from '../utils';
import { Button } from './ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/Tooltip';

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
}

export function LiveVoiceButton({ status, composerEmpty }: LiveVoiceButtonProps) {
  const intl = useIntl();
  if (status === null) return null;

  const eligible = status === 'ready' && composerEmpty;
  const message = composerEmpty ? statusMessages[status] : i18n.emptyComposerRequired;
  const label = intl.formatMessage(message);

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            shape="round"
            disabled
            aria-label={label}
            data-testid="live-voice-button"
            data-eligible={eligible}
            className={cn(
              'transition-colors',
              eligible ? 'text-text-primary/70' : 'text-text-secondary opacity-50'
            )}
          >
            <AudioLines className="w-4 h-4" />
          </Button>
        </span>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

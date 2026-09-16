import { ModeSection } from '../mode/ModeSection';
import { DictationSettings } from '../dictation/DictationSettings';
import { SecurityToggle } from '../security/SecurityToggle';
import { ResponseStylesSection } from '../response_styles/ResponseStylesSection';
import { GoosehintsSection } from './GoosehintsSection';
import { LiveVoiceSettings } from './LiveVoiceSettings';
import { SpellcheckToggle } from './SpellcheckToggle';
import { LegacyAgentLoopToggle } from './LegacyAgentLoopToggle';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../../ui/card';
import { defineMessages, useIntl } from '../../../i18n';

const i18n = defineMessages({
  modeTitle: {
    id: 'chatSettings.modeTitle',
    defaultMessage: 'Default Mode',
  },
  modeDescription: {
    id: 'chatSettings.modeDescription',
    defaultMessage:
      'Choose the default mode Goose uses for new sessions. Existing sessions keep their current mode.',
  },
  responseStylesTitle: {
    id: 'chatSettings.responseStylesTitle',
    defaultMessage: 'Response Styles',
  },
  responseStylesDescription: {
    id: 'chatSettings.responseStylesDescription',
    defaultMessage: 'Choose how Goose should format and style its responses',
  },
});

export default function ChatSettingsSection() {
  const intl = useIntl();

  return (
    <div className="space-y-4 pr-4 pb-8 mt-1">
      <Card className="pb-2 rounded-lg">
        <CardHeader className="pb-0">
          <CardTitle className="">{intl.formatMessage(i18n.modeTitle)}</CardTitle>
          <CardDescription>{intl.formatMessage(i18n.modeDescription)}</CardDescription>
        </CardHeader>
        <CardContent className="px-2">
          <ModeSection />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-2">
          <GoosehintsSection />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-2">
          <LiveVoiceSettings />
          <DictationSettings />
          <SpellcheckToggle />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardHeader className="pb-0">
          <CardTitle className="">{intl.formatMessage(i18n.responseStylesTitle)}</CardTitle>
          <CardDescription>{intl.formatMessage(i18n.responseStylesDescription)}</CardDescription>
        </CardHeader>
        <CardContent className="px-2">
          <ResponseStylesSection />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-4 pt-4">
          <LegacyAgentLoopToggle />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-2">
          <SecurityToggle />
        </CardContent>
      </Card>
    </div>
  );
}

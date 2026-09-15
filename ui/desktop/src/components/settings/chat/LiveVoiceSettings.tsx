import { useEffect, useState } from 'react';
import { useConfig } from '../../ConfigContext';
import { Switch } from '../../ui/switch';
import { defineMessages, useIntl } from '../../../i18n';

const LIVE_VOICE_ENABLED_CONFIG_KEY = 'GOOSE_LIVE_VOICE_ENABLED';

const i18n = defineMessages({
  title: {
    id: 'liveVoiceSettings.title',
    defaultMessage: 'Live voice',
  },
  description: {
    id: 'liveVoiceSettings.description',
    defaultMessage: 'Have a real-time voice conversation with Goose',
  },
});

export const LiveVoiceSettings = () => {
  const intl = useIntl();
  const { read, upsert } = useConfig();
  const [enabled, setEnabled] = useState<boolean | null>(null);

  useEffect(() => {
    void read(LIVE_VOICE_ENABLED_CONFIG_KEY, false).then((value) => {
      setEnabled(value === true || value === 1 || value === '1' || value === 'true');
    });
  }, [read]);

  const handleToggle = async (checked: boolean) => {
    await upsert(LIVE_VOICE_ENABLED_CONFIG_KEY, checked, false);
    setEnabled(checked);
  };

  return (
    <div className="flex items-center justify-between py-2 px-2 hover:bg-background-secondary rounded-lg transition-all">
      <div>
        <h3 className="text-text-primary">{intl.formatMessage(i18n.title)}</h3>
        <p className="text-xs text-text-secondary max-w-md mt-[2px]">
          {intl.formatMessage(i18n.description)}
        </p>
      </div>
      <Switch
        checked={enabled ?? false}
        disabled={enabled === null}
        onCheckedChange={handleToggle}
        variant="mono"
        aria-label={intl.formatMessage(i18n.title)}
      />
    </div>
  );
};

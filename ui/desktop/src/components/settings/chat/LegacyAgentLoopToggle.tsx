import { useEffect, useState } from 'react';
import { defineMessages, useIntl } from '../../../i18n';
import { Switch } from '../../ui/switch';

const i18n = defineMessages({
  title: {
    id: 'settings.legacyAgentLoop.title',
    defaultMessage: 'Use Legacy Agent Loop',
  },
  description: {
    id: 'settings.legacyAgentLoop.description',
    defaultMessage: 'Fall back to the classic agent loop if the new one causes issues',
  },
});

export function LegacyAgentLoopToggle() {
  const intl = useIntl();
  const [enabled, setEnabled] = useState(false);

  useEffect(() => {
    window.electron.getSetting('useLegacyAgentLoop').then((value) => setEnabled(value ?? false));
  }, []);

  const handleToggle = async (checked: boolean) => {
    setEnabled(checked);
    await window.electron.setSetting('useLegacyAgentLoop', checked);
  };

  return (
    <div className="flex items-center justify-between">
      <div>
        <h3 className="text-text-primary text-xs">{intl.formatMessage(i18n.title)}</h3>
        <p className="text-xs text-text-secondary max-w-md mt-[2px]">
          {intl.formatMessage(i18n.description)}
        </p>
      </div>
      <div className="flex items-center">
        <Switch checked={enabled} onCheckedChange={handleToggle} variant="mono" />
      </div>
    </div>
  );
}

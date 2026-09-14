import { Input } from '../../../ui/input';
import { defineMessages, useIntl } from '../../../../i18n';

const i18n = defineMessages({
  commandLabel: {
    id: 'extensionConfigFields.commandLabel',
    defaultMessage: 'Command',
  },
  commandPlaceholder: {
    id: 'extensionConfigFields.commandPlaceholder',
    defaultMessage: 'e.g. npx -y @modelcontextprotocol/my-extension [filepath]',
  },
  commandRequired: {
    id: 'extensionConfigFields.commandRequired',
    defaultMessage: 'Command is required',
  },
  endpointLabel: {
    id: 'extensionConfigFields.endpointLabel',
    defaultMessage: 'Endpoint',
  },
  endpointPlaceholder: {
    id: 'extensionConfigFields.endpointPlaceholder',
    defaultMessage: 'Enter endpoint URL...',
  },
  endpointRequired: {
    id: 'extensionConfigFields.endpointRequired',
    defaultMessage: 'Endpoint URL is required',
  },
});

interface ExtensionConfigFieldsProps {
  type: 'stdio' | 'streamable_http' | 'builtin';
  full_cmd: string;
  endpoint: string;
  onChange: (key: string, value: string) => void;
  submitAttempted?: boolean;
  isValid?: boolean;
}

export default function ExtensionConfigFields({
  type,
  full_cmd,
  endpoint,
  onChange,
  submitAttempted = false,
  isValid,
}: ExtensionConfigFieldsProps) {
  const intl = useIntl();

  if (type === 'stdio') {
    return (
      <div className="space-y-4">
        <div>
          <label className="text-sm font-medium mb-2 block text-text-primary">
            {intl.formatMessage(i18n.commandLabel)}
          </label>
          <div className="relative">
            <Input
              value={full_cmd}
              onChange={(e) => onChange('cmd', e.target.value)}
              placeholder={intl.formatMessage(i18n.commandPlaceholder)}
              className={`w-full ${!submitAttempted || isValid ? 'border-border-primary' : 'border-red-500'} text-text-primary`}
            />
            {submitAttempted && !isValid && (
              <div className="absolute text-xs text-red-500 mt-1">
                {intl.formatMessage(i18n.commandRequired)}
              </div>
            )}
          </div>
        </div>
      </div>
    );
  } else {
    return (
      <div>
        <label className="text-sm font-medium mb-2 block text-text-primary">
          {intl.formatMessage(i18n.endpointLabel)}
        </label>
        <div className="relative">
          <Input
            value={endpoint}
            onChange={(e) => onChange('endpoint', e.target.value)}
            placeholder={intl.formatMessage(i18n.endpointPlaceholder)}
            className={`w-full ${!submitAttempted || isValid ? 'border-border-primary' : 'border-red-500'} text-text-primary`}
          />
          {submitAttempted && !isValid && (
            <div className="absolute text-xs text-red-500 mt-1">
              {intl.formatMessage(i18n.endpointRequired)}
            </div>
          )}
        </div>
      </div>
    );
  }
}

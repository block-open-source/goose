import { Input } from '../../../ui/input';
import { Select } from '../../../ui/Select';
import { defineMessages, useIntl } from '../../../../i18n';

const i18n = defineMessages({
  extensionName: {
    id: 'extensionInfoFields.extensionName',
    defaultMessage: 'Extension Name',
  },
  extensionNamePlaceholder: {
    id: 'extensionInfoFields.extensionNamePlaceholder',
    defaultMessage: 'Enter extension name...',
  },
  nameRequired: {
    id: 'extensionInfoFields.nameRequired',
    defaultMessage: 'Name is required',
  },
  typeLabel: {
    id: 'extensionInfoFields.typeLabel',
    defaultMessage: 'Type',
  },
  typeStdio: {
    id: 'extensionInfoFields.typeStdio',
    defaultMessage: 'STDIO',
  },
  typeHttp: {
    id: 'extensionInfoFields.typeHttp',
    defaultMessage: 'HTTP',
  },
  typeStandardIo: {
    id: 'extensionInfoFields.typeStandardIo',
    defaultMessage: 'Standard IO (STDIO)',
  },
  typeStreamableHttp: {
    id: 'extensionInfoFields.typeStreamableHttp',
    defaultMessage: 'Streamable HTTP',
  },
  descriptionLabel: {
    id: 'extensionInfoFields.descriptionLabel',
    defaultMessage: 'Description',
  },
  descriptionPlaceholder: {
    id: 'extensionInfoFields.descriptionPlaceholder',
    defaultMessage: 'Optional description...',
  },
});

interface ExtensionInfoFieldsProps {
  name: string;
  type: 'stdio' | 'streamable_http' | 'builtin';
  description: string;
  onChange: (key: string, value: string) => void;
  submitAttempted: boolean;
}

export default function ExtensionInfoFields({
  name,
  type,
  description,
  onChange,
  submitAttempted,
}: ExtensionInfoFieldsProps) {
  const intl = useIntl();

  const isNameValid = () => {
    return name.trim() !== '';
  };

  return (
    <div className="flex flex-col gap-4 mb-6">
      {/* Top row with Name and Type side by side */}
      <div className="flex justify-between gap-4">
        <div className="flex-1">
          <label className="text-sm font-medium mb-2 block text-text-primary">
            {intl.formatMessage(i18n.extensionName)}
          </label>
          <div className="relative">
            <Input
              value={name}
              onChange={(e) => onChange('name', e.target.value)}
              placeholder={intl.formatMessage(i18n.extensionNamePlaceholder)}
              className={`${!submitAttempted || isNameValid() ? 'border-border-primary' : 'border-red-500'} text-text-primary focus:border-border-primary`}
            />
            {submitAttempted && !isNameValid() && (
              <div className="absolute text-xs text-red-500 mt-1">
                {intl.formatMessage(i18n.nameRequired)}
              </div>
            )}
          </div>
        </div>

        {/* Type Dropdown */}
        <div className="w-[200px]">
          <label className="text-sm font-medium mb-2 block text-text-primary">
            {intl.formatMessage(i18n.typeLabel)}
          </label>
          <Select
            value={{
              value: type,
              label:
                type === 'stdio'
                  ? intl.formatMessage(i18n.typeStdio)
                  : type === 'streamable_http'
                    ? intl.formatMessage(i18n.typeHttp)
                    : type.toUpperCase(),
            }}
            onChange={(newValue: unknown) => {
              const option = newValue as { value: string; label: string } | null;
              if (option) {
                onChange('type', option.value);
              }
            }}
            options={[
              { value: 'stdio', label: intl.formatMessage(i18n.typeStandardIo) },
              { value: 'streamable_http', label: intl.formatMessage(i18n.typeStreamableHttp) },
            ]}
            isSearchable={false}
          />
        </div>
      </div>

      {/* Bottom row with Description spanning full width */}
      <div className="w-full">
        <label className="text-sm font-medium mb-2 block text-text-primary">
          {intl.formatMessage(i18n.descriptionLabel)}
        </label>
        <div className="relative">
          <Input
            value={description}
            onChange={(e) => onChange('description', e.target.value)}
            placeholder={intl.formatMessage(i18n.descriptionPlaceholder)}
            className={`text-text-primary focus:border-border-primary`}
          />
        </div>
      </div>
    </div>
  );
}

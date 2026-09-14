import React, { useState, useEffect } from 'react';
import { IoIosCloseCircle, IoIosWarning, IoIosInformationCircle } from 'react-icons/io';
import { FaPencilAlt, FaSave } from 'react-icons/fa';
import { cn } from '../../utils';
import { errorMessage } from '../../utils/conversionUtils';
import { Alert, AlertType } from './types';
import { useConfig } from '../ConfigContext';
import { defineMessages, useIntl } from '../../i18n';

const alertIcons: Record<AlertType, React.ReactNode> = {
  [AlertType.Error]: <IoIosCloseCircle className="h-5 w-5" />,
  [AlertType.Warning]: <IoIosWarning className="h-5 w-5" />,
  [AlertType.Info]: <IoIosInformationCircle className="h-5 w-5" />,
};

interface AlertBoxProps {
  alert: Alert;
  className?: string;
  compactButtonEnabled?: boolean;
}

const i18n = defineMessages({
  autoCompactAt: {
    id: 'alertBox.autoCompactAt',
    defaultMessage: 'Auto compact at',
  },
  compactNow: {
    id: 'alertBox.compactNow',
    defaultMessage: 'Compact now',
  },
  failedToSaveThreshold: {
    id: 'alertBox.failedToSaveThreshold',
    defaultMessage: 'Failed to save threshold: {error}',
  },
});

const alertStyles: Record<AlertType, string> = {
  [AlertType.Error]: 'bg-[#d7040e] text-white',
  [AlertType.Warning]: 'bg-[#cc4b03] text-white',
  [AlertType.Info]: 'dark:bg-white dark:text-black bg-black text-white',
};

export const AlertBox = ({ alert, className }: AlertBoxProps) => {
  const intl = useIntl();
  const { read, upsert } = useConfig();
  const [isEditingThreshold, setIsEditingThreshold] = useState(false);
  const [loadedThreshold, setLoadedThreshold] = useState<number>(0.8);
  const [thresholdValue, setThresholdValue] = useState(80);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    const loadThreshold = async () => {
      try {
        const threshold = await read('GOOSE_AUTO_COMPACT_THRESHOLD', false);
        if (threshold !== undefined && threshold !== null && typeof threshold === 'number') {
          setLoadedThreshold(threshold);
          setThresholdValue(Math.max(1, Math.min(99, Math.round(threshold * 100))));
        }
      } catch (err) {
        console.error('Error fetching auto-compact threshold:', err);
      }
    };

    loadThreshold();
  }, [read]);

  const currentThreshold = loadedThreshold;

  const handleSaveThreshold = async () => {
    if (isSaving) return; // Prevent double-clicks

    let validThreshold = Math.max(1, Math.min(99, thresholdValue));
    if (validThreshold !== thresholdValue) {
      setThresholdValue(validThreshold);
    }

    setIsSaving(true);
    try {
      const newThreshold = validThreshold / 100; // Convert percentage to decimal

      await upsert('GOOSE_AUTO_COMPACT_THRESHOLD', newThreshold, false);

      setIsEditingThreshold(false);
      setLoadedThreshold(newThreshold);

      // Notify parent component of the threshold change
      if (alert.onThresholdChange) {
        alert.onThresholdChange(newThreshold);
      }
    } catch (error) {
      console.error('Error saving threshold:', error);
      window.alert(intl.formatMessage(i18n.failedToSaveThreshold, { error: errorMessage(error, 'Unknown error') }));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div
      className={cn('flex flex-col gap-2 px-3 py-3', alertStyles[alert.type], className)}
      onMouseDown={(e) => {
        // Prevent popover from closing when clicking inside the alert box
        if (isEditingThreshold) {
          e.stopPropagation();
        }
      }}
    >
      {alert.progress ? (
        <div className="flex flex-col gap-2">
          {/* Auto-compact threshold indicator with edit */}
          <div className="flex items-center justify-center gap-1 min-h-[20px]">
            {isEditingThreshold ? (
              <>
                <span className="text-[10px] opacity-70">{intl.formatMessage(i18n.autoCompactAt)}</span>
                <input
                  type="number"
                  min="1"
                  max="99"
                  step="1"
                  value={thresholdValue}
                  onChange={(e) => {
                    const val = parseInt(e.target.value, 10);
                    if (e.target.value === '') {
                      setThresholdValue(1);
                    } else if (!isNaN(val)) {
                      setThresholdValue(Math.max(1, Math.min(99, val)));
                    }
                  }}
                  onBlur={(e) => {
                    const val = parseInt(e.target.value, 10);
                    if (isNaN(val) || val < 1) {
                      setThresholdValue(1);
                    } else if (val > 99) {
                      setThresholdValue(99);
                    }
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      handleSaveThreshold();
                    } else if (e.key === 'Escape') {
                      setIsEditingThreshold(false);
                      const resetValue = Math.round(currentThreshold * 100);
                      setThresholdValue(Math.max(1, Math.min(99, resetValue)));
                    }
                  }}
                  onFocus={(e) => {
                    e.target.select();
                  }}
                  onClick={(e) => {
                    e.stopPropagation();
                  }}
                  className="w-12 px-1 text-[10px] bg-white/10 border border-current/30 rounded outline-none text-center focus:bg-white/20 focus:border-current/50 transition-colors"
                  disabled={isSaving}
                  autoFocus
                />
                <span className="text-[10px] opacity-70">%</span>
                <button
                  type="button"
                  onMouseDown={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    handleSaveThreshold();
                  }}
                  disabled={isSaving}
                  className="p-1 hover:opacity-60 transition-opacity cursor-pointer relative z-50"
                  style={{ minWidth: '20px', minHeight: '20px', pointerEvents: 'auto' }}
                >
                  <FaSave className="w-3 h-3" />
                </button>
              </>
            ) : (
              <>
                <span className="text-[10px] opacity-70">
                  {intl.formatMessage(i18n.autoCompactAt)} {Math.round(currentThreshold * 100)}%
                </span>
                <button
                  type="button"
                  onClick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setIsEditingThreshold(true);
                  }}
                  className="p-1 hover:opacity-60 transition-opacity cursor-pointer relative z-10"
                  style={{ minWidth: '20px', minHeight: '20px' }}
                >
                  <FaPencilAlt className="w-3 h-3 opacity-70" />
                </button>
              </>
            )}
          </div>
          {alert.showCompactButton && alert.onCompact && (
            <button
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                alert.onCompact!();
              }}
              disabled={alert.compactButtonDisabled}
              className={cn(
                'flex items-center justify-center gap-1.5 text-[11px] outline-none',
                alert.compactButtonDisabled
                  ? 'opacity-50 cursor-not-allowed'
                  : 'hover:opacity-80 cursor-pointer'
              )}
            >
              {alert.compactIcon}
              <span>{intl.formatMessage(i18n.compactNow)}</span>
            </button>
          )}
        </div>
      ) : (
        <>
          <div className="flex items-center gap-2">
            <div className="flex-shrink-0">{alertIcons[alert.type]}</div>
            <div className="flex flex-col gap-2 flex-1">
              <span className="text-[11px] break-words whitespace-pre-line">{alert.message}</span>
              {alert.action && (
                <a
                  role="button"
                  onClick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    alert.action?.onClick();
                  }}
                  className="text-[11px] text-left underline hover:opacity-80 cursor-pointer outline-none"
                >
                  {alert.action.text}
                </a>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
};

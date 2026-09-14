import { currentLocale } from '../i18n';

export function formatMessageTimestamp(timestamp?: number): string {
  const date = timestamp ? new Date(timestamp * 1000) : new Date();
  const now = new Date();

  // Format time using locale's default hour cycle
  const timeStr = date.toLocaleTimeString(currentLocale, {
    hour: 'numeric',
    minute: '2-digit',
  });

  // Check if the message is from today
  if (
    date.getDate() === now.getDate() &&
    date.getMonth() === now.getMonth() &&
    date.getFullYear() === now.getFullYear()
  ) {
    return timeStr;
  }

  // If not today, format as localized date + time
  const dateStr = date.toLocaleDateString(currentLocale, {
    month: '2-digit',
    day: '2-digit',
    year: 'numeric',
  });

  return `${dateStr} ${timeStr}`;
}

export interface ClockDisplay {
  time: string;
  meridiem: string;
  hour: number;
}

export function formatClockDisplay(
  date: Date = new Date(),
  locale: string = currentLocale
): ClockDisplay {
  const hour = date.getHours();

  try {
    const formatter = new Intl.DateTimeFormat(locale, {
      hour: 'numeric',
      minute: '2-digit',
    });

    const parts = formatter.formatToParts(date);
    const dayPeriodPart = parts.find((p) => p.type === 'dayPeriod');
    const meridiem = dayPeriodPart ? dayPeriodPart.value : '';

    const time = parts
      .filter((p) => p.type !== 'dayPeriod')
      .map((p) => p.value)
      .join('')
      .trim();

    return { time, meridiem, hour };
  } catch {
    const minutes = date.getMinutes();
    const meridiem = hour >= 12 ? 'PM' : 'AM';
    const displayHour = ((hour + 11) % 12) + 1;
    const time = `${displayHour}:${String(minutes).padStart(2, '0')}`;
    return { time, meridiem, hour };
  }
}


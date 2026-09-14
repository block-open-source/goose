import { cn } from '../../utils';
import { segmentInvisibleCharacters } from '../../utils/invisibleCharacters';

interface VisibleTextProps {
  text: string;
  className?: string;
}

// Renders text with every control or format character replaced by a visible
// Unicode code point marker so nothing in the string can hide or reorder the rest.
export function VisibleText({ text, className }: VisibleTextProps) {
  const segments = segmentInvisibleCharacters(text);
  return (
    <span className={cn('whitespace-pre-wrap break-all', className)} dir="ltr">
      {segments.map((segment, index) =>
        segment.kind === 'text' ? (
          <span key={index}>{segment.value}</span>
        ) : (
          <span
            key={index}
            data-testid="invisible-character"
            className="mx-0.5 rounded bg-yellow-200 px-1 text-[10px] font-semibold text-yellow-900 dark:bg-yellow-800/60 dark:text-yellow-100"
          >
            {segment.codePoint}
          </span>
        )
      )}
    </span>
  );
}

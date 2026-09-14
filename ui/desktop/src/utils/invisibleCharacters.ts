export type TextSegment =
  | { kind: 'text'; value: string }
  | { kind: 'invisible'; value: string; codePoint: string };

// Unicode control (Cc) and format (Cf) categories: C0/C1 controls, bidi overrides and
// isolates, zero-width characters, the tag block, soft hyphen, and similar characters
// that render as nothing or reorder surrounding text.
const INVISIBLE_CHARACTER = /[\p{Cc}\p{Cf}]/u;

export function formatCodePoint(character: string): string {
  const codePoint = character.codePointAt(0) ?? 0;
  return `U+${codePoint.toString(16).toUpperCase().padStart(4, '0')}`;
}

export function containsInvisibleCharacters(text: string): boolean {
  return INVISIBLE_CHARACTER.test(text);
}

export function segmentInvisibleCharacters(text: string): TextSegment[] {
  const segments: TextSegment[] = [];
  let visible = '';

  for (const character of text) {
    if (INVISIBLE_CHARACTER.test(character)) {
      if (visible) {
        segments.push({ kind: 'text', value: visible });
        visible = '';
      }
      segments.push({ kind: 'invisible', value: character, codePoint: formatCodePoint(character) });
    } else {
      visible += character;
    }
  }

  if (visible) {
    segments.push({ kind: 'text', value: visible });
  }
  return segments;
}

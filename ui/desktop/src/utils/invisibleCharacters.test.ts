import { describe, expect, it } from 'vitest';
import {
  containsInvisibleCharacters,
  formatCodePoint,
  segmentInvisibleCharacters,
} from './invisibleCharacters';

describe('invisibleCharacters', () => {
  it('leaves ordinary text as a single visible segment', () => {
    expect(segmentInvisibleCharacters('npx -y server')).toEqual([
      { kind: 'text', value: 'npx -y server' },
    ]);
    expect(containsInvisibleCharacters('npx -y server')).toBe(false);
  });

  it('splits out bidi overrides, zero-width and tag characters', () => {
    const text = 'safe\u202Eevil\u200B\u{E0041}';
    expect(segmentInvisibleCharacters(text)).toEqual([
      { kind: 'text', value: 'safe' },
      { kind: 'invisible', value: '\u202E', codePoint: 'U+202E' },
      { kind: 'text', value: 'evil' },
      { kind: 'invisible', value: '\u200B', codePoint: 'U+200B' },
      { kind: 'invisible', value: '\u{E0041}', codePoint: 'U+E0041' },
    ]);
    expect(containsInvisibleCharacters(text)).toBe(true);
  });

  it('treats newlines and other control characters as invisible', () => {
    expect(segmentInvisibleCharacters('echo safe\nrm -rf $HOME')).toEqual([
      { kind: 'text', value: 'echo safe' },
      { kind: 'invisible', value: '\n', codePoint: 'U+000A' },
      { kind: 'text', value: 'rm -rf $HOME' },
    ]);
  });

  it('does not flag spaces or non-ASCII letters', () => {
    expect(containsInvisibleCharacters('héllo wörld 日本')).toBe(false);
  });

  it('formats code points with at least four hex digits', () => {
    expect(formatCodePoint('\t')).toBe('U+0009');
    expect(formatCodePoint('\u180E')).toBe('U+180E');
  });
});

import { describe, expect, it } from 'vitest';
import { getTextDirection } from './textDirection';

describe('getTextDirection', () => {
  it('detects RTL for Arabic text with punctuation', () => {
    expect(getTextDirection('مرحبا، هذا اختبار.')).toBe('rtl');
  });

  it('detects RTL for Hebrew text', () => {
    expect(getTextDirection('שלום, עולם!')).toBe('rtl');
  });

  it('detects RTL for Persian text', () => {
    expect(getTextDirection('این یک پیام آزمایشی است.')).toBe('rtl');
  });

  it('detects RTL for Arabic Extended-B letters', () => {
    expect(getTextDirection(String.fromCodePoint(0x0870, 0x0871, 0x0872))).toBe('rtl');
  });

  it('detects LTR for English text', () => {
    expect(getTextDirection('Hello, world!')).toBe('ltr');
  });

  it('stays RTL when an English word leads the message', () => {
    expect(getTextDirection('Note: هذه رسالة تجريبية')).toBe('rtl');
  });

  it('stays RTL when a number leads the message', () => {
    expect(getTextDirection('123 سلام عليكم')).toBe('rtl');
  });

  it('stays RTL when an English word trails the message', () => {
    expect(getTextDirection('هذه رسالة تجريبية for testing')).toBe('rtl');
  });

  it('stays LTR when a single Arabic word sits in English text', () => {
    expect(getTextDirection('This is mostly English with سلام as one word')).toBe('ltr');
  });

  it('treats Arabic-Indic digits as neutral', () => {
    expect(getTextDirection('٣٤٥٦')).toBe(null);
    expect(getTextDirection('٣٤٥٦ مرحبا')).toBe('rtl');
  });

  it('treats Arabic separators as neutral', () => {
    expect(getTextDirection('٣٬٤٥٦')).toBe(null);
    expect(getTextDirection('٣٬٤٥٦ مرحبا')).toBe('rtl');
  });

  it('counts strong Arabic punctuation as RTL', () => {
    expect(getTextDirection('ما هذا؟')).toBe('rtl');
  });

  it('returns null for text without strong directional characters', () => {
    expect(getTextDirection('')).toBe(null);
    expect(getTextDirection('12345 !!! ...')).toBe(null);
    expect(getTextDirection('🎉 👍 100%')).toBe(null);
  });

  it('breaks ties towards LTR', () => {
    expect(getTextDirection('abcd שלום')).toBe('ltr');
  });

  it('does not count surrogate pairs as directional characters', () => {
    expect(getTextDirection('😀 שלום')).toBe('rtl');
  });
});

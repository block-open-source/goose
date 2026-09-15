import { describe, expect, it } from 'vitest';
import {
  acpHttpUrlFromHttpBase,
  httpBaseFromAcpWebSocketUrl,
  isLoopbackAcpWebSocketUrl,
  normalizeAcpHttpBaseUrl,
  statusHttpUrlFromHttpBase,
} from '../url';

describe('httpBaseFromAcpWebSocketUrl', () => {
  it('converts ws ACP URLs to HTTP bases', () => {
    expect(httpBaseFromAcpWebSocketUrl('ws://127.0.0.1:64027/acp?token=secret')).toBe(
      'http://127.0.0.1:64027'
    );
  });

  it('converts wss ACP URLs to HTTPS bases', () => {
    expect(httpBaseFromAcpWebSocketUrl('wss://example.com/acp?token=secret')).toBe(
      'https://example.com'
    );
  });

  it('preserves path prefixes before the ACP endpoint', () => {
    expect(httpBaseFromAcpWebSocketUrl('wss://example.com/goose/acp?token=secret')).toBe(
      'https://example.com/goose'
    );
  });

  it('rejects non-WebSocket URLs', () => {
    expect(() => httpBaseFromAcpWebSocketUrl('http://127.0.0.1:64027/acp')).toThrow(
      'ACP URL must use ws: or wss:'
    );
  });
});

describe('isLoopbackAcpWebSocketUrl', () => {
  it('accepts IPv4 loopback ACP URLs', () => {
    expect(isLoopbackAcpWebSocketUrl('ws://127.0.0.1:64027/acp?token=secret')).toBe(true);
    expect(isLoopbackAcpWebSocketUrl('wss://127.12.0.1:64027/acp?token=secret')).toBe(true);
  });

  it('accepts localhost ACP URLs', () => {
    expect(isLoopbackAcpWebSocketUrl('ws://localhost:64027/acp?token=secret')).toBe(true);
  });

  it('accepts IPv6 loopback ACP URLs', () => {
    expect(isLoopbackAcpWebSocketUrl('ws://[::1]:64027/acp?token=secret')).toBe(true);
  });

  it('rejects remote ACP URLs', () => {
    expect(isLoopbackAcpWebSocketUrl('wss://example.com/acp?token=secret')).toBe(false);
    expect(isLoopbackAcpWebSocketUrl('ws://192.168.1.10:3284/acp?token=secret')).toBe(false);
  });

  it('rejects DNS hostnames that start with 127', () => {
    expect(isLoopbackAcpWebSocketUrl('wss://127.evil.com/acp?token=secret')).toBe(false);
    expect(isLoopbackAcpWebSocketUrl('wss://127.0.0.1.example.com/acp?token=secret')).toBe(false);
  });

  it('rejects non-WebSocket URLs', () => {
    expect(() => isLoopbackAcpWebSocketUrl('http://127.0.0.1:64027/acp')).toThrow(
      'ACP URL must use ws: or wss:'
    );
  });
});

describe('normalizeAcpHttpBaseUrl', () => {
  it('normalizes root HTTPS base URLs', () => {
    expect(normalizeAcpHttpBaseUrl('https://example.com/')).toBe('https://example.com');
  });

  it('normalizes prefixed HTTPS base URLs', () => {
    expect(normalizeAcpHttpBaseUrl('https://example.com/goose/')).toBe('https://example.com/goose');
  });

  it('rejects WebSocket URLs', () => {
    expect(() => normalizeAcpHttpBaseUrl('wss://example.com/acp')).toThrow(
      'External ACP backend URL must use http: or https:'
    );
  });

  it('rejects direct ACP endpoint URLs', () => {
    expect(() => normalizeAcpHttpBaseUrl('https://example.com/acp')).toThrow(
      'External ACP backend URL must be the base URL before /acp'
    );
  });

  it('rejects query parameters and fragments', () => {
    expect(() => normalizeAcpHttpBaseUrl('https://example.com?token=secret')).toThrow(
      'External ACP backend URL must not include query parameters or fragments'
    );
    expect(() => normalizeAcpHttpBaseUrl('https://example.com#section')).toThrow(
      'External ACP backend URL must not include query parameters or fragments'
    );
  });
});

describe('HTTP endpoint URLs from ACP HTTP base URLs', () => {
  it('builds status URLs from root and prefixed bases', () => {
    expect(statusHttpUrlFromHttpBase('https://example.com/')).toBe('https://example.com/status');
    expect(statusHttpUrlFromHttpBase('https://example.com/goose/')).toBe(
      'https://example.com/goose/status'
    );
  });

  it('builds ACP URLs from root and prefixed bases', () => {
    expect(acpHttpUrlFromHttpBase('https://example.com/')).toBe('https://example.com/acp');
    expect(acpHttpUrlFromHttpBase('https://example.com/goose/')).toBe(
      'https://example.com/goose/acp'
    );
  });
});

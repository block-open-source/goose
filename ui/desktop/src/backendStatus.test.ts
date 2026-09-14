import { describe, expect, it, vi } from 'vitest';
import { checkBackendStatus } from './backendStatus';

type FetchInput = Parameters<typeof globalThis.fetch>[0];

const fetchInputUrl = (input: FetchInput): string => {
  if (typeof input === 'string') {
    return input;
  }
  if (input instanceof URL) {
    return input.toString();
  }
  return input.url;
};

describe('checkBackendStatus', () => {
  it('checks /status and validates the secret against /acp', async () => {
    const fetch = vi.fn(async (input: FetchInput) => {
      const url = fetchInputUrl(input);
      if (url === 'https://example.com/goose/status') {
        return new Response(null, { status: 200 });
      }
      if (url === 'https://example.com/goose/acp?token=test-secret') {
        return new Response(null, { status: 406 });
      }

      throw new Error(`Unexpected URL: ${url}`);
    });

    await expect(
      checkBackendStatus({
        baseUrl: 'https://example.com/goose',
        serverSecret: 'test-secret',
        fetch,
      })
    ).resolves.toMatchObject({ ok: true, failure: null });

    expect(fetch.mock.calls.map(([input]) => fetchInputUrl(input))).toEqual([
      'https://example.com/goose/status',
      'https://example.com/goose/acp?token=test-secret',
    ]);
  });

  it('reports the rejected secret without retrying', async () => {
    const fetch = vi.fn(async (input: FetchInput) => {
      const url = fetchInputUrl(input);
      if (url === 'https://example.com/status') {
        return new Response(null, { status: 200 });
      }
      if (url === 'https://example.com/acp?token=wrong-secret') {
        return new Response(null, { status: 401 });
      }

      throw new Error(`Unexpected URL: ${url}`);
    });

    const result = await checkBackendStatus({
      baseUrl: 'https://example.com',
      serverSecret: 'wrong-secret',
      fetch,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('Secret key: The backend rejected the secret key (HTTP 401)');
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it('reports an unusable URL without any request', async () => {
    const fetch = vi.fn();

    const result = await checkBackendStatus({
      baseUrl: 'https://example.com/acp',
      serverSecret: 'test-secret',
      fetch,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('URL:');
    expect(fetch).not.toHaveBeenCalled();
  });

  it('stops retrying a fatal network failure', async () => {
    const fetch = vi.fn(async () => {
      throw new Error('net::ERR_NAME_NOT_RESOLVED');
    });

    const result = await checkBackendStatus({
      baseUrl: 'https://nope.example.com',
      serverSecret: 'test-secret',
      fetch,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('ERR_NAME_NOT_RESOLVED');
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});

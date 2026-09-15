import { describe, expect, it, vi } from 'vitest';
import { connectRemoteBackend, type RemoteBackendParams } from './remoteBackends';

type HopRequest = NonNullable<RemoteBackendParams['request']>;

const respond = (status: number, location?: string) => ({
  status,
  statusText: '',
  header: () => null,
  location: location ?? null,
});

describe('connectRemoteBackend', () => {
  it('checks /status and validates the secret against /acp', async () => {
    const request = vi.fn(async (url: string) => {
      if (url === 'https://example.com/goose/status') {
        return respond(200);
      }
      if (url === 'https://example.com/goose/acp') {
        return respond(406);
      }

      throw new Error(`Unexpected URL: ${url}`);
    });

    const result = await connectRemoteBackend({
      baseUrl: 'https://example.com/goose',
      serverSecret: 'test-secret',
      request,
    });

    expect(result).toMatchObject({
      ok: true,
      failure: null,
      acpUrl: 'wss://example.com/goose/acp?token=test-secret',
    });
  });

  it('follows redirects and keeps the resolved ACP endpoint', async () => {
    const request = vi.fn(async (url: string) => {
      if (url === 'https://example.com/status') {
        return respond(302, 'https://backend.example.com/goose/status');
      }
      if (url === 'https://backend.example.com/goose/status') {
        return respond(200);
      }
      if (url === 'https://backend.example.com/goose/acp') {
        return respond(302, 'https://backend.example.com/socket?tenant=x');
      }
      if (url === 'https://backend.example.com/socket?tenant=x') {
        return respond(406);
      }

      throw new Error(`Unexpected URL: ${url}`);
    });

    const result = await connectRemoteBackend({
      baseUrl: 'https://example.com',
      serverSecret: 'test-secret',
      request,
    });

    expect(result.ok).toBe(true);
    expect(result.acpUrl).toBe('wss://backend.example.com/socket?tenant=x&token=test-secret');
  });

  it('sends the secret only to the resolved ACP endpoint', async () => {
    const request = vi.fn<HopRequest>(async (url) => {
      if (url === 'https://example.com/status') {
        return respond(200);
      }
      if (url === 'https://example.com/acp') {
        return respond(302, 'https://backend.example.com/acp');
      }
      if (url === 'https://backend.example.com/acp') {
        return respond(406);
      }

      throw new Error(`Unexpected URL: ${url}`);
    });

    await connectRemoteBackend({
      baseUrl: 'https://example.com',
      serverSecret: 'test-secret',
      request,
    });

    const secretRecipients = request.mock.calls
      .filter(([, init]) => init.headers?.['X-Secret-Key'])
      .map(([url]) => url);
    expect(secretRecipients).toEqual(['https://backend.example.com/acp']);
  });

  it('rejects an HTTPS to HTTP redirect', async () => {
    const request = vi.fn(async () => respond(302, 'http://backend.example.com/status'));

    const result = await connectRemoteBackend({
      baseUrl: 'https://example.com',
      serverSecret: 'test-secret',
      request,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('Redirect from HTTPS to HTTP is not allowed');
    expect(request).toHaveBeenCalledTimes(1);
  });

  it('rejects a cross-host redirect when a certificate fingerprint is pinned', async () => {
    const request = vi.fn(async () => respond(302, 'https://backend.example.com/status'));

    const result = await connectRemoteBackend({
      baseUrl: 'https://example.com',
      serverSecret: 'test-secret',
      request,
      pinnedHostname: 'example.com',
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('certificate fingerprint is configured');
  });

  it('reports the rejected secret without retrying', async () => {
    const request = vi.fn(async (url: string) => {
      if (url === 'https://example.com/status') {
        return respond(200);
      }
      if (url === 'https://example.com/acp') {
        return respond(401);
      }

      throw new Error(`Unexpected URL: ${url}`);
    });

    const result = await connectRemoteBackend({
      baseUrl: 'https://example.com',
      serverSecret: 'wrong-secret',
      request,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('Secret key: The backend rejected the secret key (HTTP 401)');
    expect(request).toHaveBeenCalledTimes(3);
  });

  it('reports an unusable URL without any request', async () => {
    const request = vi.fn();

    const result = await connectRemoteBackend({
      baseUrl: 'https://example.com/acp',
      serverSecret: 'test-secret',
      request,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('URL:');
    expect(request).not.toHaveBeenCalled();
  });

  it('stops retrying a fatal network failure', async () => {
    const request = vi.fn(async () => {
      throw new Error('net::ERR_NAME_NOT_RESOLVED');
    });

    const result = await connectRemoteBackend({
      baseUrl: 'https://nope.example.com',
      serverSecret: 'test-secret',
      request,
    });

    expect(result.ok).toBe(false);
    expect(result.failure).toContain('ERR_NAME_NOT_RESOLVED');
    expect(request).toHaveBeenCalledTimes(1);
  });
});

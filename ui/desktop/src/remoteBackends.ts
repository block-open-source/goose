import { net } from 'electron';
import { probeRequest } from './backendProbe';
import {
  acpHttpUrlFromHttpBase,
  normalizeAcpHttpBaseUrl,
  statusHttpUrlFromHttpBase,
} from './acp/url';

const RETRY_BUDGET_MS = 15000;
const RETRY_INTERVAL_MS = 250;
const PROBE_TIMEOUT_MS = 5000;
const MAX_REDIRECT_HOPS = 20;

const FATAL_ERROR_PATTERN = /panicked at|RUST_BACKTRACE|fatal error/i;
const FATAL_NETWORK_PATTERN = /NAME_NOT_RESOLVED|CERT|SSL|CLIENT_AUTH|INVALID_URL|UNSAFE_PORT/;

export interface RemoteBackendStep {
  name: string;
  ok: boolean;
  detail: string;
}

export interface RemoteBackendConnection {
  ok: boolean;
  steps: RemoteBackendStep[];
  failure: string | null;
  acpUrl: string | null;
}

export interface RemoteBackendParams {
  baseUrl: string;
  serverSecret: string;
  pinnedHostname?: string | null;
  errorLog?: string[];
  request?: HopRequest;
}

interface HopInit {
  headers?: Record<string, string>;
  signal?: AbortSignal;
}

interface Hop {
  status: number;
  statusText: string;
  header(name: string): string | null;
  location: string | null;
}

type HopRequest = (url: string, init: HopInit) => Promise<Hop>;

const headerValue = (headers: Record<string, string | string[]>, name: string): string | null => {
  const value = headers[name.toLowerCase()];
  if (value === undefined) {
    return null;
  }
  return Array.isArray(value) ? (value[0] ?? null) : value;
};

// net.fetch reports an incorrect Response.url, so each hop is issued separately
// with net.request and this module decides whether to follow it.
const netHopRequest: HopRequest = (url, init) =>
  new Promise<Hop>((resolve, reject) => {
    const request = net.request({ method: 'GET', url, redirect: 'manual', credentials: 'omit' });
    let settled = false;

    const abort = () => request.abort();
    init.signal?.addEventListener('abort', abort, { once: true });

    const settle = (finish: () => void) => {
      if (settled) {
        return;
      }
      settled = true;
      init.signal?.removeEventListener('abort', abort);
      finish();
    };

    // A manual redirect is not followed, so the request is cancelled after this
    // event and only the 'abort' event follows.
    request.on('redirect', (statusCode, _method, redirectUrl, responseHeaders) => {
      settle(() =>
        resolve({
          status: statusCode,
          statusText: '',
          header: (name) => headerValue(responseHeaders, name),
          location: redirectUrl,
        })
      );
      request.abort();
    });

    request.on('response', (response) => {
      response.on('data', () => undefined);
      response.on('end', () =>
        settle(() =>
          resolve({
            status: response.statusCode,
            statusText: response.statusMessage,
            header: (name) => headerValue(response.headers, name),
            location: null,
          })
        )
      );
      response.on('error', (error: Error) => settle(() => reject(error)));
    });

    request.on('error', (error) => settle(() => reject(error)));
    request.on('abort', () => settle(() => reject(new Error('Request aborted'))));

    for (const [name, value] of Object.entries(init.headers ?? {})) {
      request.setHeader(name, value);
    }
    request.end();
  });

class RedirectError extends Error {}

const CLIENT_AUTH_PATTERN = /ERR_SSL_CLIENT_AUTH_CERT_NEEDED|ERR_BAD_SSL_CLIENT_AUTH_CERT/;

// A server asking for a client certificate cannot be reached from net.request,
// because Chromium only runs certificate selection for WebContents-originated
// requests. That path follows redirects itself, so it is used only after the
// hop-by-hop transport reports that a certificate is required.
const mtlsHopRequest: HopRequest = async (url, init) => {
  const result = await probeRequest(url, init.headers ?? {});
  // This path follows redirects itself, so a hop cannot be validated. Rather
  // than trust an unvalidated destination, a redirected mTLS backend fails.
  if (result.url && result.url !== url) {
    throw new RedirectError(
      `Redirect to ${result.url} cannot be validated on an mTLS backend. Configure the final backend URL instead.`
    );
  }

  const headers = new Map(result.headers.map(([name, value]) => [name.toLowerCase(), value]));
  return {
    status: result.status,
    statusText: result.statusText,
    header: (name) => headers.get(name.toLowerCase()) ?? null,
    location: null,
  };
};

const isRedirect = (status: number): boolean =>
  status === 301 || status === 302 || status === 303 || status === 307 || status === 308;

const checkHop = (from: URL, to: URL, pinnedHostname: string | null): void => {
  if (to.protocol !== 'http:' && to.protocol !== 'https:') {
    throw new RedirectError(`Redirect to ${to.protocol} is not allowed, only http: and https:.`);
  }
  if (from.protocol === 'https:' && to.protocol === 'http:') {
    throw new RedirectError(
      `Redirect from HTTPS to HTTP is not allowed (${from.origin} to ${to.origin}).`
    );
  }
  if (pinnedHostname && to.hostname.toLowerCase() !== pinnedHostname) {
    throw new RedirectError(
      `Redirect to ${to.hostname} is not allowed because a certificate fingerprint is configured for ${pinnedHostname}.`
    );
  }
};

const resolveRedirects = async (
  request: HopRequest,
  startUrl: string,
  pinnedHostname: string | null,
  init: HopInit
): Promise<{ url: string; hop: Hop }> => {
  let url = new URL(startUrl);

  for (let hopCount = 0; ; hopCount += 1) {
    const hop = await request(url.toString(), init);
    if (!isRedirect(hop.status)) {
      return { url: url.toString(), hop };
    }

    if (hopCount >= MAX_REDIRECT_HOPS) {
      throw new RedirectError(`Exceeded ${MAX_REDIRECT_HOPS} redirects starting at ${startUrl}.`);
    }
    if (!hop.location) {
      throw new RedirectError(`Redirect from ${url.toString()} has no Location header.`);
    }

    const next = new URL(hop.location, url);
    checkHop(url, next, pinnedHostname);
    url = next;
  }
};

interface Probe {
  ok: boolean;
  detail: string;
  retryable: boolean;
  resolvedUrl?: string;
}

const delay = (timeoutMs: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, timeoutMs));

const errorText = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

const proxyNote = (hop: Hop): string => {
  const doormanError = hop.header('x-sq-cf-doorman-error');
  return doormanError && doormanError !== 'none'
    ? ` A proxy in front of the backend reported "${doormanError}".`
    : '';
};

// Redirects are resolved anonymously; only the resolved URL is re-requested
// with the secret, so no intermediate origin ever sees it.
const probe = async (
  request: HopRequest,
  url: string,
  pinnedHostname: string | null,
  credentials: Record<string, string> | null,
  expect: (hop: Hop, resolvedUrl: string) => Probe
): Promise<Probe> => {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), PROBE_TIMEOUT_MS);
  const attempt = async (hopRequest: HopRequest): Promise<Probe> => {
    const { url: resolvedUrl, hop } = await resolveRedirects(hopRequest, url, pinnedHostname, {
      signal: controller.signal,
    });
    const finalHop = credentials
      ? await hopRequest(resolvedUrl, { headers: credentials, signal: controller.signal })
      : hop;
    return expect(finalHop, resolvedUrl);
  };

  try {
    try {
      return await attempt(request);
    } catch (error) {
      if (request !== netHopRequest || !CLIENT_AUTH_PATTERN.test(errorText(error))) {
        throw error;
      }
      return await attempt(mtlsHopRequest);
    }
  } catch (error) {
    const detail = errorText(error);
    return {
      ok: false,
      detail,
      retryable: !(error instanceof RedirectError) && !FATAL_NETWORK_PATTERN.test(detail),
    };
  } finally {
    clearTimeout(timeout);
  }
};

const probeStatus = (
  request: HopRequest,
  baseUrl: string,
  pinnedHostname: string | null
): Promise<Probe> =>
  probe(request, statusHttpUrlFromHttpBase(baseUrl), pinnedHostname, null, (hop, resolvedUrl) =>
    hop.status >= 200 && hop.status < 300
      ? {
          ok: true,
          detail: `GET /status returned ${hop.status}.`,
          retryable: false,
          resolvedUrl,
        }
      : {
          ok: false,
          detail: `GET /status returned ${hop.status} ${hop.statusText}.${proxyNote(hop)}`,
          retryable: hop.status >= 500,
        }
  );

const probeAcp = (
  request: HopRequest,
  baseUrl: string,
  secret: string,
  pinnedHostname: string | null
): Promise<Probe> =>
  probe(
    request,
    acpHttpUrlFromHttpBase(baseUrl),
    pinnedHostname,
    { 'X-Secret-Key': secret },
    (hop, resolvedUrl) => {
      if (hop.status === 406) {
        return {
          ok: true,
          detail: 'The backend accepted the secret key.',
          retryable: false,
          resolvedUrl,
        };
      }
      if (hop.status === 401 || hop.status === 403) {
        return {
          ok: false,
          detail: `The backend rejected the secret key (HTTP ${hop.status}). It must match GOOSE_SERVER__SECRET_KEY on the backend.${proxyNote(hop)}`,
          retryable: false,
        };
      }
      return {
        ok: false,
        detail: `GET /acp returned ${hop.status} ${hop.statusText}, expected 406.${proxyNote(hop)}`,
        retryable: hop.status >= 500,
      };
    }
  );

const baseUrlFromStatusUrl = (statusUrl: string): string | null => {
  const url = new URL(statusUrl);
  const pathname = url.pathname.replace(/\/+$/, '');
  return pathname.endsWith('/status')
    ? `${url.origin}${pathname.slice(0, -'/status'.length)}`
    : null;
};

// The resolved endpoint is kept whole, including any query parameters a proxy
// added, so the socket connects exactly where the probe succeeded.
const acpWebSocketUrl = (acpUrl: string, secret: string): string => {
  const url = new URL(acpUrl);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.hash = '';
  url.searchParams.set('token', secret);
  return url.toString();
};

const isFatalError = (line: string): boolean => FATAL_ERROR_PATTERN.test(line);

export const connectRemoteBackend = async ({
  baseUrl,
  serverSecret,
  pinnedHostname = null,
  errorLog = [],
  request = netHopRequest,
}: RemoteBackendParams): Promise<RemoteBackendConnection> => {
  const steps: RemoteBackendStep[] = [];
  const pin = pinnedHostname?.toLowerCase() ?? null;

  const run = async (name: string, attempt: () => Promise<Probe>): Promise<Probe> => {
    const deadline = Date.now() + RETRY_BUDGET_MS;
    let result = await attempt();
    while (
      !result.ok &&
      result.retryable &&
      Date.now() < deadline &&
      !errorLog.some(isFatalError)
    ) {
      await delay(RETRY_INTERVAL_MS);
      result = await attempt();
    }
    steps.push({ name, ok: result.ok, detail: result.detail });
    return result;
  };

  let normalizedBaseUrl = '';
  try {
    normalizedBaseUrl = normalizeAcpHttpBaseUrl(baseUrl);
    steps.push({ name: 'URL', ok: true, detail: normalizedBaseUrl });
  } catch (error) {
    steps.push({ name: 'URL', ok: false, detail: errorText(error) });
  }

  const resolve = async (): Promise<string | null> => {
    if (!normalizedBaseUrl) {
      return null;
    }

    const reachable = await run('Reachable', () => probeStatus(request, normalizedBaseUrl, pin));
    if (!reachable.ok || !reachable.resolvedUrl) {
      return null;
    }

    const resolvedBaseUrl = baseUrlFromStatusUrl(reachable.resolvedUrl);
    if (!resolvedBaseUrl) {
      steps.push({
        name: 'Redirect',
        ok: false,
        detail: `/status resolved to ${reachable.resolvedUrl}, so the ACP path cannot be derived from it.`,
      });
      return null;
    }
    if (resolvedBaseUrl !== normalizedBaseUrl) {
      steps.push({ name: 'Redirect', ok: true, detail: `Followed to ${resolvedBaseUrl}.` });
    }

    const accepted = await run('Secret key', () =>
      probeAcp(request, resolvedBaseUrl, serverSecret, pin)
    );
    if (!accepted.ok || !accepted.resolvedUrl) {
      return null;
    }

    const statusOrigin = new URL(resolvedBaseUrl).origin;
    const acpOrigin = new URL(accepted.resolvedUrl).origin;
    if (statusOrigin !== acpOrigin) {
      steps.push({
        name: 'Redirect',
        ok: false,
        detail: `/status and /acp resolved to different origins (${statusOrigin} and ${acpOrigin}).`,
      });
      return null;
    }

    return acpWebSocketUrl(accepted.resolvedUrl, serverSecret);
  };

  const acpUrl = await resolve();

  const failed = steps.find((step) => !step.ok);
  return {
    ok: !failed,
    steps,
    failure: failed ? `${failed.name}: ${failed.detail}`.trim() : null,
    acpUrl: failed ? null : acpUrl,
  };
};

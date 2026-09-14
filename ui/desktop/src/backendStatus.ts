import {
  acpHttpUrlFromHttpBase,
  normalizeAcpHttpBaseUrl,
  statusHttpUrlFromHttpBase,
} from './acp/url';

const RETRY_BUDGET_MS = 15000;
const RETRY_INTERVAL_MS = 250;
const PROBE_TIMEOUT_MS = 5000;

const FATAL_ERROR_PATTERN = /panicked at|RUST_BACKTRACE|fatal error/i;
const FATAL_NETWORK_PATTERN = /NAME_NOT_RESOLVED|CERT|SSL|CLIENT_AUTH|INVALID_URL|UNSAFE_PORT/;

export interface BackendCheckStep {
  name: string;
  ok: boolean;
  detail: string;
}

export interface BackendCheckResult {
  ok: boolean;
  steps: BackendCheckStep[];
  failure: string | null;
}

export interface BackendCheckParams {
  baseUrl: string;
  serverSecret: string;
  fetch: typeof globalThis.fetch;
  errorLog?: string[];
}

interface Probe {
  ok: boolean;
  detail: string;
  retryable: boolean;
}

const delay = (timeoutMs: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, timeoutMs));

const errorText = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

const proxyNote = (response: Response): string => {
  const doormanError = response.headers.get('x-sq-cf-doorman-error');
  return doormanError && doormanError !== 'none'
    ? ` A proxy in front of the backend reported "${doormanError}".`
    : '';
};

const request = async (
  fetch: typeof globalThis.fetch,
  url: string,
  init: Parameters<typeof globalThis.fetch>[1],
  expect: (response: Response) => Probe
): Promise<Probe> => {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), PROBE_TIMEOUT_MS);
  try {
    return expect(await fetch(url, { ...init, signal: controller.signal }));
  } catch (error) {
    const detail = errorText(error);
    return { ok: false, detail, retryable: !FATAL_NETWORK_PATTERN.test(detail) };
  } finally {
    clearTimeout(timeout);
  }
};

const probeStatus = (
  fetch: typeof globalThis.fetch,
  baseUrl: string,
  secret: string
): Promise<Probe> =>
  request(
    fetch,
    statusHttpUrlFromHttpBase(baseUrl),
    { headers: { 'X-Secret-Key': secret } },
    (response) =>
      response.ok
        ? { ok: true, detail: `GET /status returned ${response.status}.`, retryable: false }
        : {
            ok: false,
            detail: `GET /status returned ${response.status} ${response.statusText}.${proxyNote(response)}`,
            retryable: response.status >= 500,
          }
  );

const probeSecret = (
  fetch: typeof globalThis.fetch,
  baseUrl: string,
  secret: string
): Promise<Probe> =>
  request(fetch, acpHttpUrlFromHttpBase(baseUrl, secret), {}, (response) => {
    if (response.status === 406) {
      return { ok: true, detail: 'The backend accepted the secret key.', retryable: false };
    }
    if (response.status === 401 || response.status === 403) {
      return {
        ok: false,
        detail: `The backend rejected the secret key (HTTP ${response.status}). It must match GOOSE_SERVER__SECRET_KEY on the backend.${proxyNote(response)}`,
        retryable: false,
      };
    }
    return {
      ok: false,
      detail: `GET /acp returned ${response.status} ${response.statusText}, expected 406.${proxyNote(response)}`,
      retryable: response.status >= 500,
    };
  });

export const isFatalError = (line: string): boolean => FATAL_ERROR_PATTERN.test(line);

export const checkBackendStatus = async ({
  baseUrl,
  serverSecret,
  fetch,
  errorLog = [],
}: BackendCheckParams): Promise<BackendCheckResult> => {
  const steps: BackendCheckStep[] = [];

  const run = async (name: string, probe: () => Promise<Probe>): Promise<boolean> => {
    const deadline = Date.now() + RETRY_BUDGET_MS;
    let result = await probe();
    while (
      !result.ok &&
      result.retryable &&
      Date.now() < deadline &&
      !errorLog.some(isFatalError)
    ) {
      await delay(RETRY_INTERVAL_MS);
      result = await probe();
    }
    steps.push({ name, ok: result.ok, detail: result.detail });
    return result.ok;
  };

  let normalizedBaseUrl = '';
  try {
    normalizedBaseUrl = normalizeAcpHttpBaseUrl(baseUrl);
    steps.push({ name: 'URL', ok: true, detail: normalizedBaseUrl });
  } catch (error) {
    steps.push({ name: 'URL', ok: false, detail: errorText(error) });
  }

  if (normalizedBaseUrl) {
    const reachable = await run('Reachable', () =>
      probeStatus(fetch, normalizedBaseUrl, serverSecret)
    );
    if (reachable) {
      await run('Secret key', () => probeSecret(fetch, normalizedBaseUrl, serverSecret));
    }
  }

  const failed = steps.find((step) => !step.ok);
  return {
    ok: !failed,
    steps,
    failure: failed ? `${failed.name}: ${failed.detail}`.trim() : null,
  };
};

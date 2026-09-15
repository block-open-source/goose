export function httpBaseFromAcpWebSocketUrl(acpUrl: string): string {
  const url = new URL(acpUrl);

  if (url.protocol === 'ws:') {
    url.protocol = 'http:';
  } else if (url.protocol === 'wss:') {
    url.protocol = 'https:';
  } else {
    throw new Error(`ACP URL must use ws: or wss:, got ${url.protocol}`);
  }

  const pathname = url.pathname.replace(/\/+$/, '');
  const pathPrefix = pathname.endsWith('/acp') ? pathname.slice(0, -'/acp'.length) : pathname;

  return `${url.origin}${pathPrefix}`;
}

export function isLoopbackAcpWebSocketUrl(acpUrl: string): boolean {
  const url = new URL(acpUrl);

  if (url.protocol !== 'ws:' && url.protocol !== 'wss:') {
    throw new Error(`ACP URL must use ws: or wss:, got ${url.protocol}`);
  }

  const hostname = url.hostname.toLowerCase().replace(/^\[(.*)\]$/, '$1');
  return hostname === 'localhost' || hostname === '::1' || isIpv4LoopbackLiteral(hostname);
}

function isIpv4LoopbackLiteral(hostname: string): boolean {
  const octets = hostname.split('.');
  if (octets.length !== 4 || octets.some((octet) => !/^\d+$/.test(octet))) {
    return false;
  }

  return octets.every((octet) => Number(octet) <= 255) && Number(octets[0]) === 127;
}

export function normalizeAcpHttpBaseUrl(rawBaseUrl: string): string {
  const trimmed = rawBaseUrl.trim();
  if (!trimmed) {
    throw new Error('External ACP backend URL is required');
  }

  const url = new URL(trimmed);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`External ACP backend URL must use http: or https:, got ${url.protocol}`);
  }

  if (url.search || url.hash) {
    throw new Error('External ACP backend URL must not include query parameters or fragments');
  }

  const pathname = url.pathname.replace(/\/+$/, '');
  if (pathname.endsWith('/acp')) {
    throw new Error('External ACP backend URL must be the base URL before /acp');
  }

  return `${url.origin}${pathname}`;
}

function httpEndpointUrlFromHttpBase(rawBaseUrl: string, endpoint: 'status' | 'acp'): string {
  const baseUrl = normalizeAcpHttpBaseUrl(rawBaseUrl);
  const url = new URL(baseUrl);
  url.pathname = `${url.pathname.replace(/\/+$/, '')}/${endpoint}`;
  return url.toString();
}

export function statusHttpUrlFromHttpBase(rawBaseUrl: string): string {
  return httpEndpointUrlFromHttpBase(rawBaseUrl, 'status');
}

export function acpHttpUrlFromHttpBase(rawBaseUrl: string): string {
  return httpEndpointUrlFromHttpBase(rawBaseUrl, 'acp');
}

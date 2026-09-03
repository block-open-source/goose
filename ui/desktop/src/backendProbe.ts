import { WebContentsView } from 'electron';

// Chromium only runs client certificate selection for WebContents-originated
// requests, so mTLS backends are unreachable from main-process net.request.
// These probes go through an offscreen view on the renderer partition to share
// the network path the ACP WebSocket uses. A WebContentsView is not a window, so
// it does not affect the window-all-closed lifecycle.
const PROBE_PARTITION = 'persist:goose';

type ProbeResult =
  | { ok: true; status: number; statusText: string; url: string; headers: [string, string][] }
  | { ok: false; message: string };

export interface ProbeRequestResult {
  status: number;
  statusText: string;
  url: string;
  headers: [string, string][];
}

let probeView: WebContentsView | null = null;

const getProbeView = async (origin: string): Promise<WebContentsView> => {
  if (probeView && !probeView.webContents.isDestroyed()) {
    return probeView;
  }

  probeView = new WebContentsView({
    webPreferences: { partition: PROBE_PARTITION, nodeIntegration: false, contextIsolation: true },
  });
  // The document must share the backend's origin, otherwise its fetches are
  // cross-origin and blocked before a client certificate is ever requested.
  await probeView.webContents.loadURL(origin).catch(() => undefined);
  return probeView;
};

// Redirects are followed by the renderer because a manual redirect yields an
// opaque response with no Location header, so per-hop inspection is not possible
// on this path. Only the final response is reported.
export const probeRequest = async (
  url: string,
  headers: Record<string, string>
): Promise<ProbeRequestResult> => {
  const view = await getProbeView(new URL(url).origin);
  const request = JSON.stringify({ url, headers });

  const result: ProbeResult = await (view.webContents.executeJavaScript(`
    (async () => {
      const request = ${request};
      try {
        const response = await fetch(request.url, { headers: request.headers });
        return {
          ok: true,
          status: response.status,
          statusText: response.statusText,
          url: response.url,
          headers: [...response.headers.entries()],
        };
      } catch (error) {
        return { ok: false, message: String((error && error.message) || error) };
      }
    })()
  `) as Promise<ProbeResult>);

  if (result.ok !== true) {
    throw new Error(result.message);
  }

  return {
    status: result.status,
    statusText: result.statusText,
    url: result.url,
    headers: result.headers,
  };
};

export const closeBackendProbe = (): void => {
  probeView?.webContents.close();
  probeView = null;
};

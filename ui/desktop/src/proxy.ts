import type { Session } from 'electron';

type ProxySession = Pick<Session, 'setProxy'>;
type ProxyEnvironment = Record<string, string | undefined>;

export async function configureProxy(
  defaultSession: ProxySession,
  rendererSession: ProxySession,
  environment: ProxyEnvironment = process.env
): Promise<void> {
  const httpsProxy = environment.HTTPS_PROXY || environment.https_proxy;
  const httpProxy = environment.HTTP_PROXY || environment.http_proxy;
  const proxyUrl = httpsProxy || httpProxy;

  if (!proxyUrl) {
    return;
  }

  console.log('[Main] Configuring proxy');
  const proxyConfig = {
    proxyRules: proxyUrl,
    proxyBypassRules: environment.NO_PROXY || environment.no_proxy || '',
  };

  await Promise.all([defaultSession.setProxy(proxyConfig), rendererSession.setProxy(proxyConfig)]);
  console.log('[Main] Proxy configured successfully');
}

import type { Session } from 'electron';
import { describe, expect, it, vi } from 'vitest';
import { configureProxy } from './proxy';

function createMockSession() {
  const setProxy = vi.fn<Session['setProxy']>().mockResolvedValue(undefined);
  return {
    session: { setProxy } as Pick<Session, 'setProxy'>,
    setProxy,
  };
}

describe('proxy configuration', () => {
  it('applies the same proxy configuration to both Electron sessions', async () => {
    const defaultSession = createMockSession();
    const rendererSession = createMockSession();

    await configureProxy(defaultSession.session, rendererSession.session, {
      HTTPS_PROXY: 'https://proxy.example:8443',
      NO_PROXY: 'localhost,127.0.0.1',
    });

    expect(defaultSession.setProxy).toHaveBeenCalledOnce();
    expect(rendererSession.setProxy).toHaveBeenCalledOnce();
    expect(defaultSession.setProxy).toHaveBeenCalledWith({
      proxyRules: 'https://proxy.example:8443',
      proxyBypassRules: 'localhost,127.0.0.1',
    });
    expect(rendererSession.setProxy.mock.calls[0][0]).toBe(
      defaultSession.setProxy.mock.calls[0][0]
    );
  });

  it('falls back to HTTP_PROXY without changing the default bypass rules', async () => {
    const defaultSession = createMockSession();
    const rendererSession = createMockSession();

    await configureProxy(defaultSession.session, rendererSession.session, {
      HTTP_PROXY: 'http://proxy.example:8080',
    });

    expect(defaultSession.setProxy).toHaveBeenCalledWith({
      proxyRules: 'http://proxy.example:8080',
      proxyBypassRules: '',
    });
    expect(rendererSession.setProxy).toHaveBeenCalledWith({
      proxyRules: 'http://proxy.example:8080',
      proxyBypassRules: '',
    });
  });

  it('leaves both sessions unchanged when no proxy is configured', async () => {
    const defaultSession = createMockSession();
    const rendererSession = createMockSession();

    await configureProxy(defaultSession.session, rendererSession.session, {});

    expect(defaultSession.setProxy).not.toHaveBeenCalled();
    expect(rendererSession.setProxy).not.toHaveBeenCalled();
  });
});

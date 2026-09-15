import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getEffectiveWorkingDir, resolveWorkingDir } from '../workingDir';

describe('resolveWorkingDir', () => {
  it('uses the configured external backend directory when present', () => {
    expect(resolveWorkingDir(' /home/goose ', 'C:\\Users\\goose', 'C:\\Users\\goose')).toBe(
      '/home/goose'
    );
    expect(resolveWorkingDir(' ', 'C:\\work', 'C:\\Users\\goose')).toBe('C:\\work');
    expect(resolveWorkingDir(undefined, undefined, 'C:\\Users\\goose')).toBe('C:\\Users\\goose');
  });
});

describe('getEffectiveWorkingDir', () => {
  const getSettingMock = vi.fn();
  const getSecretKeyMock = vi.fn();
  const appConfigGetMock = vi.fn();

  const mockWindow = (externalBackend: boolean, boundUrl: string, source = 'settings') => {
    appConfigGetMock.mockImplementation((key: string) => {
      if (key === 'GOOSE_EXTERNAL_BACKEND') return externalBackend;
      if (key === 'GOOSE_EXTERNAL_BACKEND_URL') return boundUrl;
      if (key === 'GOOSE_EXTERNAL_BACKEND_SOURCE') return source;
      if (key === 'GOOSE_WORKING_DIR') return '/Users/johannes/home/workspace';
      return undefined;
    });
  };

  beforeEach(() => {
    getSettingMock.mockReset();
    getSecretKeyMock.mockReset().mockResolvedValue('window-bound-secret');
    appConfigGetMock.mockReset();
    (globalThis as Record<string, unknown>).window = {
      appConfig: { get: appConfigGetMock },
      electron: { getSetting: getSettingMock, getSecretKey: getSecretKeyMock },
    } as unknown as typeof globalThis;
  });

  it('prefers the configured remote directory when bound to the matching external backend', async () => {
    mockWindow(true, 'http://remote:3000/');
    getSettingMock.mockResolvedValue({
      enabled: true,
      url: 'http://remote:3000',
      secret: 'window-bound-secret',
      workingDir: ' /home/goose/workspace ',
    });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/home/goose/workspace');
  });

  it('rejects a directory configured for a different backend credential', async () => {
    mockWindow(true, 'https://shared.example');
    getSettingMock.mockResolvedValue({
      enabled: true,
      url: 'https://shared.example',
      secret: 'new-backend-secret',
      workingDir: '/tenant-b/private-project',
    });

    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('refreshes same-credential directories but rejects settings after credential rotation', async () => {
    mockWindow(true, 'https://shared.example');
    getSettingMock
      .mockResolvedValueOnce({
        enabled: true,
        url: 'https://shared.example',
        secret: 'window-bound-secret',
        workingDir: '/tenant-a/first-project',
      })
      .mockResolvedValueOnce({
        enabled: true,
        url: 'https://shared.example',
        secret: 'window-bound-secret',
        workingDir: ' /tenant-a/second-project ',
      })
      .mockResolvedValueOnce({
        enabled: true,
        url: 'https://shared.example',
        secret: 'new-backend-secret',
        workingDir: '/tenant-b/private-project',
      });

    await expect(getEffectiveWorkingDir()).resolves.toBe('/tenant-a/first-project');
    await expect(getEffectiveWorkingDir()).resolves.toBe('/tenant-a/second-project');
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it.each([undefined, '', ' window-bound-secret '])(
    'rejects a missing, empty, or non-identical configured secret (%s)',
    async (secret) => {
      mockWindow(true, 'https://shared.example');
      getSettingMock.mockResolvedValue({
        enabled: true,
        url: 'https://shared.example',
        secret,
        workingDir: '/remote/project',
      });

      await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
    }
  );

  it('falls back to the remembered directory when the bound lease is unavailable', async () => {
    mockWindow(true, 'https://shared.example');
    getSecretKeyMock.mockResolvedValue(null);
    getSettingMock.mockResolvedValue({
      enabled: true,
      url: 'https://shared.example',
      secret: 'window-bound-secret',
      workingDir: '/remote/project',
    });

    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('falls back to the remembered directory when the bound secret cannot be read', async () => {
    mockWindow(true, 'https://shared.example');
    getSecretKeyMock.mockRejectedValue(new Error('lease unavailable'));
    getSettingMock.mockResolvedValue({
      enabled: true,
      url: 'https://shared.example',
      secret: 'window-bound-secret',
      workingDir: '/remote/project',
    });

    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('honors the configured remote directory for env-mode backends regardless of enabled/url', async () => {
    mockWindow(true, 'http://env-backend:3000', 'env');
    getSettingMock.mockResolvedValue({
      enabled: false,
      url: 'http://unrelated:4000',
      secret: 'different-settings-secret',
      workingDir: '/home/goose/workspace',
    });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/home/goose/workspace');
    expect(getSecretKeyMock).not.toHaveBeenCalled();
  });

  it('falls back to the remembered directory for env-mode backends without a configured dir', async () => {
    mockWindow(true, 'http://env-backend:3000', 'env');
    getSettingMock.mockResolvedValue({ enabled: false, workingDir: '   ' });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
    expect(getSecretKeyMock).not.toHaveBeenCalled();
  });

  it('ignores the remote directory when the window is bound to the local backend', async () => {
    mockWindow(false, '');
    getSettingMock.mockResolvedValue({ enabled: true, workingDir: '/home/goose/workspace' });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('falls back to the remembered directory when the bound backend no longer matches settings', async () => {
    mockWindow(true, 'http://server-a:3000');
    getSettingMock.mockResolvedValue({
      enabled: true,
      url: 'http://server-b:3000',
      workingDir: '/home/goose/workspace',
    });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('falls back to the remembered directory when the external backend is disabled', async () => {
    mockWindow(true, 'http://remote:3000');
    getSettingMock.mockResolvedValue({ enabled: false, workingDir: '/home/goose/workspace' });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('falls back to the remembered directory when the remote directory is blank', async () => {
    mockWindow(true, 'http://remote:3000');
    getSettingMock.mockResolvedValue({ enabled: true, workingDir: '   ' });
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });

  it('falls back to the remembered directory when the setting cannot be read', async () => {
    mockWindow(true, 'http://remote:3000');
    getSettingMock.mockRejectedValue(new Error('settings unavailable'));
    await expect(getEffectiveWorkingDir()).resolves.toBe('/Users/johannes/home/workspace');
  });
});

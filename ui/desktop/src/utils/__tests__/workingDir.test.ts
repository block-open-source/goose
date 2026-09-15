import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  getInitialWorkingDir,
  refreshWorkingDir,
  resolveWorkingDir,
  setWorkingDir,
} from '../workingDir';

describe('resolveWorkingDir', () => {
  it('uses the configured external backend directory when present', () => {
    expect(resolveWorkingDir(' /home/goose ', 'C:\\Users\\goose', 'C:\\Users\\goose')).toBe(
      '/home/goose'
    );
    expect(resolveWorkingDir(' ', 'C:\\work', 'C:\\Users\\goose')).toBe('C:\\work');
    expect(resolveWorkingDir(undefined, undefined, 'C:\\Users\\goose')).toBe('C:\\Users\\goose');
  });
});

describe('the leased working directory', () => {
  const getWorkingDir = vi.fn();

  beforeEach(() => {
    getWorkingDir.mockReset();
    setWorkingDir(null);
    (globalThis as Record<string, unknown>).window = {
      appConfig: { get: (key: string) => (key === 'GOOSE_WORKING_DIR' ? '/local/dir' : undefined) },
      electron: { getWorkingDir },
    } as unknown as typeof globalThis;
  });

  it('falls back to the directory baked in at window creation', () => {
    expect(getInitialWorkingDir()).toBe('/local/dir');
  });

  it('adopts the directory the window is currently leased to', async () => {
    getWorkingDir.mockResolvedValue('/remote/work');

    await expect(refreshWorkingDir()).resolves.toBe('/remote/work');
    expect(getInitialWorkingDir()).toBe('/remote/work');
  });

  it('keeps the last known directory when the lease cannot be read', async () => {
    setWorkingDir('/remote/work');
    getWorkingDir.mockRejectedValue(new Error('no lease'));

    await expect(refreshWorkingDir()).resolves.toBe('/remote/work');
  });

  it('ignores an empty directory from the lease', async () => {
    setWorkingDir('/remote/work');
    getWorkingDir.mockResolvedValue('');

    await expect(refreshWorkingDir()).resolves.toBe('/remote/work');
  });

  it('clears back to the window default when reset', () => {
    setWorkingDir('/remote/work');
    expect(getInitialWorkingDir()).toBe('/remote/work');

    setWorkingDir(null);
    expect(getInitialWorkingDir()).toBe('/local/dir');
  });
});

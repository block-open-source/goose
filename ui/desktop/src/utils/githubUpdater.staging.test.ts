import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('./logger', () => ({
  default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
}));

import { GitHubUpdater } from './githubUpdater';

const cleanupPaths = new Set<string>();

afterEach(async () => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  await Promise.all(
    [...cleanupPaths].map((entry) => fs.rm(entry, { recursive: true, force: true }))
  );
  cleanupPaths.clear();
});

describe('GitHubUpdater download staging', () => {
  it.skipIf(process.platform === 'win32')(
    'does not adopt a predictable directory or follow a pre-positioned archive symlink',
    async () => {
      const tempRoot = await fs.realpath(os.tmpdir());
      const workspace = await fs.mkdtemp(path.join(tempRoot, 'goose-updater-race-test-'));
      const victimPath = path.join(workspace, 'victim.txt');
      const version = '9.9.9';
      const fixedNow = Date.now() + process.pid;
      const predictableDir = path.join(tempRoot, `goose-update-${version}-${fixedNow}`);
      const predictableArchive = path.join(predictableDir, `Goose-${version}.zip`);
      cleanupPaths.add(workspace);
      cleanupPaths.add(predictableDir);

      await fs.writeFile(victimPath, 'do not overwrite');
      await fs.mkdir(predictableDir, { mode: 0o777 });
      await fs.symlink(victimPath, predictableArchive);
      vi.spyOn(Date, 'now').mockReturnValue(fixedNow);
      vi.stubGlobal(
        'fetch',
        vi.fn(
          async () =>
            new Response(new Uint8Array([0x50, 0x4b, 0x03, 0x04]), {
              status: 200,
              headers: { 'content-length': '4' },
            })
        )
      );

      const result = await new GitHubUpdater().downloadUpdate(
        'https://example.invalid/Goose.zip',
        version
      );
      expect(result.success).toBe(true);
      expect(result.downloadPath).toBeDefined();
      expect(result.extractedPath).toBeDefined();

      const stagingDir = result.extractedPath!;
      const archivePath = result.downloadPath!;
      cleanupPaths.add(stagingDir);

      expect(await fs.readFile(victimPath, 'utf8')).toBe('do not overwrite');
      expect(stagingDir).not.toBe(predictableDir);
      expect(path.dirname(archivePath)).toBe(stagingDir);
      expect((await fs.stat(stagingDir)).mode & 0o777).toBe(0o700);

      const archiveStat = await fs.lstat(archivePath);
      expect(archiveStat.isFile()).toBe(true);
      expect(archiveStat.mode & 0o777).toBe(0o600);
      expect(await fs.readFile(archivePath)).toEqual(Buffer.from([0x50, 0x4b, 0x03, 0x04]));
    }
  );
});

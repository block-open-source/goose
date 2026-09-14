import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { resolveInstallTarget } from './githubUpdater';

const tempDirs: string[] = [];
const originalPlatform = process.platform;

function setPlatform(platform: typeof process.platform): void {
  Object.defineProperty(process, 'platform', { value: platform, configurable: true });
}

async function makeTempDir(): Promise<string> {
  const dir = await fs.mkdtemp(path.join(await fs.realpath(os.tmpdir()), 'goose-target-test-'));
  tempDirs.push(dir);
  return dir;
}

async function makeInstallDir(root: string, name: string): Promise<string> {
  const installDir = path.join(root, name);
  await fs.mkdir(path.join(installDir, 'resources'), { recursive: true });
  await fs.mkdir(path.join(installDir, 'locales'), { recursive: true });
  await fs.writeFile(path.join(installDir, 'resources', 'app.asar'), 'asar');
  await fs.writeFile(path.join(installDir, 'locales', 'en-US.pak'), 'pak');
  const exePath = path.join(installDir, originalPlatform === 'win32' ? 'Goose.exe' : 'goose');
  await fs.writeFile(exePath, 'binary');
  return exePath;
}

afterEach(async () => {
  setPlatform(originalPlatform);
  await Promise.all(tempDirs.splice(0).map((dir) => fs.rm(dir, { recursive: true, force: true })));
});

describe('resolveInstallTarget', () => {
  it('resolves the .app bundle on macOS', async () => {
    setPlatform('darwin');
    // The mocked platform does not change `path`, which stays the host
    // implementation, so build the expectation the same way the code under test
    // does instead of hardcoding POSIX separators.
    const bundlePath = path.resolve('/Applications/Goose.app');
    const exePath = path.join(bundlePath, 'Contents', 'MacOS', 'Goose');

    await expect(resolveInstallTarget(exePath)).resolves.toEqual({
      targetPath: bundlePath,
      relaunchPath: bundlePath,
      executableRelativePath: path.join('Contents', 'MacOS', 'Goose'),
    });
  });

  it('rejects a macOS executable that is not inside a bundle', async () => {
    setPlatform('darwin');

    await expect(resolveInstallTarget('/usr/local/bin/goose')).rejects.toThrow(
      /Could not locate running .app bundle/
    );
  });

  it('accepts a packaged install directory on other platforms', async () => {
    setPlatform('linux');
    const root = await makeTempDir();
    const exePath = await makeInstallDir(root, 'goose-linux-x64');

    await expect(resolveInstallTarget(exePath)).resolves.toEqual({
      targetPath: path.dirname(exePath),
      relaunchPath: exePath,
      executableRelativePath: path.basename(exePath),
    });
  });

  it('refuses to update when the executable parent is not a packaged app directory', async () => {
    setPlatform('linux');
    const root = await makeTempDir();
    const exePath = path.join(root, 'goose');
    await fs.writeFile(exePath, 'binary');
    await fs.writeFile(path.join(root, 'tax-return.pdf'), 'important');

    await expect(resolveInstallTarget(exePath)).rejects.toThrow(
      /does not look like an app install directory/
    );
  });

  it('refuses to update when the install directory is a shared directory', async () => {
    setPlatform('linux');
    const root = await makeTempDir();
    const exePath = await makeInstallDir(root, 'Downloads');

    await expect(resolveInstallTarget(exePath)).rejects.toThrow(/is a shared directory/);
  });

  it('refuses to update a plausibly named directory shared with unrelated files', async () => {
    setPlatform('linux');
    const root = await makeTempDir();
    const exePath = await makeInstallDir(root, 'Stuff');
    await fs.writeFile(path.join(path.dirname(exePath), 'tax-return.pdf'), 'important');

    await expect(resolveInstallTarget(exePath)).rejects.toThrow(/is not dedicated to the app/);
  });

  it('refuses to update when the install directory is missing Electron runtime directories', async () => {
    setPlatform('linux');
    const root = await makeTempDir();
    const installDir = path.join(root, 'goose-linux-x64');
    await fs.mkdir(path.join(installDir, 'resources'), { recursive: true });
    await fs.writeFile(path.join(installDir, 'resources', 'app.asar'), 'asar');
    const exePath = path.join(installDir, 'goose');
    await fs.writeFile(exePath, 'binary');

    await expect(resolveInstallTarget(exePath)).rejects.toThrow(
      /does not look like an app install directory/
    );
  });

  it('refuses to update when the install directory is the home directory', async () => {
    setPlatform('linux');
    const home = path.resolve(os.homedir());

    await expect(resolveInstallTarget(path.join(home, 'goose'))).rejects.toThrow(
      /Refusing to auto-update/
    );
  });
});

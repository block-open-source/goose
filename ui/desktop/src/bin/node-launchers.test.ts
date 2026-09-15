import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';

const launcherSourceDir = path.dirname(fileURLToPath(import.meta.url));
const tempDirs: string[] = [];

function makeLauncherHarness(launcherName: 'node' | 'npx') {
  const rootDir = fs.mkdtempSync(path.join(os.tmpdir(), 'goose node launchers '));
  tempDirs.push(rootDir);

  const launcherDir = path.join(rootDir, 'launcher');
  const callerDir = path.join(rootDir, 'caller directory');
  const setupDir = path.join(rootDir, 'setup directory');
  const fakeBinDir = path.join(rootDir, 'fake bin');
  fs.mkdirSync(launcherDir);
  fs.mkdirSync(callerDir);
  fs.mkdirSync(setupDir);
  fs.mkdirSync(fakeBinDir);

  const launcherPath = path.join(launcherDir, launcherName);
  fs.copyFileSync(path.join(launcherSourceDir, launcherName), launcherPath);
  fs.chmodSync(launcherPath, 0o755);

  fs.writeFileSync(
    path.join(launcherDir, 'node-setup-common.sh'),
    ['cd -- "$FAKE_SETUP_DIR"', 'export PATH="$FAKE_BIN_DIR:$PATH"', 'log() { :; }', ''].join('\n')
  );

  const childPath = path.join(fakeBinDir, launcherName);
  fs.writeFileSync(
    childPath,
    ['#!/bin/bash', 'printf "%s\\n" "$PWD"', 'exit "$FAKE_CHILD_EXIT_CODE"', ''].join('\n')
  );
  fs.chmodSync(childPath, 0o755);

  return { callerDir, fakeBinDir, launcherPath, setupDir };
}

function runLauncher(launcherName: 'node' | 'npx', exitCode: number) {
  const harness = makeLauncherHarness(launcherName);
  const result = spawnSync(harness.launcherPath, ['--fixture'], {
    cwd: harness.callerDir,
    encoding: 'utf8',
    env: {
      ...process.env,
      FAKE_BIN_DIR: harness.fakeBinDir,
      FAKE_CHILD_EXIT_CODE: String(exitCode),
      FAKE_SETUP_DIR: harness.setupDir,
    },
  });

  expect(result.error).toBeUndefined();
  return { ...harness, callerDir: fs.realpathSync(harness.callerDir), result };
}

afterEach(() => {
  while (tempDirs.length > 0) {
    fs.rmSync(tempDirs.pop()!, { recursive: true, force: true });
  }
});

describe.skipIf(process.platform === 'win32').each(['node', 'npx'] as const)(
  '%s desktop launcher',
  (launcherName) => {
    it('runs the child from the caller working directory', () => {
      const { callerDir, result } = runLauncher(launcherName, 0);

      expect(result.status).toBe(0);
      expect(result.stdout.trim()).toBe(callerDir);
    });

    it('propagates a nonzero child exit status', () => {
      const { callerDir, result } = runLauncher(launcherName, 37);

      expect(result.status).toBe(37);
      expect(result.stdout.trim()).toBe(callerDir);
    });
  }
);

import { app } from 'electron';
import { compareVersions } from 'compare-versions';
import { spawn } from 'child_process';
import * as fs from 'fs/promises';
import * as path from 'path';
import * as os from 'os';
import log from './logger';
import { safeJsonParse, errorMessage } from './conversionUtils';

interface GitHubRelease {
  tag_name: string;
  name: string;
  published_at: string;
  html_url: string;
  assets: Array<{
    name: string;
    browser_download_url: string;
    size: number;
  }>;
}

interface UpdateCheckResult {
  updateAvailable: boolean;
  latestVersion?: string;
  downloadUrl?: string;
  releaseUrl?: string;
  error?: string;
}

interface InstallTarget {
  targetPath: string;
  relaunchPath: string;
  // Used to confirm the extracted payload really is an app before the backup is deleted.
  executableRelativePath: string;
}

interface SwapCommand {
  command: string;
  args: string[];
}

function runCommand(command: string, args: string[]): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: 'ignore', windowsHide: true });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${command} exited with code ${code}`));
      }
    });
  });
}

function powershellQuote(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

async function extractArchive(archivePath: string, destDir: string): Promise<void> {
  if (process.platform === 'darwin') {
    await runCommand('ditto', ['-x', '-k', archivePath, destDir]);
  } else if (process.platform === 'win32') {
    await runCommand('powershell.exe', [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `Expand-Archive -LiteralPath ${powershellQuote(archivePath)} -DestinationPath ${powershellQuote(destDir)} -Force`,
    ]);
  } else {
    await runCommand('unzip', ['-q', '-o', archivePath, '-d', destDir]);
  }
}

async function resolvePayloadPath(extractDir: string): Promise<string> {
  let current = extractDir;

  for (let depth = 0; depth < 3; depth += 1) {
    const entries = (await fs.readdir(current, { withFileTypes: true })).filter(
      (entry) => !entry.name.startsWith('.') && entry.name !== '__MACOSX'
    );

    const appBundle = entries.find((entry) => entry.isDirectory() && entry.name.endsWith('.app'));
    if (appBundle) {
      return path.join(current, appBundle.name);
    }

    if (entries.length === 1 && entries[0].isDirectory()) {
      current = path.join(current, entries[0].name);
      continue;
    }

    return current;
  }

  return current;
}

// Electron ships these alongside the executable in every packaged build, so an install
// root always contains them. Their absence means the directory is not an install root.
const REQUIRED_INSTALL_DIRECTORIES = ['locales', 'resources'];

// Everything a packaged Electron app is allowed to place next to its executable. Anything
// else means the directory holds unrelated files and cannot be replaced wholesale.
const ELECTRON_RUNTIME_DIRECTORIES = new Set(['locales', 'resources', 'swiftshader']);

const ELECTRON_RUNTIME_FILES = new Set([
  'chrome-sandbox',
  'chrome_crashpad_handler',
  'icudtl.dat',
  'libvulkan.so.1',
  'license',
  'licenses.chromium.html',
  'version',
]);

const ELECTRON_RUNTIME_EXTENSIONS = new Set([
  '.bin',
  '.dat',
  '.dll',
  '.exe',
  '.html',
  '.json',
  '.node',
  '.pak',
  '.so',
  '.txt',
]);

// Directories users commonly unpack portable builds into. Replacing one of these
// wholesale would delete unrelated files, so an install there is never swapped.
const SHARED_DIRECTORY_NAMES = new Set([
  'applications',
  'appdata',
  'bin',
  'desktop',
  'documents',
  'downloads',
  'dropbox',
  'etc',
  'home',
  'local',
  'music',
  'onedrive',
  'opt',
  'pictures',
  'program files',
  'program files (x86)',
  'programdata',
  'roaming',
  'temp',
  'tmp',
  'usr',
  'users',
  'var',
  'videos',
]);

async function pathExists(target: string): Promise<boolean> {
  try {
    await fs.access(target);
    return true;
  } catch {
    return false;
  }
}

function isSharedDirectory(dir: string): boolean {
  if (dir === path.parse(dir).root) {
    return true;
  }
  if (dir === path.resolve(os.homedir()) || dir === path.resolve(os.tmpdir())) {
    return true;
  }
  return SHARED_DIRECTORY_NAMES.has(path.basename(dir).toLowerCase());
}

async function isDirectory(target: string): Promise<boolean> {
  try {
    return (await fs.stat(target)).isDirectory();
  } catch {
    return false;
  }
}

// Packaged Electron apps always ship resources/app.asar (or an unpacked resources/app)
// alongside the locales directory, which distinguishes an install root from an arbitrary folder.
async function isPackagedAppDirectory(dir: string): Promise<boolean> {
  for (const required of REQUIRED_INSTALL_DIRECTORIES) {
    if (!(await isDirectory(path.join(dir, required)))) {
      return false;
    }
  }

  const resources = path.join(dir, 'resources');
  return (
    (await pathExists(path.join(resources, 'app.asar'))) ||
    (await pathExists(path.join(resources, 'app')))
  );
}

// A basename blacklist cannot prove a directory is safe to replace, so require that every
// entry belongs to a packaged Electron app. Anything else means the directory is shared
// with unrelated files that a wholesale swap would delete.
async function findUnexpectedInstallEntries(dir: string, exeName: string): Promise<string[]> {
  const entries = await fs.readdir(dir, { withFileTypes: true });

  return entries
    .filter((entry) => {
      const name = entry.name.toLowerCase();
      if (name.startsWith('.') || name === exeName.toLowerCase()) {
        return false;
      }
      if (entry.isDirectory()) {
        return !ELECTRON_RUNTIME_DIRECTORIES.has(name);
      }
      return (
        !ELECTRON_RUNTIME_FILES.has(name) && !ELECTRON_RUNTIME_EXTENSIONS.has(path.extname(name))
      );
    })
    .map((entry) => entry.name);
}

export async function resolveInstallTarget(exePath: string): Promise<InstallTarget> {
  const resolvedExePath = path.resolve(exePath);

  if (process.platform === 'darwin') {
    const appPath = path.resolve(resolvedExePath, '..', '..', '..');
    if (!appPath.endsWith('.app')) {
      throw new Error(`Could not locate running .app bundle from ${resolvedExePath}`);
    }
    return {
      targetPath: appPath,
      relaunchPath: appPath,
      executableRelativePath: path.relative(appPath, resolvedExePath),
    };
  }

  const installDir = path.dirname(resolvedExePath);

  if (!(await isPackagedAppDirectory(installDir))) {
    throw new Error(
      `Refusing to auto-update: ${installDir} does not look like an app install directory`
    );
  }

  if (isSharedDirectory(installDir)) {
    throw new Error(`Refusing to auto-update: ${installDir} is a shared directory`);
  }

  const unexpected = await findUnexpectedInstallEntries(installDir, path.basename(resolvedExePath));
  if (unexpected.length > 0) {
    throw new Error(
      `Refusing to auto-update: ${installDir} is not dedicated to the app (found ${unexpected
        .slice(0, 5)
        .join(', ')})`
    );
  }

  return {
    targetPath: installDir,
    relaunchPath: resolvedExePath,
    executableRelativePath: path.basename(resolvedExePath),
  };
}

async function writeSwapScript(options: {
  stagingDir: string;
  payloadPath: string;
  targetPath: string;
  relaunchPath: string;
  executableRelativePath: string;
  pid: number;
}): Promise<SwapCommand> {
  const { stagingDir, payloadPath, targetPath, relaunchPath, executableRelativePath, pid } =
    options;
  // The script deletes its staging directory once it finishes, so the log lives beside that
  // directory to survive cleanup and stay available when diagnosing a failed update.
  const logPath = `${stagingDir}-install.log`;
  // The previous install is moved aside rather than deleted so a failed copy can be rolled back.
  // It stays beside the target so the move is a same-filesystem rename instead of a full copy.
  const backupPath = `${targetPath}.goose-previous`;

  if (process.platform === 'win32') {
    const scriptPath = path.join(stagingDir, 'swap-and-relaunch.ps1');
    // Copy-Item nests the source inside an existing destination directory, so the payload
    // contents are copied into a freshly created target instead of the payload directory itself.
    // Get-ChildItem enumerates them via -LiteralPath so paths containing glob metacharacters
    // are not expanded, and -Force keeps hidden entries.
    const installedExe = powershellQuote(path.join(targetPath, executableRelativePath));
    const script = [
      `$ErrorActionPreference = 'Continue'`,
      // Start-Transcript silently produces no file when it is unavailable, so the log is written
      // directly to keep a failing detached script diagnosable.
      `function Write-Log($message) { try { Add-Content -LiteralPath ${powershellQuote(logPath)} -Value $message } catch {} }`,
      `Write-Log "swap starting for pid ${pid}"`,
      // A process object reports HasExited once the app is gone, which distinguishes a live app
      // from the handle that lingers briefly after exit.
      `$attempt = 0`,
      `while ($attempt -lt 120) {`,
      `  $proc = Get-Process -Id ${pid} -ErrorAction SilentlyContinue`,
      `  if (-not $proc -or $proc.HasExited) { break }`,
      `  Start-Sleep -Milliseconds 500`,
      `  $attempt = $attempt + 1`,
      `}`,
      // Replacing an install while it runs corrupts it, so a stalled quit aborts the swap.
      `$proc = Get-Process -Id ${pid} -ErrorAction SilentlyContinue`,
      `if ($proc -and -not $proc.HasExited) {`,
      `  Write-Log 'app is still running; aborting update'`,
      `  exit 1`,
      `}`,
      `Write-Log 'app has exited; swapping install'`,
      `Remove-Item -LiteralPath ${powershellQuote(backupPath)} -Recurse -Force -ErrorAction SilentlyContinue`,
      `Move-Item -LiteralPath ${powershellQuote(targetPath)} -Destination ${powershellQuote(backupPath)} -Force`,
      `if (Test-Path -LiteralPath ${powershellQuote(targetPath)}) { throw 'Could not move previous install aside' }`,
      `try {`,
      `  New-Item -ItemType Directory -Path ${powershellQuote(targetPath)} -Force -ErrorAction Stop | Out-Null`,
      `  $payloadEntries = (Get-ChildItem -LiteralPath ${powershellQuote(payloadPath)} -Force).FullName`,
      `  Copy-Item -LiteralPath $payloadEntries -Destination ${powershellQuote(targetPath)} -Recurse -Force -ErrorAction Stop`,
      // A valid archive can still be packaged without the executable, so the backup is only
      // discarded once the copied payload is confirmed to be a runnable install.
      `  if (-not (Test-Path -LiteralPath ${installedExe})) { throw 'Updated install is missing its executable' }`,
      `  Remove-Item -LiteralPath ${powershellQuote(backupPath)} -Recurse -Force -ErrorAction SilentlyContinue`,
      `} catch {`,
      `  Remove-Item -LiteralPath ${powershellQuote(targetPath)} -Recurse -Force -ErrorAction SilentlyContinue`,
      `  Move-Item -LiteralPath ${powershellQuote(backupPath)} -Destination ${powershellQuote(targetPath)} -Force`,
      `}`,
      `Start-Process -FilePath ${powershellQuote(relaunchPath)}`,
      `try { Stop-Transcript | Out-Null } catch {}`,
      `Remove-Item -LiteralPath ${powershellQuote(stagingDir)} -Recurse -Force -ErrorAction SilentlyContinue`,
      '',
    ].join('\r\n');

    await fs.writeFile(scriptPath, script);
    return {
      command: 'powershell.exe',
      args: ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', scriptPath],
    };
  }

  const scriptPath = path.join(stagingDir, 'swap-and-relaunch.sh');
  const quotedPayload = shellQuote(payloadPath);
  const quotedTarget = shellQuote(targetPath);
  const quotedBackup = shellQuote(backupPath);
  const quotedRelaunch = shellQuote(relaunchPath);
  const quotedInstalledExe = shellQuote(path.join(targetPath, executableRelativePath));
  const copyCommand =
    process.platform === 'darwin'
      ? `ditto ${quotedPayload} ${quotedTarget}`
      : `cp -a ${quotedPayload} ${quotedTarget}`;
  const relaunch =
    process.platform === 'darwin'
      ? [`xattr -dr com.apple.quarantine ${quotedTarget} || true`, `open ${quotedRelaunch}`]
      : [`${quotedRelaunch} >/dev/null 2>&1 &`];

  const script = [
    '#!/bin/sh',
    'set -e',
    `exec >> ${shellQuote(logPath)} 2>&1`,
    'attempt=0',
    'while [ "$attempt" -lt 120 ]; do',
    `  kill -0 ${pid} 2>/dev/null || break`,
    '  sleep 0.5',
    '  attempt=$((attempt + 1))',
    'done',
    // Touching a live bundle corrupts the running app, so a stalled shutdown aborts the swap.
    `if kill -0 ${pid} 2>/dev/null; then`,
    '  echo "App is still running; aborting update"',
    '  exit 1',
    'fi',
    `rm -rf ${quotedBackup}`,
    `mv ${quotedTarget} ${quotedBackup}`,
    // A valid archive can still be packaged without the executable, so the backup is only
    // discarded once the copied payload is confirmed to be a runnable install.
    `if ${copyCommand} && [ -x ${quotedInstalledExe} ]; then`,
    `  rm -rf ${quotedBackup}`,
    'else',
    `  rm -rf ${quotedTarget}`,
    `  mv ${quotedBackup} ${quotedTarget}`,
    'fi',
    ...relaunch,
    `rm -rf ${shellQuote(stagingDir)}`,
    '',
  ].join('\n');

  await fs.writeFile(scriptPath, script, { mode: 0o755 });
  return { command: '/bin/sh', args: [scriptPath] };
}

// Node's detached flag becomes DETACHED_PROCESS on Windows, which leaves the child with no
// console, and powershell.exe exits before its first statement without one. windowsHide still
// allocates a console without showing a window, and Windows keeps a child alive after its
// parent exits, so the swap outlives the quit without detaching. POSIX still detaches so the
// script survives the app's process group going away.
export function launchSwapScript(swap: SwapCommand): void {
  const child = spawn(swap.command, swap.args, {
    detached: process.platform !== 'win32',
    stdio: 'ignore',
    windowsHide: true,
  });
  child.unref();
}

// A ZIP can be valid yet packaged without the expected application, which would let the swap
// replace a working install with an unrunnable one. Checking before the backup is deleted keeps
// the failure recoverable.
async function assertPayloadIsRunnable(
  payloadPath: string,
  executableRelativePath: string
): Promise<void> {
  const executable = path.join(payloadPath, executableRelativePath);
  if (!(await pathExists(executable))) {
    throw new Error(
      `Update payload is missing its executable (expected ${executableRelativePath} in ${path.basename(payloadPath)})`
    );
  }

  if (process.platform === 'darwin' && !payloadPath.endsWith('.app')) {
    throw new Error(`Update payload is not an .app bundle: ${payloadPath}`);
  }
}

export async function prepareUpdateInstall(options: {
  archivePath: string;
  targetPath: string;
  relaunchPath: string;
  executableRelativePath: string;
  pid: number;
}): Promise<SwapCommand> {
  const stagingDir = path.dirname(options.archivePath);
  const extractDir = path.join(stagingDir, 'extracted');

  await fs.rm(extractDir, { recursive: true, force: true });
  await fs.mkdir(extractDir, { recursive: true });
  await extractArchive(options.archivePath, extractDir);

  const payloadPath = await resolvePayloadPath(extractDir);
  log.info(`GitHubUpdater: Update payload: ${payloadPath}`);

  await assertPayloadIsRunnable(payloadPath, options.executableRelativePath);

  return writeSwapScript({
    stagingDir,
    payloadPath,
    targetPath: options.targetPath,
    relaunchPath: options.relaunchPath,
    executableRelativePath: options.executableRelativePath,
    pid: options.pid,
  });
}

export class GitHubUpdater {
  private readonly owner = process.env.GITHUB_OWNER || 'aaif-goose';
  private readonly repo = process.env.GITHUB_REPO || 'goose';
  private readonly bundleName = process.env.GOOSE_BUNDLE_NAME || 'Goose';
  private readonly apiUrl = `https://api.github.com/repos/${this.owner}/${this.repo}/releases/latest`;

  async checkForUpdates(): Promise<UpdateCheckResult> {
    const startTime = Date.now();
    try {
      log.info('=== GitHubUpdater: STARTING UPDATE CHECK ===');
      log.info(`GitHubUpdater: API URL: ${this.apiUrl}`);
      log.info(`GitHubUpdater: Current app version: ${app.getVersion()}`);
      log.info(`GitHubUpdater: Timestamp: ${new Date().toISOString()}`);

      log.info('GitHubUpdater: Initiating fetch request...');
      const controller = new AbortController();
      const timeoutId = setTimeout(() => {
        log.error('GitHubUpdater: Fetch request timed out after 30 seconds');
        controller.abort();
      }, 30000);

      const response = await fetch(this.apiUrl, {
        headers: {
          Accept: 'application/vnd.github.v3+json',
          'User-Agent': `Goose-Desktop/${app.getVersion()}`,
        },
        signal: controller.signal,
      });

      clearTimeout(timeoutId);
      const fetchDuration = Date.now() - startTime;
      log.info(
        `GitHubUpdater: GitHub API response status: ${response.status} ${response.statusText} (took ${fetchDuration}ms)`
      );

      if (!response.ok) {
        const errorText = await response.text();
        log.error(`GitHubUpdater: GitHub API error response: ${errorText}`);
        throw new Error(`GitHub API returned ${response.status}: ${response.statusText}`);
      }

      const release: GitHubRelease = await safeJsonParse<GitHubRelease>(
        response,
        'Failed to get GitHub release information'
      );
      log.info(`GitHubUpdater: Found release: ${release.tag_name} (${release.name})`);
      log.info(`GitHubUpdater: Release published at: ${release.published_at}`);
      log.info(`GitHubUpdater: Release assets count: ${release.assets.length}`);

      const latestVersion = release.tag_name.replace(/^v/, ''); // Remove 'v' prefix if present
      const currentVersion = app.getVersion();

      log.info(
        `GitHubUpdater: Current version: ${currentVersion}, Latest version: ${latestVersion}`
      );

      // Compare versions
      const updateAvailable = compareVersions(latestVersion, currentVersion) > 0;
      log.info(`GitHubUpdater: Update available: ${updateAvailable}`);

      if (!updateAvailable) {
        return {
          updateAvailable: false,
          latestVersion,
        };
      }

      // Find the appropriate download URL based on platform
      const platform = process.platform;
      const arch = process.arch;
      let downloadUrl: string | undefined;
      let assetName: string;

      log.info(`GitHubUpdater: Looking for asset for platform: ${platform}, arch: ${arch}`);

      if (platform === 'darwin') {
        // macOS
        if (arch === 'arm64') {
          assetName = `${this.bundleName}.zip`;
        } else {
          assetName = `${this.bundleName}_intel_mac.zip`;
        }
      } else if (platform === 'win32') {
        // Windows - for future support
        assetName = `${this.bundleName}-win32-x64.zip`;
      } else {
        // Linux - for future support
        assetName = `${this.bundleName}-linux-${arch}.zip`;
      }

      log.info(`GitHubUpdater: Looking for asset named: ${assetName}`);
      log.info(`GitHubUpdater: Available assets: ${release.assets.map((a) => a.name).join(', ')}`);

      const asset = release.assets.find((a) => a.name.toLowerCase() === assetName.toLowerCase()); // keeping comparison to lowercase because Goose vs goose
      if (asset) {
        downloadUrl = asset.browser_download_url;
        log.info(`GitHubUpdater: Found matching asset: ${asset.name} (${asset.size} bytes)`);
        log.info(`GitHubUpdater: Download URL: ${downloadUrl}`);
      } else {
        log.warn(`GitHubUpdater: No matching asset found for ${assetName}`);
      }

      if (!downloadUrl) {
        throw new Error(
          `Update Available but no download URL found for platform: ${platform}, arch: ${arch}`
        );
      }

      return {
        updateAvailable: true,
        latestVersion,
        downloadUrl,
        releaseUrl: release.html_url,
      };
    } catch (error) {
      log.error('GitHubUpdater: Error checking for updates:', error);
      log.error('GitHubUpdater: Error details:', {
        message: errorMessage(error, 'Unknown error'),
        stack: error instanceof Error ? error.stack : 'No stack',
        name: error instanceof Error ? error.name : 'Unknown',
        code:
          error instanceof Error && 'code' in error
            ? (error as Error & { code: unknown }).code
            : undefined,
      });
      return {
        updateAvailable: false,
        error: errorMessage(error, 'Unknown error'),
      };
    }
  }

  async downloadUpdate(
    downloadUrl: string,
    latestVersion: string,
    onProgress?: (percent: number) => void
  ): Promise<{ success: boolean; downloadPath?: string; extractedPath?: string; error?: string }> {
    const downloadStartTime = Date.now();
    try {
      log.info('=== GitHubUpdater: STARTING DOWNLOAD ===');
      log.info(`GitHubUpdater: Download URL: ${downloadUrl}`);
      log.info(`GitHubUpdater: Version: ${latestVersion}`);
      log.info(`GitHubUpdater: Timestamp: ${new Date().toISOString()}`);

      log.info('GitHubUpdater: Initiating download fetch request...');
      const response = await fetch(downloadUrl);
      const fetchDuration = Date.now() - downloadStartTime;
      log.info(
        `GitHubUpdater: Download response received in ${fetchDuration}ms - Status: ${response.status} ${response.statusText}`
      );

      if (!response.ok) {
        throw new Error(`Download failed: ${response.status} ${response.statusText}`);
      }

      // Get total size from headers
      const contentLength = response.headers.get('content-length');
      const totalSize = contentLength ? parseInt(contentLength, 10) : 0;
      log.info(
        `GitHubUpdater: Content-Length: ${totalSize} bytes (${(totalSize / 1024 / 1024).toFixed(2)} MB)`
      );

      if (!response.body) {
        throw new Error('Response body is null');
      }
      let lastReportedPercent = -1; // Track last reported percentage to throttle updates
      let lastLoggedPercent = -1; // Track for logging at 10% intervals

      // Read the response stream
      log.info('GitHubUpdater: Starting to read response stream...');
      const reader = response.body.getReader();
      const chunks: Uint8Array[] = [];
      let downloadedSize = 0;
      let lastProgressTime = Date.now();

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        chunks.push(value);
        downloadedSize += value.length;

        // Report progress - only when percentage changes by at least 1%
        if (totalSize > 0 && onProgress) {
          const percent = Math.round((downloadedSize / totalSize) * 100);

          // Only report if percent changed (throttles from hundreds/sec to ~100 total)
          if (percent !== lastReportedPercent) {
            onProgress(percent);
            lastReportedPercent = percent;

            // Log at 10% intervals for debugging
            if (percent % 10 === 0 && percent !== lastLoggedPercent) {
              const elapsed = Date.now() - downloadStartTime;
              const speed = downloadedSize / (elapsed / 1000) / 1024; // KB/s
              log.info(
                `GitHubUpdater: Download progress ${percent}% (${(downloadedSize / 1024 / 1024).toFixed(2)}/${(totalSize / 1024 / 1024).toFixed(2)} MB) @ ${speed.toFixed(0)} KB/s`
              );
              lastLoggedPercent = percent;
            }
          }
        }

        // Warn if no progress for 30 seconds
        const now = Date.now();
        if (now - lastProgressTime > 30000) {
          log.warn(
            `GitHubUpdater: Download appears slow - no significant progress in 30 seconds (${downloadedSize}/${totalSize} bytes)`
          );
          lastProgressTime = now;
        } else if (value.length > 0) {
          lastProgressTime = now;
        }
      }

      const downloadDuration = Date.now() - downloadStartTime;
      const avgSpeed = downloadedSize / (downloadDuration / 1000) / 1024;
      log.info(
        `GitHubUpdater: Download stream complete - ${downloadedSize} bytes in ${downloadDuration}ms (avg ${avgSpeed.toFixed(0)} KB/s)`
      );

      // Combine chunks into a single buffer
      log.info('GitHubUpdater: Combining chunks into buffer...');
      const buffer = Buffer.concat(chunks.map((chunk) => Buffer.from(chunk)));
      log.info(`GitHubUpdater: Buffer created - ${buffer.length} bytes`);

      const stagingDir = await fs.mkdtemp(
        path.join(await fs.realpath(os.tmpdir()), 'goose-update-')
      );
      const fileName = `${this.bundleName}-${latestVersion}.zip`;
      const downloadPath = path.join(stagingDir, fileName);

      log.info(`GitHubUpdater: Writing file to ${downloadPath}...`);
      await fs.writeFile(downloadPath, buffer, { flag: 'wx', mode: 0o600 });

      const totalDuration = Date.now() - downloadStartTime;
      log.info(`=== GitHubUpdater: DOWNLOAD COMPLETE in ${totalDuration}ms ===`);
      log.info(`GitHubUpdater: File saved to ${downloadPath}`);

      return { success: true, downloadPath, extractedPath: stagingDir };
    } catch (error) {
      const duration = Date.now() - downloadStartTime;
      log.error(`=== GitHubUpdater: DOWNLOAD FAILED after ${duration}ms ===`);
      log.error('GitHubUpdater: Error downloading update:', error);
      log.error('GitHubUpdater: Download error details:', {
        message: errorMessage(error, 'Unknown error'),
        stack: error instanceof Error ? error.stack : 'No stack',
        name: error instanceof Error ? error.name : 'Unknown',
      });
      return {
        success: false,
        error: errorMessage(error, 'Unknown error'),
      };
    }
  }

  async installUpdate(downloadPath: string): Promise<{ success: boolean; error?: string }> {
    try {
      log.info('=== GitHubUpdater: STARTING AUTOMATIC INSTALL ===');
      log.info(`GitHubUpdater: Download path: ${downloadPath}`);

      await fs.access(downloadPath);

      const { targetPath, relaunchPath, executableRelativePath } = await resolveInstallTarget(
        app.getPath('exe')
      );
      log.info(`GitHubUpdater: Install target: ${targetPath}`);

      const swap = await prepareUpdateInstall({
        archivePath: downloadPath,
        targetPath,
        relaunchPath,
        executableRelativePath,
        pid: process.pid,
      });

      launchSwapScript(swap);

      log.info('=== GitHubUpdater: SWAP SCRIPT LAUNCHED, app will quit ===');
      return { success: true };
    } catch (error) {
      log.error('GitHubUpdater: Error installing update:', error);
      return {
        success: false,
        error: errorMessage(error, 'Unknown error'),
      };
    }
  }
}

// Create singleton instance
export const githubUpdater = new GitHubUpdater();

import { autoUpdater, UpdateInfo } from 'electron-updater';
import {
  BrowserWindow,
  ipcMain,
  nativeImage,
  Tray,
  app,
  Menu,
  MenuItemConstructorOptions,
  Notification,
} from 'electron';
import * as path from 'path';
import * as fs from 'fs/promises';
import log from './logger';
import { githubUpdater } from './githubUpdater';
import { loadRecentDirs } from './recentDirs';
import { errorMessage } from './conversionUtils';
import {
  trackUpdateCheckStarted,
  trackUpdateCheckCompleted,
  trackUpdateDownloadStarted,
  trackUpdateDownloadProgress,
  trackUpdateDownloadCompleted,
  trackUpdateInstallInitiated,
} from './analytics';

let updateAvailable = false;
let trayRef: Tray | null = null;
let isUsingGitHubFallback = false;
let githubUpdateInfo: {
  latestVersion?: string;
  downloadUrl?: string;
  releaseUrl?: string;
  downloadPath?: string;
  extractedPath?: string;
} = {};

// Store update state
let lastUpdateState: { updateAvailable: boolean; latestVersion?: string } | null = null;

// Track last reported progress to prevent backward jumps
let lastReportedProgress = 0;

// Track if IPC handlers have been registered
let ipcUpdateHandlersRegistered = false;

let autoDownloadDisabled = false;

export function setAutoDownloadDisabled(disabled: boolean) {
  autoDownloadDisabled = disabled;
  autoUpdater.autoDownload = !disabled;
  log.info(`Auto-download ${disabled ? 'disabled' : 'enabled'}`);
}

export function getAutoDownloadDisabled(): boolean {
  return autoDownloadDisabled;
}

// Register IPC handlers (only once)
export function registerUpdateIpcHandlers() {
  if (ipcUpdateHandlersRegistered) {
    return;
  }

  log.info('Registering update IPC handlers...');
  ipcUpdateHandlersRegistered = true;

  // IPC handlers for renderer process
  ipcMain.handle('check-for-updates', async () => {
    const currentVersion = autoUpdater.currentVersion?.version || app.getVersion();
    const checkStartTime = Date.now();

    try {
      log.info('=== MANUAL UPDATE CHECK INITIATED ===');
      log.info(`Manual check for updates requested at ${new Date().toISOString()}`);
      log.info(`Current version: ${currentVersion}`);
      trackUpdateCheckStarted('manual', currentVersion);

      // Reset state for new update check
      isUsingGitHubFallback = false;
      githubUpdateInfo = {};
      lastReportedProgress = 0; // Reset progress tracking

      // Ensure auto-updater is properly initialized
      if (!autoUpdater.currentVersion) {
        log.error('Auto-updater currentVersion is null/undefined');
        trackUpdateCheckCompleted('error', currentVersion, {
          usingFallback: false,
          errorType: 'auto_updater_not_initialized',
        });
        throw new Error('Auto-updater not initialized. Please restart the application.');
      }

      log.info(
        `About to check for updates with currentVersion: ${JSON.stringify(autoUpdater.currentVersion)}`
      );
      log.info(`Feed URL: ${autoUpdater.getFeedURL()}`);

      const result = await autoUpdater.checkForUpdates();
      const duration = Date.now() - checkStartTime;
      log.info(`=== MANUAL UPDATE CHECK COMPLETED in ${duration}ms ===`);
      log.info('Auto-updater checkForUpdates result:', result);

      return {
        updateInfo: result?.updateInfo,
        error: null,
      };
    } catch (error) {
      const duration = Date.now() - checkStartTime;
      log.error(`=== MANUAL UPDATE CHECK FAILED after ${duration}ms ===`);
      log.error('Error checking for updates:', error);
      log.error('Manual check error details:', {
        message: errorMessage(error, 'Unknown error'),
        stack: error instanceof Error ? error.stack : 'No stack',
        name: error instanceof Error ? error.name : 'Unknown',
        code:
          error instanceof Error && 'code' in error
            ? (error as Error & { code: unknown }).code
            : undefined,
        toString: error?.toString(),
      });

      // If electron-updater fails, try GitHub API fallback
      if (
        error instanceof Error &&
        (error.message.includes('HttpError: 404') ||
          error.message.includes('ERR_CONNECTION_REFUSED') ||
          error.message.includes('ENOTFOUND') ||
          error.message.includes('No published versions'))
      ) {
        log.info('Using GitHub API fallback in check-for-updates...');
        log.info('Manual fallback triggered by error:', error.message);
        isUsingGitHubFallback = true;

        try {
          const result = await githubUpdater.checkForUpdates();

          if (result.error) {
            trackUpdateCheckCompleted('error', currentVersion, {
              usingFallback: true,
              errorType: result.error,
            });
            return {
              updateInfo: null,
              error: result.error,
            };
          }

          // Store GitHub update info
          if (result.updateAvailable) {
            githubUpdateInfo = {
              latestVersion: result.latestVersion,
              downloadUrl: result.downloadUrl,
              releaseUrl: result.releaseUrl,
            };

            trackUpdateCheckCompleted('available', currentVersion, {
              latestVersion: result.latestVersion,
              usingFallback: true,
            });

            updateAvailable = true;
            lastUpdateState = { updateAvailable: true, latestVersion: result.latestVersion };
            updateTrayIcon(true);
            sendStatusToWindow('update-available', { version: result.latestVersion });

            if (!autoDownloadDisabled) {
              log.info('Auto-downloading update via GitHub fallback...');
              await githubAutoDownload(result.downloadUrl!, result.latestVersion!, 'manual check');
            } else {
              log.info('Auto-download disabled — skipping GitHub fallback download');
            }
          } else {
            trackUpdateCheckCompleted('not_available', currentVersion, {
              latestVersion: result.latestVersion,
              usingFallback: true,
            });

            updateAvailable = false;
            lastUpdateState = { updateAvailable: false };
            updateTrayIcon(false);
            sendStatusToWindow('update-not-available', {
              version: autoUpdater.currentVersion.version,
            });
          }

          return {
            updateInfo: null,
            error: null,
          };
        } catch (fallbackError) {
          log.error('GitHub fallback also failed:', fallbackError);
          trackUpdateCheckCompleted('error', currentVersion, {
            usingFallback: true,
            errorType: 'github_fallback_failed',
          });
          return {
            updateInfo: null,
            error: 'Unable to check for updates. Please check your internet connection.',
          };
        }
      }

      trackUpdateCheckCompleted('error', currentVersion, {
        usingFallback: false,
        errorType: errorMessage(error, 'unknown'),
      });

      return {
        updateInfo: null,
        error: errorMessage(error, 'Unknown error'),
      };
    }
  });

  ipcMain.handle('download-update', async () => {
    try {
      if (isUsingGitHubFallback && githubUpdateInfo.downloadUrl && githubUpdateInfo.latestVersion) {
        log.info('Using GitHub fallback for download...');
        lastReportedProgress = 0; // Reset progress tracking
        trackUpdateDownloadStarted(githubUpdateInfo.latestVersion, 'github-fallback');

        const result = await githubUpdater.downloadUpdate(
          githubUpdateInfo.downloadUrl,
          githubUpdateInfo.latestVersion,
          (percent) => {
            // Only send if progress increased (monotonic)
            if (percent > lastReportedProgress) {
              lastReportedProgress = percent;
              trackUpdateDownloadProgress(percent);
              sendStatusToWindow('download-progress', { percent });
            }
          }
        );

        if (result.success && result.downloadPath) {
          githubUpdateInfo.downloadPath = result.downloadPath;
          githubUpdateInfo.extractedPath = result.extractedPath;
          trackUpdateDownloadCompleted(true, githubUpdateInfo.latestVersion, 'github-fallback');
          sendStatusToWindow('update-downloaded', { version: githubUpdateInfo.latestVersion });
          return { success: true, error: null };
        } else {
          const errorMsg = result.error || 'Download failed';
          trackUpdateDownloadCompleted(
            false,
            githubUpdateInfo.latestVersion,
            'github-fallback',
            errorMsg
          );
          throw new Error(errorMsg);
        }
      } else {
        // Use electron-updater
        const version = lastUpdateState?.latestVersion || 'unknown';
        trackUpdateDownloadStarted(version, 'electron-updater');
        await autoUpdater.downloadUpdate();
        return { success: true, error: null };
      }
    } catch (error) {
      log.error('Error downloading update:', error);
      const version = githubUpdateInfo.latestVersion || lastUpdateState?.latestVersion || 'unknown';
      const method = isUsingGitHubFallback ? 'github-fallback' : 'electron-updater';
      trackUpdateDownloadCompleted(false, version, method, errorMessage(error, 'unknown'));
      return {
        success: false,
        error: errorMessage(error, 'Unknown error'),
      };
    }
  });

  ipcMain.handle('install-update', async () => {
    if (isUsingGitHubFallback) {
      log.info('Installing update from GitHub fallback...');

      const downloadPath = githubUpdateInfo.downloadPath;
      if (!downloadPath) {
        throw new Error('Update file path not found. Please download the update first.');
      }

      try {
        await fs.access(downloadPath);
      } catch {
        throw new Error('Update file not found. Please download the update first.');
      }

      trackUpdateInstallInitiated(
        githubUpdateInfo.latestVersion || 'unknown',
        'github-fallback',
        'auto_swap_and_relaunch'
      );

      const result = await githubUpdater.installUpdate(downloadPath);
      if (!result.success) {
        log.error('Error installing GitHub update:', result.error);
        throw new Error(result.error || 'Failed to install update');
      }

      log.info('Quitting app so the update swap can complete...');
      setTimeout(() => app.quit(), 0);
    } else {
      // Use electron-updater's built-in install
      trackUpdateInstallInitiated(
        lastUpdateState?.latestVersion || 'unknown',
        'electron-updater',
        'quit_and_install'
      );
      autoUpdater.quitAndInstall(false, true);
    }
  });

  ipcMain.handle('get-current-version', () => {
    return autoUpdater.currentVersion.version;
  });

  ipcMain.handle('get-update-state', () => {
    return lastUpdateState;
  });

  ipcMain.handle('is-using-github-fallback', () => {
    return isUsingGitHubFallback;
  });

  ipcMain.handle('get-auto-download-disabled', () => {
    return autoDownloadDisabled;
  });
}

// Configure auto-updater
export function setupAutoUpdater(tray?: Tray) {
  if (tray) {
    trayRef = tray;
  }

  log.info('Setting up auto-updater...');
  log.info(`Current app version: ${app.getVersion()}`);
  log.info(`Platform: ${process.platform}, Arch: ${process.arch}`);
  log.info(`ENABLE_DEV_UPDATES: ${process.env.ENABLE_DEV_UPDATES}`);
  log.info(`App is packaged: ${app.isPackaged}`);
  log.info(`App path: ${app.getAppPath()}`);
  log.info(`Resources path: ${process.resourcesPath}`);

  // Set the feed URL for GitHub releases
  const feedConfig = {
    provider: 'github' as const,
    owner: 'aaif-goose',
    repo: 'goose',
    releaseType: 'release' as const,
  };

  log.info('Setting feed URL with config:', feedConfig);
  autoUpdater.setFeedURL(feedConfig);

  // Log the feed URL after setting it
  try {
    const feedUrl = autoUpdater.getFeedURL();
    log.info(`Feed URL set to: ${feedUrl}`);
  } catch (e) {
    log.error('Error getting feed URL:', e);
  }

  // Respect GOOSE_DISABLE_AUTO_DOWNLOAD env var (takes precedence over user setting)
  const envDisabled =
    process.env.GOOSE_DISABLE_AUTO_DOWNLOAD === '1' ||
    process.env.GOOSE_DISABLE_AUTO_DOWNLOAD === 'true';
  if (envDisabled) {
    autoDownloadDisabled = true;
    log.info('Auto-download disabled via GOOSE_DISABLE_AUTO_DOWNLOAD environment variable');
  }

  // Configure auto-updater settings
  autoUpdater.autoDownload = !autoDownloadDisabled;
  autoUpdater.autoInstallOnAppQuit = true;

  // Enable updates in development mode for testing
  if (process.env.ENABLE_DEV_UPDATES === 'true') {
    log.info('Enabling dev updates config');
    autoUpdater.forceDevUpdateConfig = true;
  }

  // Additional debugging for release builds
  if (app.isPackaged) {
    log.info('App is packaged - this is a release build');
    // Try to get more info about the updater configuration
    try {
      log.info(`Auto-updater channel: ${autoUpdater.channel}`);
      log.info(`Auto-updater allowPrerelease: ${autoUpdater.allowPrerelease}`);
      log.info(`Auto-updater allowDowngrade: ${autoUpdater.allowDowngrade}`);
    } catch (e) {
      log.error('Error getting auto-updater properties:', e);
    }
  } else {
    log.info('App is not packaged - this is a development build');
  }

  // Set logger
  autoUpdater.logger = log;

  log.info('Auto-updater setup completed');

  // Check for updates on startup
  setTimeout(() => {
    const currentVersion = autoUpdater.currentVersion?.version || app.getVersion();
    const checkStartTime = Date.now();
    log.info('=== STARTUP UPDATE CHECK INITIATED ===');
    log.info(`Checking for updates on startup at ${new Date().toISOString()}`);
    log.info(`autoUpdater.currentVersion: ${JSON.stringify(autoUpdater.currentVersion)}`);
    log.info(`autoUpdater.getFeedURL(): ${autoUpdater.getFeedURL()}`);
    log.info(
      `Network online status: ${typeof navigator !== 'undefined' ? navigator.onLine : 'unknown'}`
    );

    trackUpdateCheckStarted('startup', currentVersion);

    // Set up a timeout warning for long-running checks
    const timeoutWarning = setTimeout(() => {
      log.warn(
        `Update check still in progress after 30 seconds (started at ${new Date(checkStartTime).toISOString()})`
      );
    }, 30000);

    const timeoutError = setTimeout(() => {
      log.error(
        `Update check appears stuck - no response after 60 seconds (started at ${new Date(checkStartTime).toISOString()})`
      );
    }, 60000);

    autoUpdater
      .checkForUpdates()
      .then((result) => {
        clearTimeout(timeoutWarning);
        clearTimeout(timeoutError);
        const duration = Date.now() - checkStartTime;
        log.info(`=== STARTUP UPDATE CHECK COMPLETED in ${duration}ms ===`);
        log.info('Update check result:', result);
      })
      .catch((err) => {
        clearTimeout(timeoutWarning);
        clearTimeout(timeoutError);
        const duration = Date.now() - checkStartTime;
        log.error(`=== STARTUP UPDATE CHECK FAILED after ${duration}ms ===`);
        log.error('Error checking for updates on startup:', err);
        log.error('Error details:', {
          message: err.message,
          stack: err.stack,
          name: err.name,
          code: 'code' in err ? err.code : undefined,
        });

        // If electron-updater fails, try GitHub API as fallback
        if (
          err.message.includes('HttpError: 404') ||
          err.message.includes('ERR_CONNECTION_REFUSED') ||
          err.message.includes('ENOTFOUND') ||
          err.message.includes('No published versions')
        ) {
          log.info('Using GitHub API fallback for startup update check...');
          log.info('Fallback triggered by error containing:', err.message);
          isUsingGitHubFallback = true;

          githubUpdater
            .checkForUpdates()
            .then(async (result) => {
              if (result.error) {
                trackUpdateCheckCompleted('error', currentVersion, {
                  usingFallback: true,
                  errorType: result.error,
                });
                sendStatusToWindow('error', result.error);
              } else if (result.updateAvailable) {
                // Store GitHub update info
                githubUpdateInfo = {
                  latestVersion: result.latestVersion,
                  downloadUrl: result.downloadUrl,
                  releaseUrl: result.releaseUrl,
                };

                trackUpdateCheckCompleted('available', currentVersion, {
                  latestVersion: result.latestVersion,
                  usingFallback: true,
                });

                updateAvailable = true;
                lastUpdateState = { updateAvailable: true, latestVersion: result.latestVersion };
                updateTrayIcon(true);
                sendStatusToWindow('update-available', { version: result.latestVersion });

                if (!autoDownloadDisabled) {
                  log.info('Auto-downloading update via GitHub fallback on startup...');
                  await githubAutoDownload(result.downloadUrl!, result.latestVersion!, 'on startup');
                } else {
                  log.info('Auto-download disabled — skipping GitHub fallback download on startup');
                }
              } else {
                trackUpdateCheckCompleted('not_available', currentVersion, {
                  latestVersion: result.latestVersion,
                  usingFallback: true,
                });

                updateAvailable = false;
                lastUpdateState = { updateAvailable: false };
                updateTrayIcon(false);
                sendStatusToWindow('update-not-available', {
                  version: autoUpdater.currentVersion.version,
                });
              }
            })
            .catch((fallbackError) => {
              log.error('GitHub fallback also failed on startup:', fallbackError);
              trackUpdateCheckCompleted('error', currentVersion, {
                usingFallback: true,
                errorType: 'github_fallback_failed',
              });
            });
        } else {
          trackUpdateCheckCompleted('error', currentVersion, {
            usingFallback: false,
            errorType: err.message,
          });
        }
      });
  }, 5000); // Wait 5 seconds after app starts

  // Handle update events
  autoUpdater.on('checking-for-update', () => {
    log.info('Auto-updater: Checking for update...');
    log.info(`Auto-updater: Feed URL during check: ${autoUpdater.getFeedURL()}`);
    lastReportedProgress = 0; // Reset progress tracking for new check
    sendStatusToWindow('checking-for-update');
  });

  autoUpdater.on('update-available', (info: UpdateInfo) => {
    log.info('Update available:', info);
    const currentVersion = autoUpdater.currentVersion?.version || app.getVersion();
    trackUpdateCheckCompleted('available', currentVersion, {
      latestVersion: info.version,
      usingFallback: false,
    });
    if (!autoDownloadDisabled) {
      trackUpdateDownloadStarted(info.version, 'electron-updater');
    }
    updateAvailable = true;
    lastUpdateState = { updateAvailable: true, latestVersion: info.version };
    updateTrayIcon(true);
    sendStatusToWindow('update-available', info);
  });

  autoUpdater.on('update-not-available', (info: UpdateInfo) => {
    log.info('Update not available:', info);
    const currentVersion = autoUpdater.currentVersion?.version || app.getVersion();
    trackUpdateCheckCompleted('not_available', currentVersion, {
      latestVersion: info.version,
      usingFallback: false,
    });
    updateAvailable = false;
    lastUpdateState = { updateAvailable: false };
    updateTrayIcon(false);
    sendStatusToWindow('update-not-available', info);
  });

  autoUpdater.on('error', async (err) => {
    log.error('Error in auto-updater:', err);
    log.error('Auto-updater error details:', {
      message: err.message,
      stack: err.stack,
      name: err.name,
      code: 'code' in err ? err.code : undefined,
      toString: err.toString(),
    });

    // Check if this is a 404 error (missing update files) or connection error
    if (
      err.message.includes('HttpError: 404') ||
      err.message.includes('ERR_CONNECTION_REFUSED') ||
      err.message.includes('ENOTFOUND') ||
      err.message.includes('No published versions')
    ) {
      log.info('Falling back to GitHub API for update check...');
      log.info('Fallback triggered by error:', err.message);
      isUsingGitHubFallback = true;

      try {
        const result = await githubUpdater.checkForUpdates();

        if (result.error) {
          sendStatusToWindow('error', result.error);
        } else if (result.updateAvailable) {
          // Store GitHub update info
          githubUpdateInfo = {
            latestVersion: result.latestVersion,
            downloadUrl: result.downloadUrl,
            releaseUrl: result.releaseUrl,
          };

          updateAvailable = true;
          updateTrayIcon(true);
          sendStatusToWindow('update-available', { version: result.latestVersion });

          if (!autoDownloadDisabled) {
            log.info('Auto-downloading update via GitHub fallback after error...');
            await githubAutoDownload(result.downloadUrl!, result.latestVersion!, 'after error');
          } else {
            log.info('Auto-download disabled — skipping GitHub fallback download after error');
          }
        } else {
          updateAvailable = false;
          updateTrayIcon(false);
          sendStatusToWindow('update-not-available', {
            version: autoUpdater.currentVersion.version,
          });
        }
      } catch (fallbackError) {
        log.error('GitHub fallback also failed:', fallbackError);
        sendStatusToWindow(
          'error',
          'Unable to check for updates. Please check your internet connection.'
        );
      }
    } else {
      sendStatusToWindow('error', err.message);
    }
  });

  autoUpdater.on('download-progress', (progressObj) => {
    const roundedPercent = Math.round(progressObj.percent);

    // Only send progress if it increased (prevents backward jumps)
    if (roundedPercent > lastReportedProgress) {
      lastReportedProgress = roundedPercent;
      trackUpdateDownloadProgress(roundedPercent);

      const log_message = `Download: ${roundedPercent}% (${progressObj.transferred}/${progressObj.total}) @ ${Math.round(progressObj.bytesPerSecond / 1024)} KB/s`;
      log.info(log_message);

      sendStatusToWindow('download-progress', {
        ...progressObj,
        percent: roundedPercent,
      });
    }
  });

  autoUpdater.on('update-downloaded', (info: UpdateInfo) => {
    log.info('Update downloaded:', info);
    trackUpdateDownloadCompleted(true, info.version, 'electron-updater');
    sendStatusToWindow('update-downloaded', info);

    // Show native notification
    const notification = new Notification({
      title: 'Update Ready',
      body: `Version ${info.version} will be installed when you quit Goose. Click to install now.`,
    });
    notification.show();

    // Optional: Add click handler to install immediately
    notification.on('click', () => {
      trackUpdateInstallInitiated(info.version, 'electron-updater', 'quit_and_install');
      autoUpdater.quitAndInstall(false, true);
    });
  });
}

interface UpdaterEvent {
  event: string;
  data?: unknown;
}

function sendStatusToWindow(event: string, data?: unknown) {
  const windows = BrowserWindow.getAllWindows();
  windows.forEach((win) => {
    win.webContents.send('updater-event', { event, data } as UpdaterEvent);
  });
}

// centralize GitHub fallback auto-download logic.
async function githubAutoDownload(
  downloadUrl: string,
  latestVersion: string,
  contextLabel = ''
): Promise<void> {
  // Reset progress tracking for new download
  lastReportedProgress = 0;
  trackUpdateDownloadStarted(latestVersion, 'github-fallback');

  try {
    const downloadResult = await githubUpdater.downloadUpdate(
      downloadUrl,
      latestVersion,
      (percent) => {
        // Only send if progress increased (monotonic)
        if (percent > lastReportedProgress) {
          lastReportedProgress = percent;
          trackUpdateDownloadProgress(percent);
          sendStatusToWindow('download-progress', { percent });
        }
      }
    );

    if (downloadResult.success && downloadResult.downloadPath) {
      githubUpdateInfo.downloadPath = downloadResult.downloadPath;
      githubUpdateInfo.extractedPath = downloadResult.extractedPath;
      trackUpdateDownloadCompleted(true, latestVersion, 'github-fallback');
      sendStatusToWindow('update-downloaded', { version: latestVersion });
    } else {
      trackUpdateDownloadCompleted(false, latestVersion, 'github-fallback', downloadResult.error);
      log.error(
        `GitHub auto-download failed${contextLabel ? ` (${contextLabel})` : ''}:`,
        downloadResult.error
      );
    }
  } catch (downloadError) {
    trackUpdateDownloadCompleted(
      false,
      latestVersion,
      'github-fallback',
      errorMessage(downloadError, 'unknown')
    );
    log.error(
      `Error during GitHub auto-download${contextLabel ? ` (${contextLabel})` : ''}:`,
      downloadError
    );
  }
}

function updateTrayIcon(hasUpdate: boolean) {
  if (!trayRef) return;

  if (process.env.GOOSE_VERSION) {
    hasUpdate = false;
  }

  const isDev = !app.isPackaged;
  let iconPath: string;

  if (hasUpdate) {
    // Use icon with update indicator
    if (isDev) {
      iconPath = path.join(process.cwd(), 'src', 'images', 'iconTemplateUpdate.png');
    } else {
      iconPath = path.join(process.resourcesPath, 'images', 'iconTemplateUpdate.png');
    }
    trayRef.setToolTip('Goose - Update Available');
  } else {
    // Use normal icon
    if (isDev) {
      iconPath = path.join(process.cwd(), 'src', 'images', 'iconTemplate.png');
    } else {
      iconPath = path.join(process.resourcesPath, 'images', 'iconTemplate.png');
    }
    trayRef.setToolTip('Goose');
  }

  const icon = nativeImage.createFromPath(iconPath);
  if (process.platform === 'darwin') {
    // Mark as template for macOS to handle dark/light mode
    icon.setTemplateImage(true);
  }
  trayRef.setImage(icon);

  // Update tray menu when icon changes
  updateTrayMenu(hasUpdate);
}

// Function to open settings and scroll to update section
function openUpdateSettings() {
  const windows = BrowserWindow.getAllWindows();
  if (windows.length > 0) {
    const mainWindow = windows[0];
    mainWindow.show();
    mainWindow.focus();
    // Send message to open settings and scroll to update section
    mainWindow.webContents.send('set-view', 'settings', 'update');
  }
}

// Export function to update tray menu
export function updateTrayMenu(hasUpdate: boolean) {
  if (!trayRef) return;

  const menuItems: MenuItemConstructorOptions[] = [];

  // Add update menu item if update is available
  if (hasUpdate) {
    menuItems.push({
      label: 'Update Available...',
      click: openUpdateSettings,
    });
  }

  menuItems.push(
    {
      label: 'Show Window',
      click: async () => {
        const windows = BrowserWindow.getAllWindows();
        if (windows.length === 0) {
          log.info('No windows are open, creating a new one...');
          // Get recent directories for the new window
          const recentDirs = loadRecentDirs();
          const openDir = recentDirs.length > 0 ? recentDirs[0] : null;

          // Emit event to create new window (handled in main.ts)
          ipcMain.emit('create-chat-window', {}, undefined, openDir);
          return;
        }

        // Show all windows with offset
        const initialOffsetX = 30;
        const initialOffsetY = 30;

        windows.forEach((win: BrowserWindow, index: number) => {
          const currentBounds = win.getBounds();
          const newX = currentBounds.x + initialOffsetX * index;
          const newY = currentBounds.y + initialOffsetY * index;

          win.setBounds({
            x: newX,
            y: newY,
            width: currentBounds.width,
            height: currentBounds.height,
          });

          if (!win.isVisible()) {
            win.show();
          }

          win.focus();
        });
      },
    },
    { type: 'separator' },
    { label: 'Quit', click: () => app.quit() }
  );

  const contextMenu = Menu.buildFromTemplate(menuItems);
  trayRef.setContextMenu(contextMenu);
}

// Export functions to manage tray reference
export function setTrayRef(tray: Tray) {
  trayRef = tray;
  // Update icon based on current update status
  updateTrayIcon(updateAvailable);
}

export function getUpdateAvailable(): boolean {
  return updateAvailable;
}

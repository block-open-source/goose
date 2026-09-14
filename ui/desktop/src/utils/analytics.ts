/**
 * Frontend Analytics Module
 *
 * Compatibility layer for existing UI tracking call sites. Frontend telemetry is
 * disabled during the ACP migration, so events are intentionally not sent.
 */

export function setTelemetryEnabled(_enabled: boolean): void {
  // Frontend telemetry is disabled.
}

function sendEvent(
  _eventName: string,
  _properties: Record<string, unknown> = {}
): void {
  // Frontend telemetry is disabled.
}

// ============================================================================
// Event Types
// ============================================================================

/**
 * Frontend-specific analytics events.
 *
 * NOTE: The backend (posthog.rs) already tracks:
 * - session_started (extensions, provider, model, tokens, session count, etc.)
 * - error (provider errors like rate_limit, auth, etc.)
 *
 * Frontend events focus on what the backend can't see:
 * - UI navigation patterns
 * - Onboarding funnel (where users drop off during setup)
 * - Frontend-only crashes (React errors, unhandled rejections)
 */
export type AnalyticsEvent =
  | { name: 'page_view'; properties: { page: string; referrer?: string } }
  | { name: 'onboarding_started'; properties: Record<string, never> }
  | { name: 'onboarding_provider_selected'; properties: { provider?: string; method?: string } }
  | {
      name: 'onboarding_completed';
      properties: { provider: string; model?: string; duration_seconds?: number };
    }
  | { name: 'onboarding_abandoned'; properties: { step: string; duration_seconds?: number } }
  | {
      name: 'onboarding_setup_failed';
      properties: {
        provider: 'local';
        error_message?: string;
      };
    }
  | {
      name: 'error_occurred';
      properties: {
        error_type: string;
        component?: string;
        page?: string;
        action?: string;
        stack_summary?: string;
        recoverable: boolean;
      };
    }
  | { name: 'app_crashed'; properties: { error_type: string; component?: string; page?: string } }
  | { name: 'app_reloaded'; properties: { reason?: string } }
  | { name: 'model_changed'; properties: { provider: string; model: string } }
  | { name: 'settings_tab_viewed'; properties: { tab: string } }
  | { name: 'setting_toggled'; properties: { setting: string; enabled: boolean } }
  | {
      name: 'telemetry_preference_set';
      properties: { enabled: boolean; location: 'settings' | 'onboarding' | 'modal' };
    }
  | {
      name: 'schedule_created';
      properties: {
        source_type: 'file' | 'deeplink' | 'saved';
        success: boolean;
        error_details?: string;
      };
    }
  | { name: 'schedule_deleted'; properties: { success: boolean; error_details?: string } }
  | { name: 'schedule_run_now'; properties: { success: boolean; error_details?: string } }
  | { name: 'recipe_created'; properties: { success: boolean; error_details?: string } }
  | { name: 'recipe_imported'; properties: { success: boolean; error_details?: string } }
  | { name: 'recipe_edited'; properties: { success: boolean; error_details?: string } }
  | { name: 'recipe_deleted'; properties: { success: boolean; error_details?: string } }
  | {
      name: 'recipe_started';
      properties: { success: boolean; error_details?: string; in_new_window?: boolean };
    }
  | { name: 'recipe_deeplink_copied'; properties: { success: boolean; error_details?: string } }
  | { name: 'recipe_yaml_copied'; properties: { success: boolean; error_details?: string } }
  | { name: 'recipe_exported_to_file'; properties: { success: boolean; error_details?: string } }
  | {
      name: 'recipe_scheduled';
      properties: { success: boolean; error_details?: string; action: 'add' | 'edit' | 'remove' };
    }
  | {
      name: 'recipe_slash_command_set';
      properties: { success: boolean; error_details?: string; action: 'add' | 'edit' | 'remove' };
    }
  | {
      name: 'extension_added';
      properties: {
        extension_name?: string;
        is_builtin: boolean;
        success: boolean;
        error_details?: string;
      };
    }
  | {
      name: 'extension_enabled';
      properties: {
        extension_name?: string;
        is_builtin: boolean;
        success: boolean;
        error_details?: string;
      };
    }
  | {
      name: 'extension_disabled';
      properties: {
        extension_name?: string;
        is_builtin: boolean;
        success: boolean;
        error_details?: string;
      };
    }
  | {
      name: 'extension_deleted';
      properties: {
        extension_name?: string;
        is_builtin: boolean;
        success: boolean;
        error_details?: string;
      };
    }
  // Chat input bar features
  | { name: 'input_file_attached'; properties: { file_type: 'file' | 'directory' } }
  | {
      name: 'input_voice_dictation';
      properties: {
        action: 'start' | 'stop' | 'transcribed' | 'error' | 'auto_submit';
        duration_seconds?: number;
        error_type?: string;
      };
    }
  | { name: 'input_mode_changed'; properties: { from_mode: string; to_mode: string } }
  | { name: 'input_diagnostics_opened'; properties: Record<string, never> }
  | { name: 'input_edit_recipe_opened'; properties: Record<string, never> }
  // Auto-update tracking events
  | {
      name: 'update_check_started';
      properties: { trigger: 'startup' | 'manual'; current_version: string };
    }
  | {
      name: 'update_check_completed';
      properties: {
        result: 'available' | 'not_available' | 'error';
        current_version: string;
        latest_version?: string;
        using_fallback: boolean;
        error_type?: string;
      };
    }
  | {
      name: 'update_download_started';
      properties: {
        version: string;
        method: 'electron-updater' | 'github-fallback';
      };
    }
  | {
      name: 'update_download_progress';
      properties: {
        milestone: 25 | 50 | 75 | 100;
        version: string;
        method: 'electron-updater' | 'github-fallback';
      };
    }
  | {
      name: 'update_download_completed';
      properties: {
        success: boolean;
        version: string;
        method: 'electron-updater' | 'github-fallback';
        duration_seconds?: number;
        error_type?: string;
      };
    }
  | {
      name: 'update_install_initiated';
      properties: {
        version: string;
        method: 'electron-updater' | 'github-fallback';
        action: 'quit_and_install' | 'auto_swap_and_relaunch';
      };
    };
// NOTE: slash_command_used is tracked by the backend (posthog.rs) with command_type info

export function trackEvent<T extends AnalyticsEvent>(event: T): void {
  sendEvent(event.name, event.properties);
}

export function trackPageView(page: string, referrer?: string): void {
  trackEvent({
    name: 'page_view',
    properties: { page, referrer },
  });
}

export function trackError(
  errorType: string,
  options: {
    component?: string;
    page?: string;
    action?: string;
    stackSummary?: string;
    recoverable?: boolean;
  } = {}
): void {
  trackEvent({
    name: 'error_occurred',
    properties: {
      error_type: errorType,
      component: options.component,
      page: options.page,
      action: options.action,
      stack_summary: options.stackSummary,
      recoverable: options.recoverable ?? false,
    },
  });
}

export function trackErrorWithContext(
  error: unknown,
  context: {
    component?: string;
    page?: string;
    action?: string;
    recoverable?: boolean;
  } = {}
): void {
  trackError(getErrorType(error), {
    ...context,
    stackSummary: getStackSummary(error),
  });
}

let onboardingStartTime: number | null = null;

export function trackOnboardingStarted(): void {
  onboardingStartTime = Date.now();
  trackEvent({ name: 'onboarding_started', properties: {} });
}

export function trackOnboardingProviderSelected(options: {
  provider?: string;
  method?: string;
}): void {
  trackEvent({
    name: 'onboarding_provider_selected',
    properties: options,
  });
}

export function trackOnboardingCompleted(provider: string, model?: string): void {
  const durationSeconds = onboardingStartTime
    ? Math.round((Date.now() - onboardingStartTime) / 1000)
    : undefined;

  trackEvent({
    name: 'onboarding_completed',
    properties: { provider, model, duration_seconds: durationSeconds },
  });
  onboardingStartTime = null;
}

export function trackOnboardingSetupFailed(
  provider: 'local',
  errorMessage?: string
): void {
  trackEvent({
    name: 'onboarding_setup_failed',
    properties: { provider, error_message: errorMessage },
  });
}

export function trackModelChanged(provider: string, model: string): void {
  trackEvent({
    name: 'model_changed',
    properties: { provider, model },
  });
}

export function trackSettingsTabViewed(tab: string): void {
  trackEvent({
    name: 'settings_tab_viewed',
    properties: { tab },
  });
}

export function trackSettingToggled(setting: string, enabled: boolean): void {
  trackEvent({
    name: 'setting_toggled',
    properties: { setting, enabled },
  });
}

export function trackTelemetryPreference(
  enabled: boolean,
  location: 'settings' | 'onboarding' | 'modal'
): void {
  sendEvent('telemetry_preference_set', { enabled, location });
}

export function getErrorType(error: unknown): string {
  if (error instanceof Error) {
    const name = error.name || 'Error';
    const message = error.message.split('\n')[0].slice(0, 200);
    return `${name}: ${message}`;
  }
  return String(error).slice(0, 200);
}

export function getStackSummary(error: unknown): string | undefined {
  if (!(error instanceof Error) || !error.stack) {
    return undefined;
  }

  // Extract just the function/component names from the stack
  // Skip error message, take top 4 frames
  const lines = error.stack.split('\n').slice(1, 5);
  const frames = lines
    .map((line) => {
      // Match function names like "at ComponentName" or "at Object.functionName"
      const match = line.match(/at\s+([A-Za-z0-9_$.]+)/);
      return match ? match[1] : null;
    })
    .filter(Boolean);

  return frames.length > 0 ? frames.join(' > ') : undefined;
}

// ============================================================================
// Extension Tracking
// ============================================================================

// Only track names for builtin extensions (privacy protection for user-created extensions)
function getTrackableExtensionName(extensionName: string, isBuiltin: boolean): string | undefined {
  return isBuiltin ? extensionName : undefined;
}

export function trackExtensionAdded(
  extensionName: string,
  success: boolean,
  errorDetails?: string,
  isBuiltin: boolean = false
): void {
  trackEvent({
    name: 'extension_added',
    properties: {
      extension_name: getTrackableExtensionName(extensionName, isBuiltin),
      is_builtin: isBuiltin,
      success,
      error_details: errorDetails,
    },
  });
}

export function trackExtensionEnabled(
  extensionName: string,
  success: boolean,
  errorDetails?: string,
  isBuiltin: boolean = false
): void {
  trackEvent({
    name: 'extension_enabled',
    properties: {
      extension_name: getTrackableExtensionName(extensionName, isBuiltin),
      is_builtin: isBuiltin,
      success,
      error_details: errorDetails,
    },
  });
}

export function trackExtensionDisabled(
  extensionName: string,
  success: boolean,
  errorDetails?: string,
  isBuiltin: boolean = false
): void {
  trackEvent({
    name: 'extension_disabled',
    properties: {
      extension_name: getTrackableExtensionName(extensionName, isBuiltin),
      is_builtin: isBuiltin,
      success,
      error_details: errorDetails,
    },
  });
}

export function trackExtensionDeleted(
  extensionName: string,
  success: boolean,
  errorDetails?: string,
  isBuiltin: boolean = false
): void {
  trackEvent({
    name: 'extension_deleted',
    properties: {
      extension_name: getTrackableExtensionName(extensionName, isBuiltin),
      is_builtin: isBuiltin,
      success,
      error_details: errorDetails,
    },
  });
}

// ============================================================================
// Schedule/Recipe Tracking
// ============================================================================

export function trackScheduleCreated(
  sourceType: 'file' | 'deeplink' | 'saved',
  success: boolean,
  errorDetails?: string
): void {
  trackEvent({
    name: 'schedule_created',
    properties: { source_type: sourceType, success, error_details: errorDetails },
  });
}

export function trackScheduleDeleted(success: boolean, errorDetails?: string): void {
  trackEvent({
    name: 'schedule_deleted',
    properties: { success, error_details: errorDetails },
  });
}

export function trackScheduleRunNow(success: boolean, errorDetails?: string): void {
  trackEvent({
    name: 'schedule_run_now',
    properties: { success, error_details: errorDetails },
  });
}

// ============================================================================
// Recipe Tracking
// ============================================================================

export function trackRecipeDeleted(success: boolean, errorDetails?: string): void {
  trackEvent({
    name: 'recipe_deleted',
    properties: { success, error_details: errorDetails },
  });
}

export function trackRecipeStarted(
  success: boolean,
  errorDetails?: string,
  inNewWindow?: boolean
): void {
  trackEvent({
    name: 'recipe_started',
    properties: { success, error_details: errorDetails, in_new_window: inNewWindow },
  });
}

export function trackRecipeDeeplinkCopied(success: boolean, errorDetails?: string): void {
  trackEvent({
    name: 'recipe_deeplink_copied',
    properties: { success, error_details: errorDetails },
  });
}

export function trackRecipeYamlCopied(success: boolean, errorDetails?: string): void {
  trackEvent({
    name: 'recipe_yaml_copied',
    properties: { success, error_details: errorDetails },
  });
}

export function trackRecipeExportedToFile(success: boolean, errorDetails?: string): void {
  trackEvent({
    name: 'recipe_exported_to_file',
    properties: { success, error_details: errorDetails },
  });
}

export function trackRecipeScheduled(
  success: boolean,
  action: 'add' | 'edit' | 'remove',
  errorDetails?: string
): void {
  trackEvent({
    name: 'recipe_scheduled',
    properties: { success, action, error_details: errorDetails },
  });
}

export function trackRecipeSlashCommandSet(
  success: boolean,
  action: 'add' | 'edit' | 'remove',
  errorDetails?: string
): void {
  trackEvent({
    name: 'recipe_slash_command_set',
    properties: { success, action, error_details: errorDetails },
  });
}

// NOTE: slash_command_used is tracked by the backend (posthog.rs) with richer info:
// - command_type: "builtin" | "recipe" | "unknown"
// - command_name: only for builtin commands (e.g., "compact", "summarize")
// - success: true for builtin/recipe, false for unknown

// ============================================================================
// Chat Input Bar Feature Tracking
// ============================================================================

export function trackFileAttached(fileType: 'file' | 'directory'): void {
  trackEvent({
    name: 'input_file_attached',
    properties: { file_type: fileType },
  });
}

export function trackVoiceDictation(
  action: 'start' | 'stop' | 'transcribed' | 'error' | 'auto_submit',
  durationSeconds?: number,
  errorType?: string
): void {
  trackEvent({
    name: 'input_voice_dictation',
    properties: { action, duration_seconds: durationSeconds, error_type: errorType },
  });
}

export function trackDiagnosticsOpened(): void {
  trackEvent({
    name: 'input_diagnostics_opened',
    properties: {},
  });
}

// ============================================================================
// Auto-Update Tracking
// ============================================================================

type UpdateMethod = 'electron-updater' | 'github-fallback';

let updateDownloadStartTime: number | null = null;
let currentUpdateVersion: string | null = null;
let currentUpdateMethod: UpdateMethod | null = null;
let reportedMilestones: Set<25 | 50 | 75 | 100> = new Set();

export function trackUpdateCheckStarted(
  trigger: 'startup' | 'manual',
  currentVersion: string
): void {
  trackEvent({
    name: 'update_check_started',
    properties: { trigger, current_version: currentVersion },
  });
}

export function trackUpdateCheckCompleted(
  result: 'available' | 'not_available' | 'error',
  currentVersion: string,
  options: {
    latestVersion?: string;
    usingFallback: boolean;
    errorType?: string;
  }
): void {
  trackEvent({
    name: 'update_check_completed',
    properties: {
      result,
      current_version: currentVersion,
      latest_version: options.latestVersion,
      using_fallback: options.usingFallback,
      error_type: options.errorType,
    },
  });
}

export function trackUpdateDownloadStarted(version: string, method: UpdateMethod): void {
  updateDownloadStartTime = Date.now();
  currentUpdateVersion = version;
  currentUpdateMethod = method;
  reportedMilestones = new Set();

  trackEvent({
    name: 'update_download_started',
    properties: { version, method },
  });
}

export function trackUpdateDownloadProgress(percent: number): void {
  if (!currentUpdateVersion || !currentUpdateMethod) return;

  const milestones: Array<25 | 50 | 75 | 100> = [25, 50, 75, 100];
  for (const milestone of milestones) {
    if (percent >= milestone && !reportedMilestones.has(milestone)) {
      reportedMilestones.add(milestone);
      trackEvent({
        name: 'update_download_progress',
        properties: {
          milestone,
          version: currentUpdateVersion,
          method: currentUpdateMethod,
        },
      });
    }
  }
}

export function trackUpdateDownloadCompleted(
  success: boolean,
  version: string,
  method: UpdateMethod,
  errorType?: string
): void {
  const durationSeconds = updateDownloadStartTime
    ? Math.round((Date.now() - updateDownloadStartTime) / 1000)
    : undefined;

  trackEvent({
    name: 'update_download_completed',
    properties: {
      success,
      version,
      method,
      duration_seconds: durationSeconds,
      error_type: errorType,
    },
  });

  // Reset state
  updateDownloadStartTime = null;
  currentUpdateVersion = null;
  currentUpdateMethod = null;
  reportedMilestones = new Set();
}

export function trackUpdateInstallInitiated(
  version: string,
  method: UpdateMethod,
  action: 'quit_and_install' | 'auto_swap_and_relaunch'
): void {
  trackEvent({
    name: 'update_install_initiated',
    properties: { version, method, action },
  });
}

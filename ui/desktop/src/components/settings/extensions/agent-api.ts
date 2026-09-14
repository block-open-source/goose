import { toastService } from '../../../toasts';
import type { ExtensionConfig } from '../../../types/extensions';
import { addSessionExtension, removeSessionExtension } from '../../../acp/session-extensions';
import { errorMessage } from '../../../utils/conversionUtils';
import {
  createExtensionRecoverHints,
  formatExtensionErrorMessage,
} from '../../../utils/extensionErrorUtils';

export async function addToAgent(
  extensionConfig: ExtensionConfig,
  sessionId: string,
  showToast: boolean
) {
  const extensionName = extensionConfig.name;
  let toastId = showToast
    ? toastService.loading({
        title: extensionName,
        msg: `adding ${extensionName} extension...`,
      })
    : 0;

  try {
    await addSessionExtension(sessionId, extensionConfig);
    if (showToast) {
      toastService.dismiss(toastId);
      toastService.success({
        title: extensionName,
        msg: `Successfully added extension`,
      });
    }
  } catch (error) {
    if (showToast) {
      toastService.dismiss(toastId);
      const errMsg = errorMessage(error);
      const recoverHints = createExtensionRecoverHints(errMsg);
      const msg = formatExtensionErrorMessage(errMsg, 'Failed to add extension');
      toastService.error({
        title: extensionName,
        msg: msg,
        traceback: errMsg,
        recoverHints,
      });
    }
    throw error;
  }
}

export async function removeFromAgent(
  extensionKey: string,
  extensionName: string,
  sessionId: string,
  showToast: boolean
) {
  let toastId = showToast
    ? toastService.loading({
        title: extensionName,
        msg: `Removing ${extensionName} extension...`,
      })
    : 0;

  try {
    await removeSessionExtension(sessionId, extensionKey);

    if (showToast) {
      toastService.dismiss(toastId);
      toastService.success({
        title: extensionName,
        msg: `Successfully removed extension`,
      });
    }
  } catch (error) {
    if (showToast) {
      toastService.dismiss(toastId);
      const errMsg = errorMessage(error);
      const msg = formatExtensionErrorMessage(errMsg, 'Failed to remove extension');
      toastService.error({
        title: extensionName,
        msg: msg,
        traceback: errMsg,
      });
    }
    throw error;
  }
}

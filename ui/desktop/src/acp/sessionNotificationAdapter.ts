import type { GooseSessionNotification_unstable } from '@aaif/goose-acp-client';
import type { SessionNotification } from '@agentclientprotocol/sdk';
import type { Message } from '../types/message';
import {
  applyElicitationRequest as applyElicitationRequestToState,
  applyElicitationStatus as applyElicitationStatusToState,
  type ElicitationStatus,
} from './adapter/elicitations';
import { applyGooseSessionNotification } from './adapter/gooseSessionNotifications';
import { applyContentChunk, applyThoughtChunk } from './adapter/messages';
import {
  applyPermissionRequest as applyPermissionRequestToState,
  cancelPermissionRequest as cancelPermissionRequestInState,
} from './adapter/permissions';
import {
  type AcpChatStateChange,
  type AdapterState,
  cloneMessage,
  getGooseActiveRunId,
  getGooseQueuedSteer,
} from './adapter/shared';
import { applyToolCall, applyToolCallUpdate } from './adapter/tools';
import type { AcpElicitationRequest } from './elicitationRequests';
import type { AcpPermissionRequest } from './permissionRequestTypes';

export type { AcpChatStateChange } from './adapter/shared';

export interface AcpSessionNotificationAdapter {
  apply(notification: SessionNotification): AcpChatStateChange[];
  applyGoose(notification: GooseSessionNotification_unstable): AcpChatStateChange[];
  applyPermissionRequest(request: AcpPermissionRequest): AcpChatStateChange[];
  cancelPermissionRequest(toolCallId: string, generation: string): AcpChatStateChange[];
  applyElicitationRequest(request: AcpElicitationRequest): AcpChatStateChange[];
  applyElicitationStatus(elicitationId: string, status: ElicitationStatus): AcpChatStateChange[];
  getMessages(): Message[];
}

export function createAcpSessionNotificationAdapter(
  initialMessages: Message[] = [],
  localSteerTextByMessageId: Map<string, string> = new Map()
): AcpSessionNotificationAdapter {
  const state: AdapterState = {
    messages: initialMessages.map(cloneMessage),
    localSteerTextByMessageId: new Map(localSteerTextByMessageId),
    toolCallStatesById: new Map(),
  };

  return {
    apply(notification) {
      return applyAcpSessionNotification(state, notification);
    },
    applyGoose(notification) {
      return applyGooseSessionNotification(state, notification);
    },
    applyPermissionRequest(request) {
      return applyPermissionRequestToState(state, request);
    },
    cancelPermissionRequest(toolCallId, generation) {
      return cancelPermissionRequestInState(state, toolCallId, generation);
    },
    applyElicitationRequest(request) {
      return applyElicitationRequestToState(state, request);
    },
    applyElicitationStatus(elicitationId, status) {
      return applyElicitationStatusToState(state, elicitationId, status);
    },
    getMessages() {
      return state.messages.map(cloneMessage);
    },
  };
}

function applyAcpSessionNotification(
  state: AdapterState,
  notification: SessionNotification
): AcpChatStateChange[] {
  const update = notification.update;

  switch (update.sessionUpdate) {
    case 'user_message_chunk':
      return applyContentChunk(state, 'user', update);
    case 'agent_message_chunk':
      return applyContentChunk(state, 'assistant', update);
    case 'agent_thought_chunk':
      return applyThoughtChunk(state, update);
    case 'tool_call':
      return applyToolCall(state, update);
    case 'tool_call_update':
      return applyToolCallUpdate(state, update);
    case 'session_info_update': {
      const activeRunId = getGooseActiveRunId(update);
      const queuedSteerMessageId = getGooseQueuedSteer(update);
      const changes: AcpChatStateChange[] = [];

      if (update.title || activeRunId !== undefined) {
        changes.push({
          type: 'sessionInfo',
          ...(update.title ? { name: update.title } : {}),
          ...(activeRunId !== undefined ? { activeRunId } : {}),
        });
      }

      if (queuedSteerMessageId) {
        changes.push({ type: 'localSteerConfirmed', messageId: queuedSteerMessageId });
      }

      return changes;
    }
    case 'usage_update':
      return [];
    default:
      return [];
  }
}

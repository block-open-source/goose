import type {
  ContentBlock as AcpContentBlock,
  ToolCall,
  ToolCallUpdate,
} from '@agentclientprotocol/sdk';
import type { Message } from '../../types/message';
import type { ContentBlock as GooseContentBlock } from '../../types/message';
import { findMessageForChunk } from './messages';
import { toolNotificationChange } from './toolNotifications';
import {
  type AcpChatStateChange,
  type AdapterState,
  DEFAULT_VISIBLE_MESSAGE_METADATA,
  type GooseMessageMeta,
  getGooseMessageMeta,
  isRecord,
  messagesChange,
  rawInputToArguments,
  toolIdentity,
  type ToolIdentity,
  type ToolCallState,
} from './shared';

export function applyToolCall(state: AdapterState, update: ToolCall): AcpChatStateChange[] {
  updateToolCallState(state, update);

  const gooseMeta = getGooseMessageMeta(update);
  const message = getOrCreateAssistantMessageForUpdate(state, gooseMeta);

  if (
    message.content.some(
      (content) => content.type === 'toolRequest' && content.id === update.toolCallId
    )
  ) {
    return messagesChange(state);
  }

  const identity = toolIdentity(update);
  const metadata = toolRequestMetadata(update, identity);

  message.content.push({
    type: 'toolRequest',
    id: update.toolCallId,
    toolCall: {
      status: 'success',
      value: {
        name: identity.toolName ?? update.title,
        arguments: rawInputToArguments(update.rawInput),
      },
    },
    ...(metadata ? { metadata } : {}),
    ...(update._meta ? { _meta: update._meta } : {}),
  });

  return messagesChange(state);
}

export function applyToolCallUpdate(
  state: AdapterState,
  update: ToolCallUpdate
): AcpChatStateChange[] {
  const toolCallState = updateToolCallState(state, update);
  const isFinished = toolCallState.status === 'completed' || toolCallState.status === 'failed';

  if (!isFinished) {
    const notificationChange = toolNotificationChange(update);
    return notificationChange ? [notificationChange] : [];
  }

  if (hasToolResponse(state, update.toolCallId)) {
    state.toolCallStatesById.delete(update.toolCallId);
    return messagesChange(state);
  }

  const gooseMeta = getGooseMessageMeta(update);
  const message = getOrCreateToolResponseMessageForUpdate(state, gooseMeta);
  const identity = toolIdentity(update);
  const metadata = toolResponseMetadata(toolCallState, identity);

  message.content.push({
    type: 'toolResponse',
    id: update.toolCallId,
    toolResult: {
      status: 'success',
      value: toolResultValue(
        toolCallState,
        mcpAppMetadata(update),
        toolCallState.status === 'failed'
      ),
    },
    ...(metadata ? { metadata } : {}),
  });

  state.toolCallStatesById.delete(update.toolCallId);
  return messagesChange(state);
}

function updateToolCallState(
  state: AdapterState,
  update: ToolCall | ToolCallUpdate
): ToolCallState {
  const toolCallState = mergeToolCallState(state.toolCallStatesById.get(update.toolCallId), update);
  state.toolCallStatesById.set(update.toolCallId, toolCallState);
  return toolCallState;
}

function mergeToolCallState(
  previous: ToolCallState | undefined,
  update: ToolCall | ToolCallUpdate
): ToolCallState {
  const { _meta: _ignoredMeta, ...toolCallStateUpdate } = update;
  return { ...previous, ...toolCallStateUpdate };
}

function getOrCreateAssistantMessageForUpdate(
  state: AdapterState,
  gooseMeta: GooseMessageMeta
): Message {
  const existing = findMessageForChunk(state, 'assistant', gooseMeta.messageId, gooseMeta.created);
  if (existing) {
    return existing;
  }

  const message: Message = {
    ...(gooseMeta.messageId ? { id: gooseMeta.messageId } : {}),
    role: 'assistant',
    created: gooseMeta.created ?? Math.floor(Date.now() / 1000),
    content: [],
    metadata: { ...DEFAULT_VISIBLE_MESSAGE_METADATA },
  };
  state.messages.push(message);
  return message;
}

function getOrCreateToolResponseMessageForUpdate(
  state: AdapterState,
  gooseMeta: GooseMessageMeta
): Message {
  if (gooseMeta.messageId) {
    const existing = state.messages.find(
      (message) => message.id === gooseMeta.messageId && message.role === 'user'
    );
    if (existing) {
      return existing;
    }
  }

  const message: Message = {
    ...(gooseMeta.messageId ? { id: gooseMeta.messageId } : {}),
    role: 'user',
    created: gooseMeta.created ?? Math.floor(Date.now() / 1000),
    content: [],
    metadata: { ...DEFAULT_VISIBLE_MESSAGE_METADATA },
  };
  state.messages.push(message);
  return message;
}

function hasToolResponse(state: AdapterState, toolCallId: string): boolean {
  return state.messages.some((message) =>
    message.content.some((content) => content.type === 'toolResponse' && content.id === toolCallId)
  );
}

function toolRequestMetadata(
  update: ToolCall,
  identity: ToolIdentity
): Record<string, unknown> | undefined {
  return baseToolMetadata(update, identity);
}

function toolResponseMetadata(
  update: ToolCallUpdate,
  identity: ToolIdentity
): Record<string, unknown> | undefined {
  const metadata = baseToolMetadata(update, identity) ?? {};
  if (update.rawOutput !== undefined) {
    metadata.rawOutput = update.rawOutput;
  }
  if (update.content) {
    metadata.content = update.content;
  }

  return Object.keys(metadata).length > 0 ? metadata : undefined;
}

function baseToolMetadata(
  update: ToolCall | ToolCallUpdate,
  identity: ToolIdentity
): Record<string, unknown> | undefined {
  const metadata: Record<string, unknown> = {};

  if (update.title) {
    metadata.title = update.title;
  }
  if (update.status) {
    metadata.status = update.status;
  }
  if (identity.extensionName) {
    metadata.extensionName = identity.extensionName;
  }
  if (update.kind) {
    metadata.kind = update.kind;
  }
  if (update.locations) {
    metadata.locations = update.locations;
  }

  return Object.keys(metadata).length > 0 ? metadata : undefined;
}

function toolResultValue(
  update: ToolCallUpdate,
  mcpAppMeta: DesktopMcpAppMeta | undefined,
  isError: boolean
): ToolResultValue {
  const content = toolResultContent(update);
  if (isError && content.length === 0) {
    const errorText =
      typeof update.rawOutput === 'string' && update.rawOutput.trim()
        ? update.rawOutput
        : (update.title ?? 'Tool call failed');
    content.push({ type: 'text', text: errorText });
  }

  const toolResult: ToolResultValue = {
    content,
    isError,
    ...(mcpAppMeta ? { _meta: mcpAppMeta } : {}),
  };

  if (update.rawOutput !== undefined) {
    toolResult.structuredContent = update.rawOutput;
  }

  return toolResult;
}

function toolResultContent(update: ToolCallUpdate): GooseContentBlock[] {
  const content: GooseContentBlock[] = [];

  for (const item of update.content ?? []) {
    if (item.type !== 'content') {
      continue;
    }

    const block = apiContentBlockFromAcpContentBlock(item.content);
    if (block) {
      content.push(block);
    }
  }

  if (content.length > 0) {
    return content;
  }

  if (typeof update.rawOutput === 'string') {
    return [{ type: 'text', text: update.rawOutput }];
  }

  return [];
}

function apiContentBlockFromAcpContentBlock(
  content: AcpContentBlock
): GooseContentBlock | undefined {
  switch (content.type) {
    case 'text':
      return {
        type: 'text',
        text: content.text,
        ...(content._meta ? { _meta: content._meta } : {}),
      };
    case 'image':
      return {
        type: 'image',
        data: content.data,
        mimeType: content.mimeType,
        ...(content._meta ? { _meta: content._meta } : {}),
      };
    case 'audio':
      return {
        type: 'audio',
        data: content.data,
        mimeType: content.mimeType,
      };
    case 'resource_link':
      return {
        type: 'resource_link',
        uri: content.uri,
        name: content.name,
        ...(content.description ? { description: content.description } : {}),
        ...(content.mimeType ? { mimeType: content.mimeType } : {}),
        ...(content.size !== undefined && content.size !== null ? { size: content.size } : {}),
        ...(content.title ? { title: content.title } : {}),
        ...(content._meta ? { _meta: content._meta } : {}),
      };
    case 'resource':
      return {
        type: 'resource',
        resource: apiResourceContentsFromAcpResource(content.resource),
        ...(content._meta ? { _meta: content._meta } : {}),
      };
    default:
      return undefined;
  }
}

function apiResourceContentsFromAcpResource(
  resource: Extract<AcpContentBlock, { type: 'resource' }>['resource']
): Extract<GooseContentBlock, { type: 'resource' }>['resource'] {
  if ('text' in resource) {
    return {
      uri: resource.uri,
      text: resource.text,
      ...(resource.mimeType ? { mimeType: resource.mimeType } : {}),
      ...(resource._meta ? { _meta: resource._meta } : {}),
    };
  }

  return {
    uri: resource.uri,
    blob: resource.blob,
    ...(resource.mimeType ? { mimeType: resource.mimeType } : {}),
    ...(resource._meta ? { _meta: resource._meta } : {}),
  };
}

interface DesktopMcpAppMeta extends Record<string, unknown> {
  ui: {
    resourceUri: string;
  };
  extensionName?: string;
  toolName?: string;
  toolNameIsActual?: boolean;
}

type ToolResultValue = {
  content: GooseContentBlock[];
  structuredContent?: unknown;
  isError: boolean;
  _meta?: DesktopMcpAppMeta;
};

function mcpAppMetadata(update: ToolCallUpdate): DesktopMcpAppMeta | undefined {
  if (!isRecord(update._meta)) {
    return undefined;
  }

  const goose = update._meta.goose;
  if (!isRecord(goose) || !isRecord(goose.mcpApp)) {
    return undefined;
  }

  const resourceUri = goose.mcpApp.resourceUri;
  if (typeof resourceUri !== 'string') {
    return undefined;
  }

  return {
    ui: {
      resourceUri,
    },
    extensionName:
      typeof goose.mcpApp.extensionName === 'string' ? goose.mcpApp.extensionName : undefined,
    toolName: typeof goose.mcpApp.toolName === 'string' ? goose.mcpApp.toolName : undefined,
    toolNameIsActual:
      typeof goose.mcpApp.toolNameIsActual === 'boolean'
        ? goose.mcpApp.toolNameIsActual
        : undefined,
  };
}

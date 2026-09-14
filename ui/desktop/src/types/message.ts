import type { TokenState } from './chat';

type JsonObject = Record<string, unknown>;
export type Role = 'user' | 'assistant';

export type Annotations = {
  audience?: Role[];
  lastModified?: string;
  priority?: number;
};

type ContentAnnotations =
  | {
      audience?: Role[];
      lastModified?: string;
      priority?: number;
      _meta?: JsonObject;
    }
  | JsonObject;

export type TextContent = {
  _meta?: JsonObject;
  annotations?: Annotations | JsonObject;
  text: string;
};

export type ImageContent = {
  _meta?: JsonObject;
  annotations?: Annotations | JsonObject;
  data: string;
  mimeType: string;
};

export type ContentBlock =
  | ({ type: 'text' } & RawTextContent)
  | ({ type: 'image' } & RawImageContent)
  | ({ type: 'resource' } & RawEmbeddedResource)
  | ({ type: 'audio' } & RawAudioContent)
  | ({ type: 'resource_link' } & RawResource);

type RawTextContent = {
  _meta?: JsonObject;
  annotations?: ContentAnnotations;
  text: string;
};

type RawImageContent = {
  _meta?: JsonObject;
  annotations?: ContentAnnotations;
  data: string;
  mimeType: string;
};

type RawAudioContent = {
  data: string;
  mimeType: string;
};

type RawEmbeddedResource = {
  _meta?: JsonObject;
  resource: ResourceContents;
};

type RawResource = {
  _meta?: JsonObject;
  description?: string;
  icons?: ContentIcon[];
  mimeType?: string;
  name: string;
  size?: number;
  title?: string;
  uri: string;
};

type ResourceContents =
  | {
      _meta?: JsonObject;
      mimeType?: string;
      text: string;
      uri: string;
    }
  | {
      _meta?: JsonObject;
      blob: string;
      mimeType?: string;
      uri: string;
    };

type ContentIcon = {
  mimeType?: string;
  sizes?: string[];
  src: string;
  theme?: 'light' | 'dark' | JsonObject;
};

export type SystemNotificationType =
  'thinkingMessage' | 'progressMessage' | 'inlineMessage' | 'creditsExhausted';

export type SystemNotificationContent = {
  data?: unknown;
  msg: string;
  notificationType: SystemNotificationType;
};

export type ActionRequired = {
  data: ActionRequiredData;
};

export type ActionRequiredData =
  | {
      actionType: 'toolConfirmation';
      arguments: JsonObject;
      generation?: string;
      id: string;
      prompt?: string | null;
      toolName: string;
    }
  | {
      actionType: 'elicitation';
      id: string;
      message: string;
      requested_schema: unknown;
    }
  | {
      action?: string;
      actionType: 'elicitationResponse';
      id: string;
      user_data: unknown;
    };

export type ThinkingContent = {
  signature: string;
  thinking: string;
};

export type RedactedThinkingContent = {
  data: string;
};

export type ToolConfirmationRequest = {
  arguments: JsonObject;
  id: string;
  prompt?: string | null;
  toolName: string;
};

export type ToolRequest = {
  _meta?: JsonObject;
  id: string;
  metadata?: JsonObject;
  toolCall: JsonObject;
};

export type ToolResponse = {
  id: string;
  metadata?: JsonObject;
  toolResult: JsonObject;
};

export type InferenceMetadata = {
  provider: string;
  requestedModel: string;
  resolvedModel?: string | null;
  providerSessionId?: string | null;
};

/** Mirrors the backend `MessageUsage` schema (camelCase). */
export type MessageUsage = {
  inputTokens?: number | null;
  outputTokens?: number | null;
  totalTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheWriteTokens?: number | null;
  cost?: number | null;
  costSource?: 'provider_reported' | 'estimated' | null;
  elapsedMs?: number | null;
  timeToFirstTokenMs?: number | null;
  isCompaction?: boolean;
};

export type MessageMetadata = {
  agentVisible: boolean;
  fallbackContent?: boolean;
  inference?: InferenceMetadata | null;
  outputTokenLimitReached?: boolean;
  steer?: boolean;
  usage?: MessageUsage | null;
  userVisible: boolean;
};

export type MessageContent =
  | (TextContent & { type: 'text' })
  | (ImageContent & { type: 'image' })
  | (ToolRequest & { type: 'toolRequest' })
  | (ToolResponse & { type: 'toolResponse' })
  | (ToolConfirmationRequest & { type: 'toolConfirmationRequest' })
  | (ActionRequired & { type: 'actionRequired' })
  | (ThinkingContent & { type: 'thinking' })
  | (RedactedThinkingContent & { type: 'redactedThinking' })
  | (SystemNotificationContent & { type: 'systemNotification' });

export type Message = {
  content: MessageContent[];
  created: number;
  id?: string | null;
  metadata: MessageMetadata;
  role: Role;
};

export type Conversation = Message[];

export type MessageEvent =
  | {
      message: Message;
      token_state: TokenState;
      type: 'Message';
    }
  | {
      message_id?: string | null;
      usage: MessageUsage;
      type: 'MessageUsage';
    }
  | {
      error: string;
      type: 'Error';
    }
  | {
      reason: string;
      token_state: TokenState;
      type: 'Finish';
    }
  | {
      message: JsonObject;
      request_id: string;
      type: 'Notification';
    }
  | {
      conversation: Conversation;
      type: 'UpdateConversation';
    }
  | {
      request_ids: string[];
      type: 'ActiveRequests';
    }
  | {
      type: 'Ping';
    };

export type ToolRequestMessageContent = ToolRequest & { type: 'toolRequest' };
export type ToolResponseMessageContent = ToolResponse & { type: 'toolResponse' };
export type ToolConfirmationRequestContent = ToolConfirmationRequest & {
  type: 'toolConfirmationRequest';
};
export type NotificationEvent = Extract<MessageEvent, { type: 'Notification' }>;

export type LiveOutputNotificationParams = {
  sequence: number;
  chunks: LiveOutputNotificationChunk[];
  truncated: boolean;
};

export type LiveOutputNotificationChunk = {
  stream: 'stdout' | 'stderr';
  output: string;
};

export type ImageMessageContent = Extract<Message['content'][number], { type: 'image' }>;

export interface ImageData {
  data: string;
  mimeType: string;
  _meta?: ImageMessageContent['_meta'];
  annotations?: ImageMessageContent['annotations'];
}

export function imageDataFromMessage(message: Message): ImageData[] {
  return message.content
    .filter((c): c is ImageMessageContent => c.type === 'image')
    .map((c) => ({
      data: c.data,
      mimeType: c.mimeType,
      ...(c._meta ? { _meta: c._meta } : {}),
      ...(c.annotations ? { annotations: c.annotations } : {}),
    }));
}

export interface UserInput {
  msg: string;
  images: ImageData[];
}

export function createUserMessage(text: string, images?: ImageData[]): Message {
  const content: Message['content'] = [];

  if (text.trim()) {
    content.push({ type: 'text', text });
  }

  if (images && images.length > 0) {
    images.forEach((img) => {
      content.push({
        type: 'image',
        data: img.data,
        mimeType: img.mimeType,
        ...(img._meta ? { _meta: img._meta } : {}),
        ...(img.annotations ? { annotations: img.annotations } : {}),
      });
    });
  }

  return {
    id: generateMessageId(),
    role: 'user',
    created: Math.floor(Date.now() / 1000),
    content,
    metadata: { userVisible: true, agentVisible: true },
  };
}

export function generateMessageId(): string {
  return Math.random().toString(36).substring(2, 10);
}

export function getTextAndImageContent(message: Message): {
  textContent: string;
  imagePaths: string[];
} {
  let textContent = '';
  const imagePaths: string[] = [];

  for (const content of message.content) {
    if (content.type === 'text') {
      textContent += content.text;
    } else if (content.type === 'image') {
      imagePaths.push(`data:${content.mimeType};base64,${content.data}`);
    }
  }

  // Strip assistant-only markup that shouldn't appear in rendered text
  if (message.role === 'assistant') {
    textContent = stripToolCallMarkers(textContent);
  }

  return { textContent, imagePaths };
}

function stripToolCallMarkers(text: string): string {
  // Remove all tool call XML markers and their content
  return text
    .replace(/<\|tool_calls_section_begin\|>[\s\S]*?<\|tool_calls_section_end\|>/g, '')
    .replace(/<\|tool_call_begin\|>[\s\S]*?<\|tool_call_end\|>/g, '')
    .replace(/<\|tool_call_argument_begin\|>[\s\S]*?<\|tool_call_argument_end\|>/g, '')
    .trim();
}

export function getThinkingContent(message: Message): string | null {
  const parts: string[] = [];

  // Structured thinking content blocks
  for (const content of message.content) {
    if (content.type === 'thinking' && 'thinking' in content && content.thinking) {
      parts.push(content.thinking);
    }
  }

  return parts.length > 0 ? parts.join('') : null;
}

export function getToolRequests(message: Message): (ToolRequest & { type: 'toolRequest' })[] {
  return message.content.filter(
    (content): content is ToolRequest & { type: 'toolRequest' } => content.type === 'toolRequest'
  );
}

export function getToolResponses(message: Message): (ToolResponse & { type: 'toolResponse' })[] {
  return message.content.filter(
    (content): content is ToolResponse & { type: 'toolResponse' } => content.type === 'toolResponse'
  );
}

export function getToolConfirmationContent(
  message: Message
): (ActionRequired & { type: 'actionRequired' }) | undefined {
  return message.content.find(
    (content): content is ActionRequired & { type: 'actionRequired' } =>
      content.type === 'actionRequired' && content.data.actionType === 'toolConfirmation'
  );
}

export function getToolConfirmationRequestContent(
  message: Message
): ToolConfirmationRequestContent | undefined {
  return message.content.find(
    (content): content is ToolConfirmationRequestContent =>
      content.type === 'toolConfirmationRequest'
  );
}

export interface ToolConfirmationData {
  generation?: string;
  id: string;
  toolName: string;
  arguments: Record<string, unknown>;
  prompt?: string | null;
}

export function getAnyToolConfirmationData(message: Message): ToolConfirmationData | undefined {
  const confirmationRequest = getToolConfirmationRequestContent(message);
  if (confirmationRequest) {
    return {
      id: confirmationRequest.id,
      toolName: confirmationRequest.toolName,
      arguments: confirmationRequest.arguments,
      prompt: confirmationRequest.prompt,
    };
  }

  const actionRequired = getToolConfirmationContent(message);
  if (actionRequired && actionRequired.data.actionType === 'toolConfirmation') {
    return {
      generation: actionRequired.data.generation,
      id: actionRequired.data.id,
      toolName: actionRequired.data.toolName,
      arguments: actionRequired.data.arguments,
      prompt: actionRequired.data.prompt,
    };
  }

  return undefined;
}

export function getPendingToolConfirmationIds(messages: Message[]): Set<string> {
  const pendingIds = new Set<string>();
  const respondedIds = new Set<string>();

  for (const message of messages) {
    const responses = getToolResponses(message);
    for (const response of responses) {
      respondedIds.add(response.id);
    }
  }

  for (const message of messages) {
    const confirmationData = getAnyToolConfirmationData(message);
    if (confirmationData && !respondedIds.has(confirmationData.id)) {
      pendingIds.add(confirmationData.id);
    }
  }

  return pendingIds;
}

export function getElicitationContent(
  message: Message
): (ActionRequired & { type: 'actionRequired' }) | undefined {
  return message.content.find(
    (content): content is ActionRequired & { type: 'actionRequired' } =>
      content.type === 'actionRequired' && content.data.actionType === 'elicitation'
  );
}

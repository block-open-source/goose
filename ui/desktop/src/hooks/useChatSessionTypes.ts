import type { ChatState } from '../types/chatState';
import type { TokenState } from '../types/chat';
import type { ImageData, Message, NotificationEvent, UserInput } from '../types/message';
import type { Session } from '../types/session';

export interface UseChatSessionParams {
  sessionId: string;
  onStreamFinish: () => void;
  onSessionLoaded?: () => void;
}

export interface UseChatSessionResult {
  session?: Session;
  messages: Message[];
  chatState: ChatState;
  progressMessage?: string;
  updateSession: (updater: (session: Session) => Session) => void;
  handleSubmit: (input: UserInput) => Promise<void>;
  onSteerQueuedMessage?: (input: UserInput) => Promise<boolean>;
  submitElicitationResponse: (
    elicitationId: string,
    userData: Record<string, unknown>
  ) => Promise<boolean>;
  stopStreaming: () => void;
  retrySessionLoad: () => Promise<void>;
  sessionLoadError?: string;
  tokenState: TokenState;
  notifications: Map<string, NotificationEvent[]>;
  pauseQueueOnStop: boolean;
  queueProcessingBlocked: boolean;
  hasActiveRun: boolean;
  onMessageUpdate: (
    messageId: string,
    newContent: string,
    editType: 'fork' | 'edit',
    retainedImages: ImageData[]
  ) => Promise<void>;
}

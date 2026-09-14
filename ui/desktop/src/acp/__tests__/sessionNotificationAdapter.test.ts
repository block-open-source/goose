import type { GooseSessionNotification_unstable } from '@aaif/goose-acp-client';
import type { RequestPermissionRequest, SessionNotification } from '@agentclientprotocol/sdk';
import { describe, expect, it } from 'vitest';
import { getToolResponses, type Message, type NotificationEvent } from '../../types/message';
import {
  createAcpSessionNotificationAdapter,
  type AcpChatStateChange,
} from '../sessionNotificationAdapter';

const SESSION_ID = 'session-1';
const DEFAULT_TOOL_CALL = {
  sessionUpdate: 'tool_call',
  toolCallId: 'tool-1',
  title: 'Read file',
  status: 'pending',
} as const;
const DEFAULT_TOOL_CALL_UPDATE = {
  sessionUpdate: 'tool_call_update',
  toolCallId: 'tool-1',
} as const;
const OUTPUT_TOKEN_LIMIT_FALLBACK_TEXT =
  'Response stopped because the model reached its output-token limit.';

function acpUpdate(update: SessionNotification['update']): SessionNotification {
  return {
    sessionId: SESSION_ID,
    update,
  };
}

function gooseUpdate(
  update: GooseSessionNotification_unstable['update']
): GooseSessionNotification_unstable {
  return {
    sessionId: SESSION_ID,
    update,
  };
}

function agentText(text: string): SessionNotification {
  return acpUpdate({
    sessionUpdate: 'agent_message_chunk',
    content: { type: 'text', text },
  });
}

function userText(text: string): SessionNotification {
  return acpUpdate({
    sessionUpdate: 'user_message_chunk',
    content: { type: 'text', text },
  });
}

function agentThought(text: string): SessionNotification {
  return acpUpdate({
    sessionUpdate: 'agent_thought_chunk',
    content: { type: 'text', text },
  });
}

function agentImage(data: string, mimeType: string): SessionNotification {
  return acpUpdate({
    sessionUpdate: 'agent_message_chunk',
    content: { type: 'image', data, mimeType },
  });
}

function expectOnlyMessagesChange(chatStateChanges: AcpChatStateChange[]): Message[] {
  expect(chatStateChanges).toHaveLength(1);

  const [chatStateChange] = chatStateChanges;
  expect(chatStateChange.type).toBe('messages');

  if (chatStateChange.type !== 'messages') {
    throw new Error('expected messages state change');
  }

  return chatStateChange.messages;
}

function expectMessagesAndLocalSteerConfirmation(
  chatStateChanges: AcpChatStateChange[],
  messageId: string
): Message[] {
  expect(chatStateChanges).toHaveLength(2);

  const [messagesChange, confirmationChange] = chatStateChanges;
  expect(messagesChange.type).toBe('messages');
  expect(confirmationChange).toEqual({ type: 'localSteerConfirmed', messageId });

  if (messagesChange.type !== 'messages') {
    throw new Error('expected messages state change');
  }

  return messagesChange.messages;
}

function expectOnlyNotificationChange(chatStateChanges: AcpChatStateChange[]): NotificationEvent {
  expect(chatStateChanges).toHaveLength(1);

  const [chatStateChange] = chatStateChanges;
  expect(chatStateChange.type).toBe('notification');

  if (chatStateChange.type !== 'notification') {
    throw new Error('expected notification state change');
  }

  return chatStateChange.notification;
}

function firstContent(message: Message): Message['content'][number] {
  const content = message.content[0];
  expect(content).toBeDefined();
  return content;
}

function permissionRequest(
  sessionId: string,
  toolCallId: string,
  title: string,
  path: string
): RequestPermissionRequest {
  return {
    sessionId,
    options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }],
    toolCall: {
      toolCallId,
      title,
      rawInput: { path },
      content: [{ type: 'content', content: { type: 'text', text: `Allow ${title}?` } }],
    },
  };
}

describe('createAcpSessionNotificationAdapter', () => {
  describe('apply', () => {
    describe('message chunks', () => {
      it('maps and merges text chunks by role', () => {
        const adapter = createAcpSessionNotificationAdapter();

        adapter.apply(agentText('Hello '));

        const secondChunkStateChanges = adapter.apply(agentText('world'));
        let messages = expectOnlyMessagesChange(secondChunkStateChanges);

        expect(messages).toHaveLength(1);
        expect(messages[0].role).toBe('assistant');
        expect(firstContent(messages[0])).toMatchObject({ type: 'text', text: 'Hello world' });

        const userTextStateChanges = adapter.apply(userText('Question'));
        messages = expectOnlyMessagesChange(userTextStateChanges);

        expect(messages).toHaveLength(2);
        expect(messages[1].role).toBe('user');
        expect(firstContent(messages[1])).toMatchObject({ type: 'text', text: 'Question' });
      });

      it('appends repeated adjacent text deltas', () => {
        const adapter = createAcpSessionNotificationAdapter();

        adapter.apply(agentText('Hel'));
        const messages = expectOnlyMessagesChange(adapter.apply(agentText('l')));

        expect(messages).toHaveLength(1);
        expect(firstContent(messages[0])).toMatchObject({ type: 'text', text: 'Hell' });
      });

      it('keeps streamed content visible when an output-limit fallback chunk arrives later', () => {
        const adapter = createAcpSessionNotificationAdapter();

        adapter.apply(
          acpUpdate({
            sessionUpdate: 'agent_message_chunk',
            content: { type: 'text', text: 'Partial response' },
            _meta: { goose: { messageId: 'msg-1' } },
          } as SessionNotification['update'])
        );

        const messages = expectOnlyMessagesChange(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'agent_message_chunk',
              content: { type: 'text', text: OUTPUT_TOKEN_LIMIT_FALLBACK_TEXT },
              _meta: {
                goose: {
                  messageId: 'msg-1',
                  outputTokenLimitReached: true,
                  fallbackContent: true,
                },
              },
            } as SessionNotification['update'])
          )
        );

        expect(messages).toHaveLength(1);
        expect(firstContent(messages[0])).toMatchObject({
          type: 'text',
          text: 'Partial response',
        });
        expect(messages[0].metadata.outputTokenLimitReached).toBe(true);
        expect(messages[0].metadata.fallbackContent).toBeUndefined();
      });

      it('reconciles locally rendered steer text with server chunks', () => {
        const adapter = createAcpSessionNotificationAdapter([
          {
            id: 'steer-1',
            role: 'user',
            created: 123,
            content: [
              { type: 'text', text: 'hello' },
              { type: 'image', data: 'base64-image', mimeType: 'image/png' },
            ],
            metadata: { userVisible: true, agentVisible: true, steer: true },
          },
        ]);

        let messages = expectMessagesAndLocalSteerConfirmation(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'user_message_chunk',
              content: { type: 'text', text: 'hel' },
              _meta: {
                goose: {
                  messageId: 'steer-1',
                  steer: true,
                },
              },
            } as SessionNotification['update'])
          ),
          'steer-1'
        );

        expect(firstContent(messages[0])).toMatchObject({ type: 'text', text: 'hel' });
        expect(messages[0].content[1]).toMatchObject({
          type: 'image',
          data: 'base64-image',
          mimeType: 'image/png',
        });
        expect(messages[0].metadata.steer).toBe(true);

        messages = expectMessagesAndLocalSteerConfirmation(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'user_message_chunk',
              content: { type: 'text', text: 'lo' },
              _meta: {
                goose: {
                  messageId: 'steer-1',
                  steer: true,
                },
              },
            } as SessionNotification['update'])
          ),
          'steer-1'
        );

        expect(firstContent(messages[0])).toMatchObject({ type: 'text', text: 'hello' });

        messages = expectMessagesAndLocalSteerConfirmation(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'user_message_chunk',
              content: { type: 'image', data: 'base64-image', mimeType: 'image/png' },
              _meta: {
                goose: {
                  messageId: 'steer-1',
                  steer: true,
                },
              },
            } as SessionNotification['update'])
          ),
          'steer-1'
        );

        expect(messages[0].content).toEqual([
          { type: 'text', text: 'hello' },
          { type: 'image', data: 'base64-image', mimeType: 'image/png' },
        ]);
      });

      it('appends repeated local steer text deltas without collapsing them', () => {
        const adapter = createAcpSessionNotificationAdapter([
          {
            id: 'steer-1',
            role: 'user',
            created: 123,
            content: [{ type: 'text', text: 'haha' }],
            metadata: { userVisible: true, agentVisible: true, steer: true },
          },
        ]);

        let messages = expectMessagesAndLocalSteerConfirmation(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'user_message_chunk',
              content: { type: 'text', text: 'ha' },
              _meta: {
                goose: {
                  messageId: 'steer-1',
                  steer: true,
                },
              },
            } as SessionNotification['update'])
          ),
          'steer-1'
        );

        expect(firstContent(messages[0])).toMatchObject({ type: 'text', text: 'ha' });

        messages = expectMessagesAndLocalSteerConfirmation(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'user_message_chunk',
              content: { type: 'text', text: 'ha' },
              _meta: {
                goose: {
                  messageId: 'steer-1',
                  steer: true,
                },
              },
            } as SessionNotification['update'])
          ),
          'steer-1'
        );

        expect(firstContent(messages[0])).toMatchObject({ type: 'text', text: 'haha' });
      });

      it('maps image and thinking chunks to existing message content shapes', () => {
        const imageAdapter = createAcpSessionNotificationAdapter();

        const imageStateChanges = imageAdapter.apply(agentImage('base64-image', 'image/png'));
        const imageMessages = expectOnlyMessagesChange(imageStateChanges);

        expect(firstContent(imageMessages[0])).toMatchObject({
          type: 'image',
          data: 'base64-image',
          mimeType: 'image/png',
        });

        const thoughtAdapter = createAcpSessionNotificationAdapter();
        thoughtAdapter.apply(agentThought('Thinking '));

        const thoughtStateChanges = thoughtAdapter.apply(agentThought('more'));
        const thoughtMessages = expectOnlyMessagesChange(thoughtStateChanges);

        expect(thoughtMessages).toHaveLength(1);
        expect(firstContent(thoughtMessages[0])).toMatchObject({
          type: 'thinking',
          thinking: 'Thinking more',
          signature: '',
        });
      });

      it('preserves output-limit metadata when creating a thought message', () => {
        const adapter = createAcpSessionNotificationAdapter();

        const messages = expectOnlyMessagesChange(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'agent_thought_chunk',
              content: { type: 'text', text: 'Truncated thinking' },
              _meta: {
                goose: {
                  messageId: 'thought-1',
                  outputTokenLimitReached: true,
                },
              },
            } as SessionNotification['update'])
          )
        );

        expect(messages).toHaveLength(1);
        expect(messages[0].metadata.outputTokenLimitReached).toBe(true);
      });

      it('preserves output-limit metadata when updating a thought message', () => {
        const adapter = createAcpSessionNotificationAdapter();

        adapter.apply(
          acpUpdate({
            sessionUpdate: 'agent_thought_chunk',
            content: { type: 'text', text: 'Truncated ' },
            _meta: { goose: { messageId: 'thought-1' } },
          } as SessionNotification['update'])
        );

        const messages = expectOnlyMessagesChange(
          adapter.apply(
            acpUpdate({
              sessionUpdate: 'agent_thought_chunk',
              content: { type: 'text', text: 'thinking' },
              _meta: {
                goose: {
                  messageId: 'thought-1',
                  outputTokenLimitReached: true,
                },
              },
            } as SessionNotification['update'])
          )
        );

        expect(messages).toHaveLength(1);
        expect(firstContent(messages[0])).toMatchObject({
          type: 'thinking',
          thinking: 'Truncated thinking',
        });
        expect(messages[0].metadata.outputTokenLimitReached).toBe(true);
      });
    });

    describe('tools', () => {
      it('maps tool calls and successful responses, including MCP app metadata', () => {
        const adapter = createAcpSessionNotificationAdapter();

        const toolCallStateChanges = adapter.apply(
          acpUpdate({
            sessionUpdate: 'tool_call',
            toolCallId: 'tool-1',
            title: 'Read file',
            kind: 'read',
            status: 'in_progress',
            rawInput: { path: 'README.md' },
            locations: [{ path: 'README.md', line: 1 }],
            _meta: {
              goose: {
                toolCall: {
                  extensionName: 'developer',
                  toolName: 'read_file',
                },
              },
            },
          })
        );
        let messages = expectOnlyMessagesChange(toolCallStateChanges);

        expect(messages).toHaveLength(1);
        expect(messages[0].role).toBe('assistant');
        expect(firstContent(messages[0])).toMatchObject({
          type: 'toolRequest',
          id: 'tool-1',
          toolCall: {
            status: 'success',
            value: {
              name: 'read_file',
              arguments: { path: 'README.md' },
            },
          },
          metadata: {
            title: 'Read file',
            status: 'in_progress',
            extensionName: 'developer',
            kind: 'read',
            locations: [{ path: 'README.md', line: 1 }],
          },
        });

        const toolResponseStateChanges = adapter.apply(
          acpUpdate({
            sessionUpdate: 'tool_call_update',
            toolCallId: 'tool-1',
            status: 'completed',
            rawOutput: 'raw result',
            content: [
              {
                type: 'content',
                content: { type: 'text', text: 'rendered result' },
              },
            ],
            _meta: {
              goose: {
                mcpApp: {
                  resourceUri: 'ui://app/resource',
                  extensionName: 'developer',
                  toolName: 'read_file',
                  toolNameIsActual: true,
                },
              },
            },
          })
        );
        messages = expectOnlyMessagesChange(toolResponseStateChanges);

        expect(messages).toHaveLength(2);
        expect(messages[1].role).toBe('user');
        expect(firstContent(messages[1])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          toolResult: {
            status: 'success',
            value: {
              content: [{ type: 'text', text: 'rendered result' }],
              isError: false,
              _meta: {
                ui: { resourceUri: 'ui://app/resource' },
                extensionName: 'developer',
                toolName: 'read_file',
                toolNameIsActual: true,
              },
            },
          },
          metadata: {
            status: 'completed',
            rawOutput: 'raw result',
          },
        });
      });

      it('preserves failed tool responses as results with error status', () => {
        const adapter = createAcpSessionNotificationAdapter();

        const failedToolStateChanges = adapter.apply(
          acpUpdate({
            sessionUpdate: 'tool_call_update',
            toolCallId: 'tool-1',
            status: 'failed',
            title: 'Read file',
            rawOutput: 'permission denied',
          })
        );
        const messages = expectOnlyMessagesChange(failedToolStateChanges);

        expect(messages).toHaveLength(1);
        expect(messages[0].role).toBe('user');
        expect(firstContent(messages[0])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          toolResult: {
            status: 'success',
            value: {
              content: [{ type: 'text', text: 'permission denied' }],
              structuredContent: 'permission denied',
              isError: true,
            },
          },
          metadata: {
            title: 'Read file',
            status: 'failed',
            rawOutput: 'permission denied',
          },
        });
      });

      it('preserves content from an unfinished tool call update', () => {
        const adapter = createAcpSessionNotificationAdapter();
        const result = { type: 'text' as const, text: 'Intermediate result' };
        const content = [{ type: 'content' as const, content: result }];

        adapter.apply(acpUpdate(DEFAULT_TOOL_CALL));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content }));

        const messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }))
        );

        expect(messages).toHaveLength(2);
        expect(firstContent(messages[1])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          metadata: {
            title: 'Read file',
            status: 'completed',
          },
          toolResult: {
            status: 'success',
            value: {
              content: [result],
              isError: false,
            },
          },
        });
      });

      it('replaces earlier content with later content', () => {
        const adapter = createAcpSessionNotificationAdapter();
        const initialResult = { type: 'text' as const, text: 'First result' };
        const replacementResult = { type: 'text' as const, text: 'Second result' };
        const initialContent = [{ type: 'content' as const, content: initialResult }];
        const replacementContent = [{ type: 'content' as const, content: replacementResult }];

        adapter.apply(acpUpdate(DEFAULT_TOOL_CALL));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content: initialContent }));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content: replacementContent }));

        const messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }))
        );

        expect(firstContent(messages[1])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          metadata: {
            content: replacementContent,
          },
          toolResult: {
            status: 'success',
            value: {
              content: [replacementResult],
              isError: false,
            },
          },
        });
      });

      it('clears earlier content with an empty content update', () => {
        const adapter = createAcpSessionNotificationAdapter();
        const result = { type: 'text' as const, text: 'Intermediate result' };
        const content = [{ type: 'content' as const, content: result }];

        adapter.apply(acpUpdate(DEFAULT_TOOL_CALL));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content }));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content: [] }));

        const messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }))
        );

        expect(firstContent(messages[1])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          metadata: {
            content: [],
          },
          toolResult: {
            status: 'success',
            value: {
              content: [],
              isError: false,
            },
          },
        });
      });

      it('preserves unsupported content without rendering it', () => {
        const adapter = createAcpSessionNotificationAdapter();
        const diff = {
          type: 'diff' as const,
          path: '/tmp/file.txt',
          oldText: 'old content',
          newText: 'new content',
        };
        const terminalReference = {
          type: 'terminal' as const,
          terminalId: 'terminal-1',
        };
        const content = [diff, terminalReference];

        adapter.apply(acpUpdate(DEFAULT_TOOL_CALL));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content }));

        const messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }))
        );

        expect(firstContent(messages[1])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          metadata: {
            content,
          },
          toolResult: {
            status: 'success',
            value: {
              content: [],
              isError: false,
            },
          },
        });
      });

      it('uses unfinished content for a failed response when raw output is absent', () => {
        const adapter = createAcpSessionNotificationAdapter();
        const result = { type: 'text' as const, text: 'file not found' };
        const content = [{ type: 'content' as const, content: result }];

        adapter.apply(acpUpdate(DEFAULT_TOOL_CALL));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content }));

        const messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'failed' }))
        );

        expect(messages).toHaveLength(2);
        expect(firstContent(messages[1])).toMatchObject({
          type: 'toolResponse',
          id: 'tool-1',
          metadata: {
            title: 'Read file',
            status: 'failed',
            content,
          },
          toolResult: {
            status: 'success',
            value: {
              content: [{ type: 'text', text: 'file not found' }],
              isError: true,
            },
          },
        });
      });

      it('keeps interleaved tool call state isolated by ID', () => {
        const adapter = createAcpSessionNotificationAdapter();
        const toolOneResult = { type: 'text' as const, text: 'First tool result' };
        const toolTwoResult = { type: 'text' as const, text: 'Second tool result' };
        const toolOneContent = [{ type: 'content' as const, content: toolOneResult }];
        const toolTwoContent = [{ type: 'content' as const, content: toolTwoResult }];

        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL, title: 'First tool' }));
        adapter.apply(
          acpUpdate({ ...DEFAULT_TOOL_CALL, toolCallId: 'tool-2', title: 'Second tool' })
        );
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, content: toolOneContent }));
        adapter.apply(
          acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, toolCallId: 'tool-2', content: toolTwoContent })
        );

        let messages = expectOnlyMessagesChange(
          adapter.apply(
            acpUpdate({
              ...DEFAULT_TOOL_CALL_UPDATE,
              toolCallId: 'tool-2',
              status: 'completed',
            })
          )
        );
        const toolTwoResponse = messages
          .flatMap(getToolResponses)
          .find((response) => response.id === 'tool-2');

        expect(toolTwoResponse).toMatchObject({
          metadata: {
            title: 'Second tool',
            content: toolTwoContent,
          },
          toolResult: {
            status: 'success',
            value: {
              content: [toolTwoResult],
            },
          },
        });

        messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }))
        );
        const toolOneResponse = messages
          .flatMap(getToolResponses)
          .find((response) => response.id === 'tool-1');

        expect(toolOneResponse).toMatchObject({
          metadata: {
            title: 'First tool',
            content: toolOneContent,
          },
          toolResult: {
            status: 'success',
            value: {
              content: [toolOneResult],
            },
          },
        });
      });

      it('does not create a duplicate response for repeated completed updates', () => {
        const adapter = createAcpSessionNotificationAdapter();

        adapter.apply(acpUpdate(DEFAULT_TOOL_CALL));
        adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }));

        const messages = expectOnlyMessagesChange(
          adapter.apply(acpUpdate({ ...DEFAULT_TOOL_CALL_UPDATE, status: 'completed' }))
        );
        const responses = messages
          .flatMap(getToolResponses)
          .filter((response) => response.id === 'tool-1');

        expect(responses).toHaveLength(1);
      });

      it('maps in-progress tool message notifications', () => {
        const adapter = createAcpSessionNotificationAdapter();

        const notificationStateChanges = adapter.apply(
          acpUpdate({
            sessionUpdate: 'tool_call_update',
            toolCallId: 'tool-1',
            status: 'in_progress',
            _meta: {
              toolNotification: {
                type: 'message',
                params: {
                  level: 'info',
                  logger: 'subagent:session-1',
                  data: {
                    text: 'Running search...',
                  },
                },
              },
            },
          })
        );
        const notification = expectOnlyNotificationChange(notificationStateChanges);

        expect(notification).toMatchObject({
          type: 'Notification',
          request_id: 'tool-1',
          message: {
            method: 'notifications/message',
            params: {
              level: 'info',
              logger: 'subagent:session-1',
              data: {
                text: 'Running search...',
              },
            },
          },
        });
      });

      it('maps in-progress tool progress notifications', () => {
        const adapter = createAcpSessionNotificationAdapter();

        const notificationStateChanges = adapter.apply(
          acpUpdate({
            sessionUpdate: 'tool_call_update',
            toolCallId: 'tool-1',
            status: 'in_progress',
            _meta: {
              toolNotification: {
                type: 'progress',
                params: {
                  progressToken: 'scan-repo',
                  progress: 3,
                  total: 10,
                  message: 'Scanned 3 of 10 directories',
                },
              },
            },
          })
        );
        const notification = expectOnlyNotificationChange(notificationStateChanges);

        expect(notification).toMatchObject({
          type: 'Notification',
          request_id: 'tool-1',
          message: {
            method: 'notifications/progress',
            params: {
              progressToken: 'scan-repo',
              progress: 3,
              total: 10,
              message: 'Scanned 3 of 10 directories',
            },
          },
        });
      });
    });
  });

  describe('applyGoose', () => {
    it('maps usage updates into token state', () => {
      const adapter = createAcpSessionNotificationAdapter();

      expect(
        adapter.applyGoose(
          gooseUpdate({
            sessionUpdate: 'usage_update',
            used: 42,
            contextLimit: 200,
            accumulatedInputTokens: 10,
            accumulatedOutputTokens: 15,
            accumulatedCost: 0.12,
          })
        )
      ).toEqual([
        {
          type: 'tokenState',
          tokenState: {
            totalTokens: 42,
            contextLimit: 200,
            accumulatedInputTokens: 10,
            accumulatedOutputTokens: 15,
            accumulatedTotalTokens: 25,
            accumulatedCost: 0.12,
          },
        },
      ]);
    });

    it('maps status messages and keeps later id-less chunks separate', () => {
      const adapter = createAcpSessionNotificationAdapter();

      const noticeStateChanges = adapter.applyGoose(
        gooseUpdate({
          sessionUpdate: 'status_message',
          status: { type: 'notice', message: 'Checking files' },
        })
      );
      let messages = expectOnlyMessagesChange(noticeStateChanges);

      expect(messages).toHaveLength(1);
      expect(messages[0].metadata).toMatchObject({ userVisible: true, agentVisible: false });
      expect(firstContent(messages[0])).toMatchObject({
        type: 'systemNotification',
        notificationType: 'inlineMessage',
        msg: 'Checking files',
      });

      const textStateChanges = adapter.apply(agentText('Result'));
      messages = expectOnlyMessagesChange(textStateChanges);

      expect(messages).toHaveLength(2);
      expect(firstContent(messages[1])).toMatchObject({ type: 'text', text: 'Result' });

      const progressStateChanges = adapter.applyGoose(
        gooseUpdate({
          sessionUpdate: 'status_message',
          status: { type: 'progress', message: 'Still working' },
        })
      );
      expect(progressStateChanges).toEqual([{ type: 'progressMessage', message: 'Still working' }]);
    });
  });

  describe('applyPermissionRequest', () => {
    it('maps permission requests to action-required tool confirmations', () => {
      const adapter = createAcpSessionNotificationAdapter();

      const request: RequestPermissionRequest = {
        sessionId: SESSION_ID,
        options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }],
        toolCall: {
          toolCallId: 'tool-1',
          title: 'Edit file',
          rawInput: { path: 'README.md' },
          content: [
            {
              type: 'content',
              content: { type: 'text', text: 'Allow editing README.md?' },
            },
          ],
          _meta: {
            goose: {
              toolCall: {
                toolName: 'edit_file',
              },
            },
          },
        },
      };

      const permissionStateChanges = adapter.applyPermissionRequest({
        generation: 'permission-generation-1',
        request,
      });
      const messages = expectOnlyMessagesChange(permissionStateChanges);

      expect(messages).toHaveLength(1);
      expect(messages[0].role).toBe('assistant');
      expect(firstContent(messages[0])).toMatchObject({
        type: 'actionRequired',
        data: {
          actionType: 'toolConfirmation',
          generation: 'permission-generation-1',
          id: 'tool-1',
          toolName: 'edit_file',
          arguments: { path: 'README.md' },
          prompt: 'Allow editing README.md?',
        },
      });
    });

    it('replaces reused tool call IDs with the current permission details', () => {
      const adapter = createAcpSessionNotificationAdapter();
      const first = permissionRequest(SESSION_ID, 'tool-1', 'Read file', 'README.md');
      const second = permissionRequest(SESSION_ID, 'tool-1', 'Run command', 'secrets.txt');

      adapter.applyPermissionRequest({ generation: 'generation-a', request: first });
      const messages = expectOnlyMessagesChange(
        adapter.applyPermissionRequest({ generation: 'generation-b', request: second })
      );

      expect(messages).toHaveLength(1);
      expect(firstContent(messages[0])).toMatchObject({
        type: 'actionRequired',
        data: {
          actionType: 'toolConfirmation',
          generation: 'generation-b',
          id: 'tool-1',
          toolName: 'Run command',
          arguments: { path: 'secrets.txt' },
          prompt: 'Allow Run command?',
        },
      });
    });

    it('removes only the matching permission generation when cancelled', () => {
      const adapter = createAcpSessionNotificationAdapter();
      const request = permissionRequest(SESSION_ID, 'tool-1', 'Read file', 'README.md');
      adapter.applyPermissionRequest({ generation: 'generation-a', request });

      expect(adapter.cancelPermissionRequest('tool-1', 'generation-stale')).toEqual([]);
      const messages = expectOnlyMessagesChange(
        adapter.cancelPermissionRequest('tool-1', 'generation-a')
      );

      expect(messages).toEqual([]);
    });
  });

  describe('session_info_update with queuedSteer', () => {
    it('emits localSteerConfirmed when queuedSteer meta is present', () => {
      const adapter = createAcpSessionNotificationAdapter();
      const changes = adapter.apply(
        acpUpdate({
          sessionUpdate: 'session_info_update',
          _meta: {
            goose: {
              queuedSteer: { messageId: 'steer-msg-1', runId: 'run-1' },
            },
          },
        })
      );

      expect(changes).toEqual([{ type: 'localSteerConfirmed', messageId: 'steer-msg-1' }]);
    });

    it('emits both sessionInfo and localSteerConfirmed when both are present', () => {
      const adapter = createAcpSessionNotificationAdapter();
      const changes = adapter.apply(
        acpUpdate({
          sessionUpdate: 'session_info_update',
          title: 'New Title',
          _meta: {
            goose: {
              queuedSteer: { messageId: 'steer-msg-2', runId: 'run-2' },
            },
          },
        })
      );

      expect(changes).toHaveLength(2);
      expect(changes[0]).toEqual({ type: 'sessionInfo', name: 'New Title' });
      expect(changes[1]).toEqual({ type: 'localSteerConfirmed', messageId: 'steer-msg-2' });
    });

    it('returns empty array when session_info_update has no relevant fields', () => {
      const adapter = createAcpSessionNotificationAdapter();
      const changes = adapter.apply(acpUpdate({ sessionUpdate: 'session_info_update' }));
      expect(changes).toEqual([]);
    });
  });
});

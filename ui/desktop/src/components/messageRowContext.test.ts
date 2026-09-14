import { describe, expect, it } from 'vitest';
import type { Message, MessageContent } from '../types/message';
import { deriveMessageRowContexts } from './messageRowContext';

const visibleMetadata: Message['metadata'] = { agentVisible: true, userVisible: true };

function message(
  id: string,
  role: Message['role'],
  content: MessageContent[],
  resolvedModel?: string
): Message {
  return {
    id,
    role,
    created: 1,
    content,
    metadata: {
      ...visibleMetadata,
      ...(resolvedModel
        ? {
            inference: {
              provider: 'test',
              requestedModel: resolvedModel,
              resolvedModel,
            },
          }
        : {}),
    },
  };
}

function toolRequest(id: string): MessageContent {
  return {
    type: 'toolRequest',
    id,
    toolCall: {
      status: 'success',
      value: { name: 'test_tool', arguments: {} },
    },
  };
}

function toolResponse(id: string, value: string): MessageContent {
  return {
    type: 'toolResponse',
    id,
    toolResult: {
      status: 'success',
      value: { content: [{ type: 'text', text: value }], isError: false },
    },
  };
}

function toolConfirmation(id: string): MessageContent {
  return {
    type: 'actionRequired',
    data: {
      actionType: 'toolConfirmation',
      id,
      toolName: 'test_tool',
      arguments: {},
    },
  };
}

describe('deriveMessageRowContexts', () => {
  it('uses the last matching response after the request', () => {
    const messages = [
      message('response-before', 'user', [toolResponse('tool-1', 'before')]),
      message('request', 'assistant', [toolRequest('tool-1')]),
      message('response-after-1', 'user', [toolResponse('tool-1', 'after-1')]),
      message('response-after-2', 'user', [toolResponse('tool-1', 'after-2')]),
    ];

    const contexts = deriveMessageRowContexts(messages);

    expect(contexts[1].toolStates).toHaveLength(1);
    expect(contexts[1].toolStates[0].response).toEqual(toolResponse('tool-1', 'after-2'));
  });

  it('derives confirmation and pending state for each tool request', () => {
    const messages = [
      message('requests', 'assistant', [toolRequest('tool-1'), toolRequest('tool-2')]),
      message('confirmation-1', 'user', [toolConfirmation('tool-1')]),
      message('confirmation-2', 'user', [toolConfirmation('tool-2')]),
      message('response', 'user', [toolResponse('tool-1', 'complete')]),
    ];

    const contexts = deriveMessageRowContexts(messages);

    expect(contexts[0].toolStates).toMatchObject([
      {
        requestId: 'tool-1',
        confirmation: { id: 'tool-1', toolName: 'test_tool', arguments: {} },
        isPending: false,
      },
      {
        requestId: 'tool-2',
        confirmation: { id: 'tool-2', toolName: 'test_tool', arguments: {} },
        isPending: true,
      },
    ]);
    expect(contexts[1].toolConfirmationShownInline).toBe(true);
    expect(contexts[2].toolConfirmationShownInline).toBe(true);
  });

  it('preserves tool-call chain and timestamp behavior', () => {
    const messages = [
      message('tool-1', 'assistant', [
        { type: 'text', text: 'Starting tools.' },
        toolRequest('tool-1'),
      ]),
      message('tool-2', 'assistant', [toolRequest('tool-2')]),
      message('done', 'assistant', [{ type: 'text', text: 'Done.' }]),
    ];

    const contexts = deriveMessageRowContexts(messages);

    expect(contexts[0]).toMatchObject({ isInToolCallChain: true, hideTimestamp: true });
    expect(contexts[1]).toMatchObject({ isInToolCallChain: true, hideTimestamp: false });
    expect(contexts[2]).toMatchObject({ isInToolCallChain: false, hideTimestamp: false });
  });

  it('tracks the previous resolved model for model disclosures', () => {
    const messages = [
      message('model-a', 'assistant', [{ type: 'text', text: 'A' }], 'model-a'),
      message('user', 'user', [{ type: 'text', text: 'Continue' }]),
      message('model-b', 'assistant', [{ type: 'text', text: 'B' }], 'model-b'),
    ];

    const contexts = deriveMessageRowContexts(messages);

    expect(contexts[0].previousResolvedModel).toBeNull();
    expect(contexts[1].previousResolvedModel).toBeNull();
    expect(contexts[2].previousResolvedModel).toBe('model-a');
  });
});

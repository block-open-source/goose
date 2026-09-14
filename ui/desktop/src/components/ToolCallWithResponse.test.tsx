import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../i18n/test-utils';
import type {
  Message,
  NotificationEvent,
  ToolRequestMessageContent,
  ToolResponseMessageContent,
} from '../types/message';
import { getAnyToolConfirmationData } from '../types/message';
import { resolveAcpPermissionRequest } from '../acp/permissionRequests';
import ToolCallWithResponse from './ToolCallWithResponse';

vi.mock('../acp/permissionRequests', () => ({
  resolveAcpPermissionRequest: vi.fn(),
}));

const toolRequest: ToolRequestMessageContent = {
  type: 'toolRequest',
  id: 'tool-1',
  toolCall: {
    status: 'success',
    value: {
      name: 'developer__shell',
      arguments: {
        command: 'build',
      },
    },
  },
};

const liveOutputNotification: NotificationEvent = {
  type: 'Notification',
  request_id: 'tool-1',
  message: {
    method: 'goose/live_output',
    params: {
      sequence: 1,
      chunks: [
        {
          stream: 'stdout',
          output: 'starting\n',
        },
        {
          stream: 'stderr',
          output: 'checking\n',
        },
      ],
      truncated: false,
    },
  },
};

const toolResponse: ToolResponseMessageContent = {
  type: 'toolResponse',
  id: 'tool-1',
  toolResult: {
    status: 'success',
    value: {
      content: [
        {
          type: 'text',
          text: 'final result',
        },
      ],
      isError: false,
    },
  },
};

const failedToolResponse: ToolResponseMessageContent = {
  type: 'toolResponse',
  id: 'tool-1',
  toolResult: {
    status: 'success',
    value: {
      content: [
        {
          type: 'text',
          text: 'this output is important',
        },
      ],
      isError: true,
    },
  },
};

function renderToolCall(response?: ToolResponseMessageContent) {
  return render(
    <ToolCallWithResponse
      isCancelledMessage={false}
      toolRequest={toolRequest}
      toolResponse={response}
      notifications={[liveOutputNotification]}
      isStreamingMessage={!response}
      isPendingApproval={false}
    />,
    { wrapper: IntlTestWrapper }
  );
}

describe('ToolCallWithResponse live output', () => {
  beforeEach(() => {
    vi.mocked(window.electron.getSetting).mockResolvedValue('detailed');
  });

  it('renders raw live output while running and replaces it with the final result', async () => {
    const { rerender } = renderToolCall();

    expect(screen.getByText(/starting/)).toHaveTextContent('starting checking');
    expect(screen.queryByText(/stdout|stderr/)).not.toBeInTheDocument();

    rerender(
      <ToolCallWithResponse
        isCancelledMessage={false}
        toolRequest={toolRequest}
        toolResponse={toolResponse}
        notifications={[liveOutputNotification]}
        isStreamingMessage={false}
        isPendingApproval={false}
      />
    );

    expect(screen.queryByText(/starting/)).not.toBeInTheDocument();
    expect(await screen.findByText('final result')).toBeInTheDocument();
  });

  it('renders output from a failed tool call', async () => {
    renderToolCall(failedToolResponse);

    expect(await screen.findByText('this output is important')).toBeInTheDocument();
  });

  it('passes the current ACP permission generation through inline approval', async () => {
    const permissionMessage: Message = {
      content: [
        {
          type: 'actionRequired',
          data: {
            actionType: 'toolConfirmation',
            arguments: { command: 'build' },
            generation: 'permission-generation-1',
            id: 'tool-1',
            toolName: 'developer__shell',
          },
        },
      ],
      created: 0,
      metadata: { agentVisible: true, userVisible: true },
      role: 'assistant',
    };

    render(
      <ToolCallWithResponse
        sessionId="session-1"
        isCancelledMessage={false}
        toolRequest={toolRequest}
        isPendingApproval
        confirmationContent={getAnyToolConfirmationData(permissionMessage)}
      />,
      { wrapper: IntlTestWrapper }
    );

    await userEvent.click(screen.getByRole('button', { name: 'Allow Once' }));

    expect(resolveAcpPermissionRequest).toHaveBeenCalledWith(
      'session-1',
      'tool-1',
      'permission-generation-1',
      'allow_once'
    );
  });
});

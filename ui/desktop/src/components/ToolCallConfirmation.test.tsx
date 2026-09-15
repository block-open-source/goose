import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../i18n/test-utils';
import type { ActionRequired } from '../types/message';
import ToolCallConfirmation from './ToolCallConfirmation';
import type { ToolApprovalData } from './ToolApprovalButtons';

vi.mock('./ToolApprovalButtons', () => ({
  default: ({ data }: { data: ToolApprovalData }) => (
    <div
      data-testid="approval-buttons"
      data-generation={data.generation}
      data-request-id={data.id}
      data-session-id={data.sessionId}
      data-tool-name={data.toolName}
    />
  ),
}));

const securityPrompt = 'This command sends a local file to a remote service.';

const actionRequiredContent = {
  type: 'actionRequired',
  data: {
    actionType: 'toolConfirmation',
    generation: 'permission-generation-1',
    id: 'request-1',
    toolName: 'developer__shell',
    arguments: {
      command: 'upload /home/alice/private.txt to files.example.test',
    },
    prompt: securityPrompt,
  },
} as ActionRequired & { type: 'actionRequired' };

describe('ToolCallConfirmation', () => {
  describe.each([undefined, securityPrompt])('with prompt %s', (prompt) => {
    it.each([
      'example__read_file',
      'example__READ_FILE',
      'example__Read_File',
      'shell',
      'extension__nested__read_file',
      `extension__${'long_tool_name_'.repeat(20)}READ_FILE`,
    ])('shows the exact identifier %s before approval', (toolName) => {
      render(
        <ToolCallConfirmation
          sessionId="session-1"
          isClicked={false}
          actionRequiredContent={{
            type: 'actionRequired',
            data: {
              actionType: 'toolConfirmation',
              generation: 'permission-generation-1',
              id: 'request-1',
              toolName,
              arguments: {},
              prompt,
            },
          }}
        />,
        { wrapper: IntlTestWrapper }
      );

      expect(
        screen.getByText(
          prompt ? `Allow ${toolName}?` : `Goose would like to call ${toolName}. Allow?`
        )
      ).toBeInTheDocument();
      const buttons = screen.getByTestId('approval-buttons');
      expect(buttons).toHaveAttribute('data-tool-name', toolName);
      expect(buttons).toHaveAttribute('data-request-id', 'request-1');
      expect(buttons).toHaveAttribute('data-generation', 'permission-generation-1');
      expect(buttons).toHaveAttribute('data-session-id', 'session-1');
    });
  });

  it('shows the concrete tool arguments before approval', () => {
    render(
      <ToolCallConfirmation
        sessionId="session-1"
        isClicked={false}
        actionRequiredContent={actionRequiredContent}
      />,
      { wrapper: IntlTestWrapper }
    );

    expect(screen.getByText('command')).toBeInTheDocument();
    expect(screen.getByText(/upload \/home\/alice\/private\.txt/)).toBeInTheDocument();
    expect(screen.getByTestId('approval-buttons')).toHaveAttribute(
      'data-generation',
      'permission-generation-1'
    );
  });

  it('shows the security prompt before approval', () => {
    render(
      <ToolCallConfirmation
        sessionId="session-1"
        isClicked={false}
        actionRequiredContent={actionRequiredContent}
      />,
      { wrapper: IntlTestWrapper }
    );

    expect(screen.getByText(securityPrompt)).toBeInTheDocument();
    expect(screen.getByTestId('approval-buttons')).toBeInTheDocument();
  });
});

import type { ActionRequired } from '../types/message';
import { defineMessages, useIntl } from '../i18n';
import ToolApprovalButtons from './ToolApprovalButtons';
import { ToolCallArguments, type ToolCallArgumentValue } from './ToolCallArguments';

const i18n = defineMessages({
  allowToolCallWithName: {
    id: 'toolConfirmation.allowToolCallWithName',
    defaultMessage: 'Allow {toolName}?',
  },
  gooseWouldLikeToCallWithName: {
    id: 'toolConfirmation.gooseWouldLikeToCallWithName',
    defaultMessage: 'Goose would like to call {toolName}. Allow?',
  },
});

type ToolConfirmationData = Extract<ActionRequired['data'], { actionType: 'toolConfirmation' }>;

interface ToolConfirmationProps {
  sessionId: string;
  isClicked: boolean;
  actionRequiredContent: ActionRequired & { type: 'actionRequired' };
}

export default function ToolConfirmation({
  sessionId,
  isClicked,
  actionRequiredContent,
}: ToolConfirmationProps) {
  const intl = useIntl();
  const data = actionRequiredContent.data as ToolConfirmationData;
  const { generation, id, toolName, arguments: toolArguments, prompt } = data;

  return (
    <div className="goose-message-content bg-background-primary border border-border-primary rounded-2xl overflow-hidden">
      <div className="bg-background-secondary px-4 py-2 text-text-primary break-all">
        {prompt
          ? intl.formatMessage(i18n.allowToolCallWithName, { toolName })
          : intl.formatMessage(i18n.gooseWouldLikeToCallWithName, { toolName })}
      </div>
      <div className="px-4 pb-2">
        {prompt && <div className="py-2 text-sm text-amber-600 dark:text-amber-400">{prompt}</div>}
        <ToolCallArguments args={toolArguments as Record<string, ToolCallArgumentValue>} />
        <ToolApprovalButtons
          data={{ generation, id, toolName, prompt: prompt ?? undefined, sessionId, isClicked }}
        />
      </div>
    </div>
  );
}

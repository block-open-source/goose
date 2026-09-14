import type { AgentMention, AvailableCommand } from '@aaif/goose-acp-client';
import type { DisplayItem } from '../components/MentionPopover';
import { getAcpClient } from './acpConnection';

type SlashCommandItemType = Extract<DisplayItem['itemType'], 'Builtin' | 'Recipe' | 'Skill'>;
type AutocompleteDisplayItem = DisplayItem;

const SLASH_COMMAND_ITEM_TYPES = new Set<string>(['Builtin', 'Recipe', 'Skill']);

function isSlashCommandItemType(value: unknown): value is SlashCommandItemType {
  return typeof value === 'string' && SLASH_COMMAND_ITEM_TYPES.has(value);
}

function stringMetaValue(
  meta: AvailableCommand['_meta'],
  key: string
): string | undefined {
  const value = meta?.[key];
  return typeof value === 'string' && value.trim() ? value : undefined;
}

function cwdParam(cwd: string): { cwd?: string } {
  const trimmed = cwd.trim();
  return trimmed ? { cwd: trimmed } : {};
}

export function availableCommandToDisplayItem(
  command: AvailableCommand
): AutocompleteDisplayItem | null {
  const commandType = stringMetaValue(command._meta, 'commandType');
  if (!isSlashCommandItemType(commandType)) {
    return null;
  }

  const sourcePath = stringMetaValue(command._meta, 'sourcePath');
  const extra = commandType === 'Recipe' ? sourcePath ?? command.description : command.description;

  return {
    name: command.name,
    extra,
    itemType: commandType,
    relativePath: command.name,
  };
}

export function agentMentionToDisplayItem(agent: AgentMention): AutocompleteDisplayItem {
  const mention = agent.mention.trim() || `@${agent.name}`;

  return {
    name: agent.name,
    extra: agent.description,
    itemType: 'Agent',
    relativePath: agent.name,
    insertText: mention.endsWith(' ') ? mention : `${mention} `,
  };
}

export async function listSlashCommandItems(cwd: string): Promise<AutocompleteDisplayItem[]> {
  const client = await getAcpClient();
  const response = await client.goose.slashCommandsList_unstable(cwdParam(cwd));
  return response.availableCommands
    .map(availableCommandToDisplayItem)
    .filter((item): item is AutocompleteDisplayItem => item !== null);
}

export async function listAgentMentionItems(
  cwd: string,
  sessionId?: string
): Promise<AutocompleteDisplayItem[]> {
  const client = await getAcpClient();
  const response = await client.goose.agentMentionsList_unstable({
    ...cwdParam(cwd),
    ...(sessionId ? { sessionId } : {}),
  });
  return response.agents.map(agentMentionToDisplayItem);
}

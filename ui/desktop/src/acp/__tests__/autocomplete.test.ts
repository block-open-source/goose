import type { AgentMention, AvailableCommand } from '@aaif/goose-acp-client';
import { describe, expect, it } from 'vitest';
import { agentMentionToDisplayItem, availableCommandToDisplayItem } from '../autocomplete';

function command(overrides: Partial<AvailableCommand>): AvailableCommand {
  return {
    name: 'release',
    description: 'Run release workflow',
    ...overrides,
  };
}

function agent(overrides: Partial<AgentMention> = {}): AgentMention {
  return {
    name: 'reviewer',
    description: 'Review code changes',
    sourceType: 'agent',
    mention: '@reviewer',
    ...overrides,
  };
}

describe('ACP autocomplete mapping', () => {
  it('maps builtin commands to display items with descriptions', () => {
    expect(
      availableCommandToDisplayItem(
        command({
          _meta: { commandType: 'Builtin' },
        })
      )
    ).toEqual({
      name: 'release',
      extra: 'Run release workflow',
      itemType: 'Builtin',
      relativePath: 'release',
    });
  });

  it('maps skill commands to display items with descriptions', () => {
    expect(
      availableCommandToDisplayItem(
        command({
          _meta: { commandType: 'Skill' },
        })
      )
    ).toEqual({
      name: 'release',
      extra: 'Run release workflow',
      itemType: 'Skill',
      relativePath: 'release',
    });
  });

  it('maps recipe commands and prefers sourcePath for display text', () => {
    expect(
      availableCommandToDisplayItem(
        command({
          _meta: {
            commandType: 'Recipe',
            sourcePath: '/tmp/release.yaml',
          },
        })
      )
    ).toEqual({
      name: 'release',
      extra: '/tmp/release.yaml',
      itemType: 'Recipe',
      relativePath: 'release',
    });
  });

  it('falls back to recipe descriptions when sourcePath is missing', () => {
    expect(
      availableCommandToDisplayItem(
        command({
          _meta: { commandType: 'Recipe' },
        })
      )
    ).toEqual({
      name: 'release',
      extra: 'Run release workflow',
      itemType: 'Recipe',
      relativePath: 'release',
    });
  });

  it('skips commands without a valid commandType', () => {
    expect(availableCommandToDisplayItem(command({}))).toBeNull();
    expect(
      availableCommandToDisplayItem(
        command({
          _meta: { commandType: 'Agent' },
        })
      )
    ).toBeNull();
  });

  it('maps agent mentions and uses server-provided mention text', () => {
    expect(agentMentionToDisplayItem(agent())).toEqual({
      name: 'reviewer',
      extra: 'Review code changes',
      itemType: 'Agent',
      relativePath: 'reviewer',
      insertText: '@reviewer ',
    });
  });

  it('does not add a second trailing space to agent mention text', () => {
    expect(agentMentionToDisplayItem(agent({ mention: '@reviewer ' }))).toMatchObject({
      insertText: '@reviewer ',
    });
  });
});

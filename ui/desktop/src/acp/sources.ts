import type { SourceEntry, SourceType } from '@aaif/goose-acp-client';
import { getAcpClient } from './acpConnection';

const SKILL_SOURCE_TYPES: SourceType[] = ['skill', 'builtinSkill'];
const inFlightSkillSourceLoads = new Map<string, Promise<SourceEntry[]>>();

export async function listSkillSources(projectDir: string): Promise<SourceEntry[]> {
  const inFlightLoad = inFlightSkillSourceLoads.get(projectDir);
  if (inFlightLoad) {
    return inFlightLoad;
  }

  const load = loadSkillSources(projectDir);
  inFlightSkillSourceLoads.set(projectDir, load);

  try {
    return await load;
  } finally {
    if (inFlightSkillSourceLoads.get(projectDir) === load) {
      inFlightSkillSourceLoads.delete(projectDir);
    }
  }
}

async function loadSkillSources(projectDir: string): Promise<SourceEntry[]> {
  const client = await getAcpClient();
  const responses = await Promise.all(
    SKILL_SOURCE_TYPES.map((type) =>
      client.goose.sourcesList_unstable({
        type,
        projectDir,
      })
    )
  );

  return responses
    .flatMap((response) => response.sources)
    .sort(
      (a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }) ||
        a.path.localeCompare(b.path)
    );
}

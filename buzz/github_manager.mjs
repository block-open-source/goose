import { readFileSync } from "node:fs";

export function getProjectIssues(
  runJson,
  { command, projectNumber, projectOwner, projectLimit, repository },
) {
  const normalizedRepository = repository.toLowerCase();
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const project = runJson(command, [
      "project",
      "item-list",
      String(projectNumber),
      "--owner",
      projectOwner,
      "--limit",
      String(projectLimit),
      "--format",
      "json",
    ]);
    if (!Number.isSafeInteger(project.totalCount) || !Array.isArray(project.items)) {
      throw new Error("GitHub returned an invalid project item list.");
    }
    if (project.totalCount > projectLimit) {
      throw new Error(
        `GitHub reports ${project.totalCount} project items. Raise --project-limit.`,
      );
    }
    if (project.items.length === project.totalCount) {
      return {
        project,
        byNumber: new Map(
          project.items
            .filter(
              (item) =>
                item.content?.type === "Issue" &&
                item.content.repository?.toLowerCase() === normalizedRepository,
            )
            .map((item) => [item.content.number, item]),
        ),
      };
    }
    if (attempt === 1) {
      throw new Error(
        `Expected ${project.totalCount} project items but received ${project.items.length}.`,
      );
    }
  }
}

export function getOpenIssues(runJson, { command, repository }) {
  const pages = runJson(command, [
    "api",
    "--paginate",
    "--slurp",
    `repos/${repository}/issues?state=open&per_page=100`,
  ]);
  if (!Array.isArray(pages) || pages.some((page) => !Array.isArray(page))) {
    throw new Error("GitHub returned an invalid paginated issue response.");
  }
  return pages
    .flat()
    .filter((issue) => !issue.pull_request)
    .map((issue) => ({
      number: issue.number,
      title: issue.title,
      url: issue.html_url,
      repository,
      assignees: (issue.assignees || []).map((assignee) => ({
        login: assignee.login,
      })),
    }));
}

export function selectRecentQueueEntries(messages, count, linksFromMessage) {
  const ignored = messages
    .filter((message) => !Number.isSafeInteger(message.created_at))
    .map((message) => ({
      message_id: message.id || null,
      reason: "invalid-created-at",
    }));
  const allEntries = messages
    .filter((message) => Number.isSafeInteger(message.created_at))
    .sort(
      (left, right) =>
        left.created_at - right.created_at ||
        String(left.id || "").localeCompare(String(right.id || "")),
    )
    .flatMap((message) =>
      linksFromMessage(message).map((link) => ({ message, link })),
    );
  const deferredCount = Math.max(0, allEntries.length - count);
  ignored.push(
    ...allEntries.slice(0, deferredCount).map(({ message, link }) => ({
      message_id: message.id,
      link,
      reason: "outside-recent-window",
    })),
  );
  return {
    entries: allEntries.slice(deferredCount),
    ignored,
  };
}

export function readCoreTeam(path) {
  let document;
  try {
    document = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Error(`Could not read core team file ${path}: ${error.message}`);
  }

  if (!Array.isArray(document.owners) || !Array.isArray(document.members)) {
    throw new Error(`${path} must contain owners and members arrays.`);
  }

  const parsedPeople = [
    ...document.owners.map((entry) => person(entry, "owner", path)),
    ...document.members.map((entry) => person(entry, "member", path)),
  ];
  const people = parsedPeople.map(({ bots, ...entry }) => entry);
  if (people.length === 0) {
    throw new Error(`${path} has no people.`);
  }

  const byGithub = new Map();
  const byPubkey = new Map();
  for (const entry of people) {
    const github = entry.github.toLowerCase();
    if (byGithub.has(github)) {
      throw new Error(`More than one core team entry uses ${entry.github}.`);
    }
    if (byPubkey.has(entry.pubkey)) {
      throw new Error(`More than one core team entry uses ${entry.pubkey}.`);
    }
    byGithub.set(github, entry);
    byPubkey.set(entry.pubkey, entry);
  }

  return {
    people,
    owners: people.filter((entry) => entry.role === "owner"),
    members: people.filter((entry) => entry.role === "member"),
    byGithub,
    byPubkey,
    botsByPerson: new Map(
      parsedPeople.map((entry) => [entry.pubkey, entry.bots]),
    ),
  };
}

function person(entry, role, path) {
  if (
    !entry ||
    typeof entry.name !== "string" ||
    !entry.name.trim() ||
    typeof entry.github !== "string" ||
    !entry.github.trim() ||
    typeof entry.pubkey !== "string" ||
    !/^[0-9a-f]{64}$/i.test(entry.pubkey) ||
    typeof entry.capacity !== "number" ||
    !Number.isFinite(entry.capacity) ||
    entry.capacity <= 0 ||
    !Array.isArray(entry.interest) ||
    entry.interest.length === 0 ||
    entry.interest.some(
      (interest) => typeof interest !== "string" || !interest.trim(),
    )
  ) {
    throw new Error(
      `Every person in ${path} must have a name, GitHub handle, hexadecimal ` +
        "pubkey, positive capacity, and non-empty interest list.",
    );
  }

  const bots = entry.bots || {};
  if (
    typeof bots !== "object" ||
    Array.isArray(bots) ||
    Object.entries(bots).some(
      ([name, pubkey]) =>
        !name.trim() ||
        typeof pubkey !== "string" ||
        !/^[0-9a-f]{64}$/i.test(pubkey),
    )
  ) {
    throw new Error(
      `Bots for ${JSON.stringify(entry.name)} in ${path} must map names to hexadecimal pubkeys.`,
    );
  }

  return {
    name: entry.name.trim(),
    github: entry.github.trim(),
    pubkey: entry.pubkey.toLowerCase(),
    role,
    capacity: entry.capacity,
    interest: entry.interest.map((interest) => interest.trim()),
    bots: Object.entries(bots).map(([name, pubkey]) => ({
      name: name.trim(),
      pubkey: pubkey.toLowerCase(),
      role: "bot",
    })),
  };
}

export function issueReferenceFromChannel(channel) {
  const description = [channel.about, channel.description]
    .filter((value) => typeof value === "string" && value)
    .join("\n");
  const url = description.match(
    /https:\/\/github\.com\/([^/\s]+)\/([^/\s]+)\/(issues|pull)\/([1-9]\d*)/i,
  );
  if (url) {
    return {
      repository: `${url[1]}/${url[2]}`,
      number: Number.parseInt(url[4], 10),
      kind: url[3].toLowerCase() === "issues" ? "issue" : "pull-request",
      source: "description",
    };
  }

  const name = channel.name || "";
  const legacy = name.match(/^([^\s]+\/[^\s]+)\s+#([1-9]\d*)(?:\s|$)/);
  if (legacy) {
    return {
      repository: legacy[1],
      number: Number.parseInt(legacy[2], 10),
      kind: null,
      source: "legacy-name",
    };
  }

  const canonical = name.match(/^#?([1-9]\d*)(?:\s|$)/);
  return canonical
    ? {
        repository: null,
        number: Number.parseInt(canonical[1], 10),
        kind: null,
        source: "name",
      }
    : null;
}

export function channelMatchesIssue(channel, issue) {
  const reference = issueReferenceFromChannel(channel);
  if (
    !reference ||
    reference.kind === "pull-request" ||
    reference.number !== issue.number
  ) {
    return false;
  }
  return (
    !reference.repository ||
    reference.repository.toLowerCase() === issue.repository.toLowerCase()
  );
}

export function bestMatchingIssueChannels(channels, issue) {
  const matches = channels
    .filter((channel) => channelMatchesIssue(channel, issue))
    .map((channel) => ({
      channel,
      rank: issueReferenceRank(issueReferenceFromChannel(channel)),
    }));
  const bestRank = Math.max(0, ...matches.map((match) => match.rank));
  return matches
    .filter((match) => match.rank === bestRank)
    .map((match) => match.channel);
}

export function issueReferenceRank(reference) {
  if (reference?.source === "description") {
    return 3;
  }
  if (reference?.source === "legacy-name") {
    return 2;
  }
  return reference ? 1 : 0;
}

export function repositoryFromIssueUrl(url) {
  try {
    const parts = new URL(url).pathname.split("/").filter(Boolean);
    return parts.length >= 4 && parts[2] === "issues"
      ? `${parts[0]}/${parts[1]}`
      : null;
  } catch {
    return null;
  }
}

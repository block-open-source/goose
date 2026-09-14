import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  bestMatchingIssueChannels,
  channelMatchesIssue,
  getOpenIssues,
  getProjectIssues,
  issueReferenceFromChannel,
  readCoreTeam,
  selectRecentQueueEntries,
} from "./github_manager.mjs";

const issue = {
  repository: "aaif-goose/goose",
  number: 123,
};

test("matches current and legacy issue channels", () => {
  assert.equal(
    channelMatchesIssue(
      {
        name: "123 short title",
        description:
          "Discussion for aaif-goose/goose#123: https://github.com/aaif-goose/goose/issues/123",
      },
      issue,
    ),
    true,
  );
  assert.equal(
    channelMatchesIssue({ name: "aaif-goose/goose #123" }, issue),
    true,
  );
  assert.equal(channelMatchesIssue({ name: "#123 title" }, issue), true);
});

test("does not match another repository from an explicit reference", () => {
  assert.equal(
    channelMatchesIssue(
      {
        name: "123 title",
        description: "https://github.com/example/elsewhere/issues/123",
      },
      issue,
    ),
    false,
  );
});

test("parses a legacy channel ending at the issue number", () => {
  assert.deepEqual(
    issueReferenceFromChannel({ name: "aaif-goose/goose #123" }),
    {
      repository: "aaif-goose/goose",
      number: 123,
      kind: null,
      source: "legacy-name",
    },
  );
});

test("prefers an explicit issue channel over a bare numeric name", () => {
  const explicit = {
    channel_id: "explicit",
    name: "123 real issue",
    description: "https://github.com/aaif-goose/goose/issues/123",
  };
  const stray = {
    channel_id: "stray",
    name: "123 followups",
  };
  assert.deepEqual(bestMatchingIssueChannels([stray, explicit], issue), [explicit]);
});

test("does not adopt a pull-request channel", () => {
  assert.equal(
    channelMatchesIssue(
      {
        name: "123 pull request",
        description: "https://github.com/aaif-goose/goose/pull/123",
      },
      issue,
    ),
    false,
  );
});

test("reports malformed and deferred queue entries", () => {
  const { entries, ignored } = selectRecentQueueEntries(
    [
      { id: "invalid", created_at: "today", content: "invalid" },
      { id: "old", created_at: 1, content: "old" },
      { id: "new", created_at: 2, content: "new" },
    ],
    1,
    (message) => [message.content],
  );
  assert.deepEqual(entries.map((entry) => entry.link), ["new"]);
  assert.deepEqual(ignored, [
    { message_id: "invalid", reason: "invalid-created-at" },
    {
      message_id: "old",
      link: "old",
      reason: "outside-recent-window",
    },
  ]);
});

test("retries a project read that changes while being listed", () => {
  let calls = 0;
  const issueItem = {
    content: {
      type: "Issue",
      repository: "aaif-goose/goose",
      number: 123,
    },
  };
  const result = getProjectIssues(
    () => {
      calls += 1;
      return calls === 1
        ? { totalCount: 2, items: [issueItem] }
        : { totalCount: 1, items: [issueItem] };
    },
    {
      command: "gh",
      projectNumber: 1,
      projectOwner: "aaif-goose",
      projectLimit: 1000,
      repository: "aaif-goose/goose",
    },
  );
  assert.equal(calls, 2);
  assert.equal(result.byNumber.get(123), issueItem);
});

test("matches project repository names without case sensitivity", () => {
  const issueItem = {
    content: {
      type: "Issue",
      repository: "AAIF-Goose/Goose",
      number: 123,
    },
  };
  const result = getProjectIssues(
    () => ({ totalCount: 1, items: [issueItem] }),
    {
      command: "gh",
      projectNumber: 1,
      projectOwner: "aaif-goose",
      projectLimit: 1000,
      repository: "aaif-goose/goose",
    },
  );
  assert.equal(result.byNumber.get(123), issueItem);
});

test("normalizes paginated REST issues and excludes pull requests", () => {
  const issues = getOpenIssues(
    () => [
      [
        {
          number: 123,
          title: "Issue",
          html_url: "https://github.com/aaif-goose/goose/issues/123",
          assignees: [{ login: "person" }],
        },
        { number: 124, pull_request: {} },
      ],
    ],
    { command: "gh", repository: "aaif-goose/goose" },
  );
  assert.deepEqual(issues, [
    {
      number: 123,
      title: "Issue",
      url: "https://github.com/aaif-goose/goose/issues/123",
      repository: "aaif-goose/goose",
      assignees: [{ login: "person" }],
    },
  ]);
});

test("uses one complete core-team schema", (context) => {
  const directory = mkdtempSync(join(tmpdir(), "buzz-core-team-"));
  context.after(() => rmSync(directory, { recursive: true }));
  const path = join(directory, "core-team.json");
  const person = {
    name: "Person",
    github: "person",
    pubkey: "1".repeat(64),
    capacity: 1,
    interest: ["testing"],
    bots: { Bot: "2".repeat(64) },
  };
  writeFileSync(
    path,
    JSON.stringify({ owners: [person], members: [] }),
  );

  const team = readCoreTeam(path);
  assert.equal(team.people.length, 1);
  assert.equal(team.botsByPerson.get(person.pubkey).length, 1);

  delete person.capacity;
  writeFileSync(
    path,
    JSON.stringify({ owners: [person], members: [] }),
  );
  assert.throws(() => readCoreTeam(path), /positive capacity/);
});

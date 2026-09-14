# Goose Buzz automation

These tools connect issues in `aaif-goose/goose` to the public Goose Buzz
community at `buzz.gdk.so`.

The setup uses two separate identities:

- **Github Manager** is a service identity. The scripts use its Nostr key to
  create and manage issue channels, add members, post issue summaries, and sync
  channel topics.
- **A managed bot**, currently **Doose**, is an agent identity run by Buzz
  Desktop. It can review an issue when Github Manager explicitly mentions it.

Github Manager owns the channels it creates. The checked-in core team is also
added so people can find and manage them in Buzz Desktop.

## Requirements

- Node.js
- [GitHub CLI](https://cli.github.com/) authenticated with access to the
  repository and its project board; the recipe also needs permission to assign
  issues
- Buzz Desktop on macOS, or a `buzz` CLI on `PATH`
- Goose CLI with a configured model provider
- A running public Buzz community

The scripts use `https://buzz.gdk.so` by default. Set `BUZZ_RELAY_URL` to target
another community. Set `BUZZ_BIN` or `GH_BIN` to override CLI discovery.

The community and issue channels are public. Confirm that a new Nostr identity
can join the community before relying on these scripts; relay configuration
names have changed between Buzz releases, so use the controls provided by the
deployed version rather than copying old allowlist environment variables.

## Set up a new installation

### 1. Create Github Manager

Run:

```sh
./buzz/create_github_manager
```

The script:

1. Generates a dedicated Nostr key pair.
2. Stores the keys outside the repository.
3. Uploads `assets/GithubManager.png`.
4. Publishes a Buzz profile named `Github Manager`.

The identity is stored in:

```text
$GOOSE_BUZZ_HOME/github-manager
```

`GOOSE_BUZZ_HOME` defaults to `$XDG_CONFIG_HOME/goose/buzz`, or
`~/.config/goose/buzz` when `XDG_CONFIG_HOME` is not set. The identity directory
is mode `0700`; its files are mode `0600`.

The important files are:

```text
private-key.nsec   Secret key used by the scripts
public-key.npub    Shareable Nostr public key
public-key.hex     Shareable public key used by the Buzz CLI
profile-created   Time at which the profile was published
```

The command is deliberately idempotent. If all three key files and the profile
marker exist, it fixes their permissions and changes nothing. If the keys exist
without the marker, it publishes the profile again without replacing the
identity. If only part of the key pair exists, it stops rather than silently
replacing the identity.

Back up the entire `github-manager` directory in a secure password or secrets
manager. The private key cannot be recovered from Buzz. Anyone with it can act
as Github Manager.

### 2. Create and run a bot

Create at least one managed agent in Buzz Desktop. The bot must have its own
Nostr identity; do not reuse the Github Manager key. Start the agent and make
sure it shows as online.

The channel script adds the bot to new channels, but membership alone does not
let Github Manager instruct it. Buzz agents default to accepting instructions
only from their human owner.

### 3. Allow Github Manager to instruct the bot

In Buzz Desktop:

1. Open **Agents**.
2. Select the bot, such as **Doose**.
3. Choose **Edit agent**.
4. Expand **Advanced**.
5. Change **Who can send instructions** to **Selected people**.
6. Search for **Github Manager** and choose **Add**.
7. Save the agent and restart it if Buzz does not restart it automatically.

This permission is security-sensitive. A selected identity can instruct the
agent to use the computer, files, accounts, and connected tools available to
that agent. Keep the list narrow. Do not choose **Anyone** just because the
community itself is public.

To verify the permission, send a message as Github Manager with both readable
mention text and the bot's public key:

```sh
export BUZZ_PRIVATE_KEY="$(cat ~/.config/goose/buzz/github-manager/private-key.nsec)"
export BUZZ_RELAY_URL=https://buzz.gdk.so
BUZZ_CLI="${BUZZ_BIN:-/Applications/Buzz.app/Contents/MacOS/buzz}"

"$BUZZ_CLI" messages send \
  --channel <channel-uuid> \
  --content "@Doose reply with OK" \
  --mention <doose-public-key-hex>
```

A literal `@Doose` without the `--mention` public key may be rendered as text
without waking the remote agent. The successful command returns the bot key in
`mention_pubkeys`. The bot may take a minute or two to respond.

### 4. Configure the core team

`core-team.json` defines the people added to every new issue channel. Each
person has a display name, GitHub handle, stable Buzz public key, and an
`interest` list of topics they know about, based on their typical public issue
and pull-request work. `capacity` sets that person's target share of new
assignments; most people are `1` and Filip is `0.5`. A person can also have a
`bots` mapping from bot display name to bot public key. Adding that person adds
those identities with the Buzz `bot` role.

The current roster is:

- Douwe Osinga as an owner
    - Doose as their bot
- Alex Hancock as a member
- filip as a member
- jasper as a member
- Mic as a member
- lifei as a member
    - Lifei goose agent as their bot
- Jack Amadeo as a member

Edit the checked-in file when the permanent team changes. Set
`BUZZ_CORE_TEAM_FILE` to use a different roster file for another installation.
Use `--no-core-team` for a one-off channel that should not receive the standard
people. If a person from the file is then supplied explicitly with `--owner` or
`--person`, their associated bots are still included.

## Tools

### `create_github_manager`

Creates the Github Manager key pair and profile as described above. It has no
arguments and never rotates an existing identity.

```sh
./buzz/create_github_manager
```

### `create_issue_channel`

Reads a GitHub issue, creates a permanent open stream, sets its initial topic to
`⚪ GitHub phase: Inbox`, adds owners, people, and bots, and posts the supplied
summary with a link to the issue. It does not modify the GitHub issue.

```sh
./buzz/create_issue_channel 12345 \
  --summary "Why the issue matters and what needs to be decided." \
  --person "Issue Reporter"
```

The owners and members in `core-team.json` and their associated bots are added
by default. Repeat `--owner`, `--person`, or `--bot` to add more identities.
Each command-line value can be an exact Buzz display name or a 64-character
hexadecimal public key. People receive the `member` role and bots receive the
`bot` role.

Use `--repo owner/repo` for a repository other than `aaif-goose/goose`. A full
GitHub issue URL also works. For a multiline summary, use `--summary-file path`
or pipe the text with `--summary-file -`.

The command prints the issue, channel, message, and participant details as JSON.
Channel names use `#<issue-number> <issue-title>` in the Buzz UI. The script
refuses to create a duplicate when an active or archived channel already
matches the issue number. For an existing channel, explicitly supplied owners,
people, and bots are added without posting the summary a second time. An
archived matching channel is unarchived before its roster is updated. If setup
of a new channel fails after creation, the incomplete channel is deleted so the
next run can retry it.

Adding a bot does not trigger it. After the channel is created, Github Manager
must send a new message with an explicit bot mention:

```sh
"${BUZZ_BIN:-/Applications/Buzz.app/Contents/MacOS/buzz}" messages send \
  --channel <channel-uuid> \
  --content "@Doose review this issue and post your assessment in this channel." \
  --mention <doose-public-key-hex>
```

### `list_issue_work`

Lists issues in the project's Inbox that need an owner or channel, and open
issues linked from the Buzz `issues to add` channel. An assigned Inbox issue
without a channel is included so a failed channel creation can be retried. A
queue entry can contain a Goose issue or pull request URL, or just `#<issue
number>`. Queue entries with an existing issue channel are treated as processed.
Pull request links resolve to their issue when GitHub reports exactly one
closing issue. The JSON also includes the core team, GitHub handles, Buzz public
keys, interests, and assignment capacity so a Goose recipe can select an owner.
`recent_assignment_load` counts core-team assignees across the 100 most recently
created issues, including closed issues. Phase and issue age do not affect the
count. It does not change GitHub or Buzz.

```sh
./buzz/list_issue_work
```

The project defaults to `aaif-goose` project 1. Use `--repo`,
`--project-owner`, `--project-number`, and `--queue-channel` for another
installation. The command fails instead of returning a partial list when the
project, channel, or message limits are reached. Queue links that cannot be
resolved are reported separately instead of guessed. A new installation must
create one Buzz channel with the configured queue name before running the
recipe.

Only the 20 most recent issue or pull-request links in the queue are considered.
Conversation without GitHub links does not consume that limit. Use
`--queue-count` to change it. Older links are reported with
`outside-recent-window`; messages without a valid timestamp are reported with
`invalid-created-at`.

Queue authors are only returned as `queue_requesters` when their public key is
present in `core-team.json`. Other authors are reported as
`ignored_queue_requesters` and cannot influence channel membership.

### `syncissues`

Fetches all open GitHub issues and all Buzz channels, matches issue channels by
the GitHub URL in their description, and synchronizes the project phase and
assignees to the channel topic. Active and archived channels use the same
matching rules. Legacy number-only channels are checked against GitHub so pull
request channels are reported and skipped instead of being treated as closed
issues. When more than one name points at an issue, an explicit GitHub issue URL
in the channel description wins over a legacy or bare numeric name:

| GitHub phase | Buzz topic marker |
| --- | --- |
| Inbox | ⚪ |
| Needs info | 🟡 |
| Accepted / design | 🟣 |
| Ready | 🟢 |
| Verification | 🔵 |
| Done | ✅ |

Topics use this format:

```text
🟢 Ready -- assigned to: @github-handle
```

Multiple assignees are comma-separated. Issues without an assignee say
`assigned to: unassigned`.

Issue channels without a corresponding open GitHub issue get the topic
`✅ GitHub issue: Closed` and are archived. If an issue reopens, its channel is
unarchived even when it has no project status; its project phase is restored
when available. Other Buzz channels are ignored.

The script also checks for new replies from people outside the repository.
GitHub authors associated as `OWNER`, `MEMBER`, or `COLLABORATOR` are considered
internal; other human users are considered outsiders. For each new outsider
reply on an open issue with a matching channel, Github Manager mentions the
issue's core-team assignee and posts the author name and a link to the GitHub
comment. The comment body is deliberately not copied into Buzz, so comment text
cannot inject mentions or bot instructions.

The `Snoozed until` date comes from the GitHub project. Once that date arrives,
Github Manager posts `@owner, the snooze is expired.` to the matching channel.
Each issue is notified once per snooze date. Changing the date permits a new
notification when the new date arrives.

The first non-dry run initializes a repository-specific cursor without posting
historical replies. Later successful runs advance it. The cursor is stored next
to the Github Manager key as
`issue-comment-sync-<owner>-<repository>.json`, with mode `0600`. A dry run reads
the cursor and reports `would-notify` actions without posting messages or
advancing it.

Run the local Buzz checks with `just test-buzz`. The same checks run in GitHub
Actions when the Buzz automation changes.

Snooze notifications have a separate
`issue-snooze-sync-<owner>-<repository>.json` state file in the same directory.

Always inspect a dry run first:

```sh
./buzz/syncissues --dry-run
./buzz/syncissues
```

The script checks that neither the GitHub issue list nor the Buzz channel list
was truncated before changing channels. Raise `--limit` if it stops at the Buzz
limit. Use `--repo`, `--project`, `--project-owner`, or `--project-number` to
target another repository or project.

### `github_issue_manager.yaml`

This Goose recipe manages the full Inbox loop:

1. List unassigned Inbox issues and unresolved work from `issues to add`.
2. Read each issue and rank the three strongest matches based on `interest`.
3. Choose the least loaded of those three using assignments on the 100 newest
   issues, adjusting only for the person's configured capacity.
4. Assign the issue to that person's GitHub handle.
5. Create a focused Buzz channel, or promote the owner when the channel already
   exists. Start with Douwe and the issue owner, then add relevant people until
   the channel has at least three distinct humans. If Douwe owns the issue, two
   others are required. A fourth person may be added when useful.
6. Run `syncissues`.

Issue bodies and comments are treated as untrusted data. The recipe is allowed
to assign issues but is instructed not to edit bodies, post GitHub comments,
change project fields, labels, or issue state.

Run it once with:

```sh
goose run \
  --recipe "$PWD/buzz/github_issue_manager.yaml" \
  --params "automation_dir=$PWD/buzz" \
  --no-session
```

Preview assignments without changing GitHub or Buzz:

```sh
goose run \
  --recipe "$PWD/buzz/github_issue_manager.yaml" \
  --params "automation_dir=$PWD/buzz" \
  --params "dry_run=true" \
  --no-session
```

### `run_hourly`

Runs the recipe, waits an hour after it finishes, and repeats. Override the wait
with `BUZZ_MANAGER_INTERVAL_SECONDS`.

```sh
./buzz/run_hourly
```

The dedicated machine must stay awake and have working Goose, `gh`, and Buzz
CLI configuration. This runner does not require Goose's scheduler.

## Move or recover the setup

To move Github Manager to another machine, copy its complete identity directory
over a trusted encrypted connection. This moves the manager key and the sync
cursors without copying a human or bot identity:

```text
~/.config/goose/buzz/github-manager
```

On the destination, preserve the directory as `0700` and its files as `0600`,
then run `create_github_manager` to confirm that it finds the identity. Also
install and authenticate `gh`, install and configure Goose, and install Buzz
Desktop or configure `BUZZ_BIN`.

Do not copy Buzz Desktop's application-data directory. The hourly workflow only
needs Github Manager's key. Doose can keep running on the original machine, or
a separate bot identity can be created on the dedicated machine later.

Running `create_github_manager` with an empty identity directory creates a new
identity, not a replacement copy of the old one. If the old private key is lost:

1. Generate a new Github Manager identity.
2. Add the new identity as an owner of channels that must remain manageable.
3. Replace the old Github Manager entry in every bot's **Selected people** list.
4. Remove the old identity where possible.
5. Store a secure backup of the new key.

Existing channel events remain signed by the old identity; they cannot be
re-signed by the new one.

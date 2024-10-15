# Goose CLI Commands

Goose provides a command-line interface (CLI) with various commands to manage sessions, toolkits, and more. Below is a list of the available commands and their descriptions:

## Goose CLI

### `version`

**Usage:**
```sh
  goose version
```

Lists the version of Goose and any associated plugins.

### `session`

#### `start`

**Usage:**
```sh
  goose session start [--profile PROFILE] [--plan PLAN] [--log-level [DEBUG|INFO|WARNING|ERROR|CRITICAL]] [--tracing]
```

Starts a new Goose session.

If you want to enable locally hosted Langfuse tracing, pass the --tracing flag after starting your local Langfuse server as outlined in the [Contributing Guide's][contributing] Development guidelines.

#### `resume`

**Usage:**
```sh
  goose session resume [NAME] [--profile PROFILE]
```

Resumes an existing Goose session.

#### `list`

**Usage:**
```sh
  goose session list
```

Lists all Goose sessions.

#### `clear`

**Usage:**
```sh
  goose session clear [--keep KEEP]
```

Deletes old Goose sessions, keeping the most recent ones as specified by the `--keep` option.

### `toolkit`

#### `list`

**Usage:**
```sh
  goose toolkit list
```

Lists all available toolkits with their descriptions.

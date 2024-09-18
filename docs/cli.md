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
  goose session start [--profile PROFILE] [--plan PLAN]
```

Starts a new Goose session.

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

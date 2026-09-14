# @aaif/goose-acp

Install and resolve the Goose executable through npm.

This package distributes the Goose CLI using platform-specific optional npm
dependencies. It does not contain or depend on the Goose ACP client.

## Installation

```bash
npm install @aaif/goose-acp
```

The matching `@aaif/goose-binary-*` package is installed automatically. Do not
install a platform package directly; `@aaif/goose-acp` provides the supported
`goose` command.

## Usage

Run the Goose CLI installed by the package:

```bash
npx goose acp
npx goose serve
```

The launcher forwards arguments and standard input, output, and error streams to
the native executable. It preserves the executable's exit status and forwards
termination signals.

Resolve the executable path programmatically:

```typescript
import { resolveGooseBinary } from "@aaif/goose-acp";

const binaryPath = resolveGooseBinary();
```

`resolveGooseBinary()` first uses `GOOSE_BINARY` when it is set. Otherwise, it
selects the package matching `process.platform` and `process.arch`. In both
cases it verifies that the executable exists and returns an absolute path.

Use the override to run a locally built or custom Goose executable:

```bash
GOOSE_BINARY=/path/to/goose npx goose acp
```

`GOOSE_BINARY` must point directly to a native Goose executable, not a
`node_modules/.bin/goose` command shim.

Supported platforms:

| Operating system | Architecture |
| ---------------- | ------------ |
| macOS            | ARM64        |
| macOS            | x64          |
| Linux            | ARM64        |
| Linux            | x64          |
| Windows          | x64          |

Package managers must install optional dependencies. If optional dependencies
are disabled, the resolver reports which platform package is missing.

# @aaif/goose-acp-client

TypeScript client library for communicating with Goose over an existing Agent
Client Protocol (ACP) transport.

This package provides:

- TypeScript types and Zod validators for Goose ACP extension methods
- `GooseExtClient` for calling Goose extension methods
- Client capability definitions and MCP Apps helpers

It does not install, resolve, or start the Goose executable. Applications own
the transport and process lifecycle.

## Installation

```bash
npm install @aaif/goose-acp-client @agentclientprotocol/sdk
```

## Usage

Compose the Goose extension client with the standard ACP SDK:

```typescript
import {
  client as createAcpClient,
  methods,
  PROTOCOL_VERSION,
  type Stream,
} from "@agentclientprotocol/sdk";
import { GooseExtClient } from "@aaif/goose-acp-client";

async function connectToGoose(stream: Stream) {
  const app = createAcpClient({ name: "my-product" });
  const connection = app.connect(stream);
  const goose = new GooseExtClient(connection.agent);

  await connection.agent.request(methods.agent.initialize, {
    protocolVersion: PROTOCOL_VERSION,
    clientInfo: {
      name: "my-product",
      version: "1.0.0",
    },
    clientCapabilities: {},
  });

  return { connection, goose };
}
```

The application creates and owns the `stream`, including its connection and
process lifecycle. Call `connection.close()` when the application no longer
needs the connection.

## Development

From `ui/goose-acp-client`:

```bash
pnpm run build
```

The generated TypeScript types come from the Rust schemas in `crates/goose`.

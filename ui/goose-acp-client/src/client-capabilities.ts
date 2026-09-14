import type { GooseMcpHostCapabilities } from "./mcp-apps.js";

export interface GooseClientCapabilitiesMeta {
  goose?: {
    mcpHostCapabilities?: GooseMcpHostCapabilities;
    customNotifications?: boolean;
  };
}

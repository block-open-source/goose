import { RESOURCE_MIME_TYPE } from "@modelcontextprotocol/ext-apps/app-bridge";
import type {
  McpUiAppResourceConfig,
  McpUiAppToolConfig,
} from "@modelcontextprotocol/ext-apps/server";
import type {
  BlobResourceContents,
  ReadResourceResult,
  TextResourceContents,
  Tool,
} from "@modelcontextprotocol/sdk/types.js";

export const GOOSE_MCP_UI_EXTENSION_ID = "io.modelcontextprotocol/ui" as const;

export interface GooseMcpUiExtensionSettings {
  mimeTypes: string[];
}

export interface GooseMcpHostCapabilities {
  extensions: Record<string, GooseMcpUiExtensionSettings>;
}

export type GooseToolUiMetadata = Extract<
  McpUiAppToolConfig["_meta"],
  { ui: unknown }
>["ui"];

export type GooseToolMetadata = NonNullable<Tool["_meta"]> & {
  ui?: GooseToolUiMetadata;
  goose_extension?: string;
};

export type GooseSessionTool = Tool & {
  meta?: GooseToolMetadata;
  _meta?: GooseToolMetadata;
};

export type GooseTextResourceContents = TextResourceContents;

export type GooseBlobResourceContents = BlobResourceContents;

export type GooseResourceContents = TextResourceContents | BlobResourceContents;

export type GooseReadResourceResult = ReadResourceResult;

export type GooseResourceMetadata = NonNullable<
  Extract<NonNullable<McpUiAppResourceConfig["_meta"]>, { ui?: unknown }>["ui"]
>;

export interface GooseMcpAppToolPayload {
  toolName: string;
  extensionName: string;
  resourceUri: string;
  toolMeta?: GooseToolMetadata;
  resourceResult?: GooseReadResourceResult | null;
  readError?: string;
}

export interface GooseToolCallUpdateMeta {
  goose?: {
    mcpApp?: GooseMcpAppToolPayload;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

export const DEFAULT_GOOSE_MCP_HOST_CAPABILITIES: GooseMcpHostCapabilities = {
  extensions: {
    [GOOSE_MCP_UI_EXTENSION_ID]: {
      mimeTypes: [RESOURCE_MIME_TYPE],
    },
  },
};

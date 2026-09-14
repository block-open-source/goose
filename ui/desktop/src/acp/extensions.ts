import type { ExtensionConfig, ExtensionEntry } from '../types/extensions';
import type { GooseExtension, GooseExtensionEntry } from '@aaif/goose-acp-client';
import { getAcpClient } from './acpConnection';

export type ConfiguredExtensionEntry = ExtensionEntry & { configKey?: string };

export interface ConfiguredExtensionsResponse {
  extensions: ConfiguredExtensionEntry[];
  warnings: string[];
}

export function gooseExtensionName(extension: GooseExtension): string {
  return extension.type === 'mcp' ? extension.server.name : extension.name;
}

function headersToRecord(headers: { name: string; value: string }[] = []) {
  return Object.fromEntries(headers.map(({ name, value }) => [name, value]));
}

function availableToolsOrUndefined(availableTools?: string[] | null): string[] | undefined {
  return availableTools?.length ? availableTools : undefined;
}

export function gooseExtensionToExtensionConfig(extension: GooseExtension): ExtensionConfig | null {
  switch (extension.type) {
    case 'builtin':
    case 'platform':
      return {
        ...extension,
        description: extension.description ?? '',
        available_tools: availableToolsOrUndefined(extension.available_tools),
      };
    case 'mcp': {
      const server = extension.server;
      if ('command' in server) {
        return {
          type: 'stdio',
          name: server.name,
          description: extension.description ?? '',
          cmd: server.command,
          args: server.args,
          env_keys: extension.envKeys ?? [],
          timeout: extension.timeout,
          bundled: extension.bundled,
          available_tools: availableToolsOrUndefined(extension.available_tools),
        };
      }
      if ('url' in server) {
        return {
          type: 'streamable_http',
          name: server.name,
          description: extension.description ?? '',
          uri: server.url,
          headers: headersToRecord(server.headers),
          env_keys: extension.envKeys ?? [],
          timeout: extension.timeout,
          socket: extension.socket,
          client_id: extension.clientId,
          client_secret_key: extension.clientSecretKey,
          scopes: extension.scopes ?? [],
          bundled: extension.bundled,
          available_tools: availableToolsOrUndefined(extension.available_tools),
        };
      }
      return null;
    }
  }
}

function gooseExtensionEntryToExtensionEntry(
  entry: GooseExtensionEntry
): ConfiguredExtensionEntry | null {
  const config = gooseExtensionToExtensionConfig(entry.extension);
  if (!config) {
    return null;
  }
  return { ...config, enabled: entry.enabled, configKey: entry.configKey ?? undefined };
}

export async function getConfiguredGooseExtensions(): Promise<GooseExtensionEntry[]> {
  const client = await getAcpClient();
  const response = await client.goose.configExtensionsList_unstable({});
  return response.extensions;
}

export async function getConfiguredExtensions(): Promise<ConfiguredExtensionsResponse> {
  const client = await getAcpClient();
  const response = await client.goose.configExtensionsList_unstable({});
  return {
    extensions: response.extensions
      .map(gooseExtensionEntryToExtensionEntry)
      .filter((entry): entry is ConfiguredExtensionEntry => entry !== null),
    warnings: response.warnings ?? [],
  };
}

export function extensionConfigToGooseExtension(config: ExtensionConfig): GooseExtension | null {
  switch (config.type) {
    case 'builtin':
      return {
        type: 'builtin',
        name: config.name,
        description: config.description,
        display_name: config.display_name,
        timeout: config.timeout,
        bundled: config.bundled,
        available_tools: availableToolsOrUndefined(config.available_tools),
      };
    case 'platform':
      return {
        type: 'platform',
        name: config.name,
        description: config.description,
        display_name: config.display_name,
        bundled: config.bundled,
        available_tools: availableToolsOrUndefined(config.available_tools),
      };
    case 'stdio':
      return {
        type: 'mcp',
        server: { name: config.name, command: config.cmd, args: config.args ?? [], env: [] },
        envKeys: config.env_keys ?? [],
        description: config.description,
        timeout: config.timeout,
        bundled: config.bundled,
        available_tools: availableToolsOrUndefined(config.available_tools),
      };
    case 'streamable_http':
      return {
        type: 'mcp',
        server: {
          type: 'http',
          name: config.name,
          url: config.uri,
          headers: Object.entries(config.headers ?? {}).map(([name, value]) => ({ name, value })),
        },
        envKeys: config.env_keys ?? [],
        description: config.description,
        timeout: config.timeout,
        socket: config.socket,
        clientId: config.client_id,
        clientSecretKey: config.client_secret_key,
        scopes: config.scopes ?? [],
        bundled: config.bundled,
        available_tools: availableToolsOrUndefined(config.available_tools),
      };
  }
}

export async function addConfigExtension(config: ExtensionConfig, enabled: boolean): Promise<void> {
  const extension = extensionConfigToGooseExtension(config);
  if (!extension) {
    throw new Error(`Unsupported extension type for ACP: ${config.type}`);
  }
  const client = await getAcpClient();
  await client.goose.configExtensionsAdd_unstable({ extension, enabled });
}

export async function removeConfigExtension(configKey: string): Promise<void> {
  const client = await getAcpClient();
  await client.goose.configExtensionsRemove_unstable({ configKey });
}

export async function setConfigExtensionEnabled(
  configKey: string,
  enabled: boolean
): Promise<void> {
  const client = await getAcpClient();
  await client.goose.configExtensionsSetEnabled_unstable({ configKey, enabled });
}

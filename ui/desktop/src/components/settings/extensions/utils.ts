import type { FixedExtensionEntry } from '../../ConfigContext';
import type { ExtensionConfig } from '../../../types/extensions';

// Default extension timeout in seconds
// TODO: keep in sync with rust better

export const DEFAULT_EXTENSION_TIMEOUT = 300;

/**
 * Converts an extension name to a key format
 * TODO: need to keep this in sync better with `name_to_key` on the rust side
 */
export function nameToKey(name: string): string {
  return name
    .split('')
    .filter((char) => !char.match(/\s/))
    .join('')
    .toLowerCase();
}

export interface ExtensionFormData {
  name: string;
  description: string;
  type: 'stdio' | 'streamable_http' | 'builtin';
  cmd?: string;
  endpoint?: string;
  enabled: boolean;
  timeout?: number;
  envVars: {
    key: string;
    value: string;
    isEdited?: boolean;
  }[];
  headers: {
    key: string;
    value: string;
    isEdited?: boolean;
  }[];
  installation_notes?: string;
  available_tools?: string[];
  // streamable_http fields with no form input yet; carried through so an
  // unrelated edit does not strip them from the saved config.
  socket?: string | null;
  client_id?: string | null;
  client_secret_key?: string | null;
  scopes?: string[];
}

export function getDefaultFormData(): ExtensionFormData {
  return {
    name: '',
    description: '',
    type: 'stdio',
    cmd: '',
    endpoint: '',
    enabled: true,
    timeout: 300,
    envVars: [],
    headers: [],
  };
}

export function extensionToFormData(extension: FixedExtensionEntry): ExtensionFormData {
  // Type guard: Check if 'envs' property exists for this variant
  const hasEnvs = extension.type === 'streamable_http' || extension.type === 'stdio';

  // Handle both envs (legacy) and env_keys (new secrets)
  let envVars = [];

  // Add legacy envs with their values
  if (hasEnvs && extension.envs) {
    envVars.push(
      ...Object.entries(extension.envs).map(([key, value]) => ({
        key,
        value: value as string,
        isEdited: true, // We want to submit legacy values as secrets to migrate forward
      }))
    );
  }

  // Add env_keys with placeholder values
  if (hasEnvs && extension.env_keys) {
    envVars.push(
      ...extension.env_keys.map((key) => ({
        key,
        value: '••••••••', // Placeholder for secret values
        isEdited: false, // Mark as not edited initially
      }))
    );
  }

  // Handle headers for streamable_http
  let headers = [];
  if (extension.type === 'streamable_http' && 'headers' in extension && extension.headers) {
    headers.push(
      ...Object.entries(extension.headers).map(([key, value]) => ({
        key,
        value: value as string,
        isEdited: false, // Mark as not edited initially
      }))
    );
  }

  const availableTools =
    'available_tools' in extension
      ? availableToolsOrUndefined(extension.available_tools)
      : undefined;

  return {
    name: extension.name || '',
    description: extension.description || '',
    type: extension.type === 'platform' ? 'stdio' : extension.type,
    cmd:
      extension.type === 'stdio'
        ? combineCmdAndArgs(extension.cmd, extension.args ?? [])
        : undefined,
    endpoint: extension.type === 'streamable_http' ? (extension.uri ?? undefined) : undefined,
    enabled: extension.enabled,
    timeout: 'timeout' in extension ? (extension.timeout ?? undefined) : undefined,
    envVars,
    headers,
    installation_notes: (extension as Record<string, unknown>)['installation_notes'] as
      string | undefined,
    ...(availableTools ? { available_tools: availableTools } : {}),
    ...(extension.type === 'streamable_http'
      ? {
          socket: extension.socket,
          client_id: extension.client_id,
          client_secret_key: extension.client_secret_key,
          scopes: extension.scopes,
        }
      : {}),
  };
}

function availableToolsOrUndefined(availableTools?: string[] | null): string[] | undefined {
  return availableTools && availableTools.length > 0 ? availableTools : undefined;
}

function availableToolsConfig(availableTools?: string[] | null) {
  const normalized = availableToolsOrUndefined(availableTools);
  return normalized ? { available_tools: normalized } : undefined;
}

export function createExtensionConfig(formData: ExtensionFormData): ExtensionConfig {
  // Extract just the keys from env vars
  const env_keys = formData.envVars.map(({ key }) => key).filter((key) => key.length > 0);

  if (formData.type === 'stdio') {
    // we put the cmd + args all in the form cmd field but need to split out into cmd + args
    const { cmd, args } = splitCmdAndArgs(formData.cmd || '');

    return {
      type: 'stdio',
      name: formData.name,
      description: formData.description,
      cmd: cmd,
      args: args,
      timeout: formData.timeout,
      ...(env_keys.length > 0 ? { env_keys } : {}),
      ...availableToolsConfig(formData.available_tools),
    };
  } else if (formData.type === 'streamable_http') {
    // Extract headers
    const headers = formData.headers
      .filter(({ key, value }) => key.length > 0 && value.length > 0)
      .reduce(
        (acc, header) => {
          acc[header.key] = header.value;
          return acc;
        },
        {} as Record<string, string>
      );

    return {
      type: 'streamable_http',
      name: formData.name,
      description: formData.description,
      timeout: formData.timeout,
      uri: formData.endpoint || '',
      ...(env_keys.length > 0 ? { env_keys } : {}),
      headers,
      ...availableToolsConfig(formData.available_tools),
      ...(formData.socket != null ? { socket: formData.socket } : {}),
      ...(formData.client_id != null ? { client_id: formData.client_id } : {}),
      ...(formData.client_secret_key != null
        ? { client_secret_key: formData.client_secret_key }
        : {}),
      ...(formData.scopes?.length ? { scopes: formData.scopes } : {}),
    };
  }

  return {
    type: formData.type,
    name: formData.name,
    description: formData.description,
    timeout: formData.timeout,
    ...availableToolsConfig(formData.available_tools),
  };
}

function isWindowsPlatform(): boolean {
  return typeof window !== 'undefined' && window.electron?.platform === 'win32';
}

export function splitCmdAndArgs(str: string): { cmd: string; args: string[] } {
  const trimmed = str.trim();
  if (!trimmed) {
    return { cmd: '', args: [] };
  }

  const words = parseCommandLine(trimmed, isWindowsPlatform());

  const cmd = words[0] || '';
  const args = words.slice(1);

  return {
    cmd,
    args,
  };
}

function parseCommandLine(value: string, windows: boolean): string[] {
  const words: string[] = [];
  let word = '';
  let wordStarted = false;
  let quote: "'" | '"' | undefined;

  for (let index = 0; index < value.length; index += 1) {
    const character = value[index];

    if (windows && quote !== "'" && character === '\\') {
      let runEnd = index;
      while (value[runEnd] === '\\') {
        runEnd += 1;
      }

      const backslashCount = runEnd - index;
      if (value[runEnd] === '"') {
        word += '\\'.repeat(Math.floor(backslashCount / 2));
        if (backslashCount % 2 === 0) {
          quote = quote === '"' ? undefined : '"';
        } else {
          word += '"';
        }
        index = runEnd;
      } else {
        word += '\\'.repeat(backslashCount);
        index = runEnd - 1;
      }
      wordStarted = true;
    } else if (quote) {
      if (character === quote) {
        quote = undefined;
      } else if (quote === '"' && character === '\\' && index + 1 < value.length) {
        const next = value[index + 1];
        if (next === '"' || next === '\\' || next === '$') {
          word += next;
          index += 1;
        } else {
          word += character;
        }
      } else {
        word += character;
      }
      wordStarted = true;
    } else if (/\s/.test(character)) {
      if (wordStarted) {
        words.push(word);
        word = '';
        wordStarted = false;
      }
    } else if (character === '"' || (!windows && character === "'")) {
      quote = character;
      wordStarted = true;
    } else if (character === '\\' && !windows && index + 1 < value.length) {
      word += value[index + 1];
      wordStarted = true;
      index += 1;
    } else {
      word += character;
      wordStarted = true;
    }
  }

  if (wordStarted) {
    words.push(word);
  }

  return words;
}

export function combineCmdAndArgs(cmd: string, args: string[]): string {
  const windows = isWindowsPlatform();
  return [cmd, ...args].map((value) => quoteCommandPart(value, windows)).join(' ');
}

function quoteCommandPart(value: string, windows: boolean): string {
  if (windows) {
    return quoteWindowsCommandPart(value);
  }

  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, `'"'"'`)}'`;
}

function quoteWindowsCommandPart(value: string): string {
  if (value.length > 0 && !/[\s"]/u.test(value)) {
    return value;
  }

  let quoted = '"';
  let backslashCount = 0;

  for (const character of value) {
    if (character === '\\') {
      backslashCount += 1;
    } else if (character === '"') {
      quoted += '\\'.repeat(backslashCount * 2 + 1) + character;
      backslashCount = 0;
    } else {
      quoted += '\\'.repeat(backslashCount) + character;
      backslashCount = 0;
    }
  }

  return quoted + '\\'.repeat(backslashCount * 2) + '"';
}

export function extractCommand(link: string): string {
  const url = new URL(link);
  const cmd = url.searchParams.get('cmd') || 'Unknown Command';
  const args = url.searchParams.getAll('arg').map(decodeURIComponent);

  // Combine the command and its arguments into a reviewable format
  return `${cmd} ${args.join(' ')}`.trim();
}

export function extractExtensionName(link: string): string {
  const url = new URL(link);
  const name = url.searchParams.get('name');
  return name ? decodeURIComponent(name) : 'Unknown Extension';
}

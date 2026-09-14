import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  nameToKey,
  getDefaultFormData,
  extensionToFormData,
  createExtensionConfig,
  extractCommand,
  extractExtensionName,
  splitCmdAndArgs,
  combineCmdAndArgs,
  DEFAULT_EXTENSION_TIMEOUT,
} from './utils';
import type { FixedExtensionEntry } from '../../ConfigContext';

describe('Extension Utils', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe('nameToKey', () => {
    it('should convert name to lowercase key format', () => {
      expect(nameToKey('My Extension')).toBe('myextension');
      expect(nameToKey('Test-Extension_Name')).toBe('test-extension_name');
      expect(nameToKey('UPPERCASE')).toBe('uppercase');
    });

    it('should remove spaces', () => {
      expect(nameToKey('Extension With Spaces')).toBe('extensionwithspaces');
      expect(nameToKey('  Multiple   Spaces  ')).toBe('multiplespaces');
    });
  });

  describe('getDefaultFormData', () => {
    it('should return default form data structure', () => {
      const defaultData = getDefaultFormData();

      expect(defaultData).toEqual({
        name: '',
        description: '',
        type: 'stdio',
        cmd: '',
        endpoint: '',
        enabled: true,
        timeout: 300,
        envVars: [],
        headers: [],
      });
    });
  });

  describe('extensionToFormData', () => {
    it('should convert stdio extension to form data', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'test-extension',
        description: 'Test description',
        cmd: 'python',
        args: ['script.py', '--flag'],
        enabled: true,
        timeout: 600,
        env_keys: ['API_KEY', 'SECRET'],
      };

      const formData = extensionToFormData(extension);

      expect(formData).toEqual({
        name: 'test-extension',
        description: 'Test description',
        type: 'stdio',
        cmd: 'python script.py --flag',
        endpoint: undefined,
        enabled: true,
        timeout: 600,
        envVars: [
          { key: 'API_KEY', value: '••••••••', isEdited: false },
          { key: 'SECRET', value: '••••••••', isEdited: false },
        ],
        headers: [],
      });
    });

    it('should convert streamable_http extension to form data', () => {
      const extension: FixedExtensionEntry = {
        type: 'streamable_http',
        name: 'http-extension',
        description: 'HTTP description',
        uri: 'http://api.example.com',
        enabled: true,
        headers: {
          Authorization: 'Bearer token',
          'Content-Type': 'application/json',
        },
        env_keys: ['API_KEY'],
      };

      const formData = extensionToFormData(extension);

      expect(formData).toEqual({
        name: 'http-extension',
        description: 'HTTP description',
        type: 'streamable_http',
        cmd: undefined,
        endpoint: 'http://api.example.com',
        enabled: true,
        timeout: undefined,
        envVars: [{ key: 'API_KEY', value: '••••••••', isEdited: false }],
        headers: [
          { key: 'Authorization', value: 'Bearer token', isEdited: false },
          { key: 'Content-Type', value: 'application/json', isEdited: false },
        ],
      });
    });

    it('should preserve streamable_http OAuth client fields and socket through a form round-trip', () => {
      const extension: FixedExtensionEntry = {
        type: 'streamable_http',
        name: 'oauth-extension',
        description: 'OAuth description',
        uri: 'http://api.example.com',
        enabled: true,
        headers: {},
        socket: '@egress.sock',
        client_id: 'registered-client',
        client_secret_key: 'OAUTH_CLIENT_SECRET',
        scopes: ['scope.read'],
      };

      const formData = extensionToFormData(extension);
      const config = createExtensionConfig(formData);

      expect(config).toMatchObject({
        type: 'streamable_http',
        socket: '@egress.sock',
        client_id: 'registered-client',
        client_secret_key: 'OAUTH_CLIENT_SECRET',
        scopes: ['scope.read'],
      });
    });

    it('should handle legacy envs field', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'legacy-extension',
        description: 'legacy',
        cmd: 'node',
        args: ['app.js'],
        enabled: true,
        envs: {
          OLD_KEY: 'old_value',
          LEGACY_TOKEN: 'legacy_token',
        },
        env_keys: ['NEW_KEY'],
      };

      const formData = extensionToFormData(extension);

      expect(formData.envVars).toEqual([
        { key: 'OLD_KEY', value: 'old_value', isEdited: true },
        { key: 'LEGACY_TOKEN', value: 'legacy_token', isEdited: true },
        { key: 'NEW_KEY', value: '••••••••', isEdited: false },
      ]);
    });

    it('should handle builtin extension', () => {
      const extension: FixedExtensionEntry = {
        type: 'builtin',
        name: 'developer',
        description: 'developer',
        enabled: true,
      };

      const formData = extensionToFormData(extension);

      expect(formData).toEqual({
        name: 'developer',
        description: 'developer',
        type: 'builtin',
        cmd: undefined,
        endpoint: undefined,
        enabled: true,
        timeout: undefined,
        envVars: [],
        headers: [],
      });
    });

    it('should preserve available tools metadata', () => {
      const extension: FixedExtensionEntry = {
        type: 'builtin',
        name: 'developer',
        description: 'developer',
        enabled: true,
        available_tools: ['shell', 'read_file'],
      };

      const formData = extensionToFormData(extension);

      expect(formData.available_tools).toEqual(['shell', 'read_file']);
    });

    it('should omit empty available tools metadata', () => {
      const extension: FixedExtensionEntry = {
        type: 'builtin',
        name: 'developer',
        description: 'developer',
        enabled: true,
        available_tools: [],
      };

      const formData = extensionToFormData(extension);

      expect(formData).not.toHaveProperty('available_tools');
    });

    it('should not escape @ in command args', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'context7',
        description: 'Context7 MCP',
        cmd: 'npx',
        args: ['-y', '@upstash/context7-mcp'],
        enabled: true,
      };

      const formData = extensionToFormData(extension);
      expect(formData.cmd).toBe('npx -y @upstash/context7-mcp');
    });

    it('should quote args with spaces', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'java-app',
        description: 'Java app',
        cmd: '/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java',
        args: ['-classpath', '/path/with spaces/lib.jar', 'Main'],
        enabled: true,
      };

      const formData = extensionToFormData(extension);
      expect(formData.cmd).toBe(
        "'/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java' -classpath '/path/with spaces/lib.jar' Main"
      );
    });

    it('should roundtrip command with @ through form data', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'context7',
        description: 'Context7 MCP',
        cmd: 'npx',
        args: ['-y', '@upstash/context7-mcp'],
        enabled: true,
      };

      const formData = extensionToFormData(extension);
      const { cmd, args } = splitCmdAndArgs(formData.cmd || '');
      expect(cmd).toBe('npx');
      expect(args).toEqual(['-y', '@upstash/context7-mcp']);
    });

    it('should roundtrip command with spaces through form data', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'java-app',
        description: 'Java app',
        cmd: '/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java',
        args: ['-classpath', '/path/with spaces/lib.jar', 'Main'],
        enabled: true,
      };

      const formData = extensionToFormData(extension);
      const { cmd, args } = splitCmdAndArgs(formData.cmd || '');
      expect(cmd).toBe('/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java');
      expect(args).toEqual(['-classpath', '/path/with spaces/lib.jar', 'Main']);
    });

    it('should roundtrip args with double quotes and spaces through form data', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'test',
        description: 'test',
        cmd: 'node',
        args: ['/My "Project"/bin/run'],
        enabled: true,
      };

      const formData = extensionToFormData(extension);
      expect(formData.cmd).toBe('node \'/My "Project"/bin/run\'');
      const { cmd, args } = splitCmdAndArgs(formData.cmd || '');
      expect(cmd).toBe('node');
      expect(args).toEqual(['/My "Project"/bin/run']);
    });

    it.each([
      ['-"c"', 'touch /tmp/pwned'],
      ['--flag=value;next', '|', '>', '*'],
      ['', 'both\'and"quotes', '$HOME', '`command`'],
      ['line one\nline two', 'trailing\\', 'café'],
    ])('faithfully roundtrips metacharacter-bearing argv %#', (...args) => {
      const combined = combineCmdAndArgs('npx', args);
      expect(splitCmdAndArgs(combined)).toEqual({ cmd: 'npx', args });
    });

    it('does not synthesize a blocked npx flag through the extension form', () => {
      const extension: FixedExtensionEntry = {
        type: 'stdio',
        name: 'quoted-flag',
        description: 'quoted flag regression',
        cmd: 'npx',
        args: ['-"c"', 'touch /tmp/pwned'],
        enabled: true,
      };

      expect(createExtensionConfig(extensionToFormData(extension))).toMatchObject({
        cmd: 'npx',
        args: ['-"c"', 'touch /tmp/pwned'],
      });
    });
  });

  describe('createExtensionConfig', () => {
    it('should create stdio extension config', () => {
      const formData = {
        name: 'test-stdio',
        description: 'Test stdio extension',
        type: 'stdio' as const,
        cmd: 'python script.py --arg1 --arg2',
        endpoint: '',
        enabled: true,
        timeout: 300,
        envVars: [
          { key: 'API_KEY', value: 'secret123', isEdited: true },
          { key: '', value: '', isEdited: false }, // Should be filtered out
        ],
        headers: [],
      };

      const config = createExtensionConfig(formData);

      expect(config).toEqual({
        type: 'stdio',
        name: 'test-stdio',
        description: 'Test stdio extension',
        cmd: 'python',
        args: ['script.py', '--arg1', '--arg2'],
        timeout: 300,
        env_keys: ['API_KEY'],
      });
    });

    it('should create streamable_http extension config', () => {
      const formData = {
        name: 'test-http',
        description: 'Test HTTP extension',
        type: 'streamable_http' as const,
        cmd: '',
        endpoint: 'http://api.example.com',
        enabled: true,
        timeout: 300,
        envVars: [{ key: 'API_KEY', value: 'key123', isEdited: true }],
        headers: [
          { key: 'Authorization', value: 'Bearer token', isEdited: true },
          { key: '', value: '', isEdited: false }, // Should be filtered out
        ],
      };

      const config = createExtensionConfig(formData);

      expect(config).toEqual({
        type: 'streamable_http',
        name: 'test-http',
        description: 'Test HTTP extension',
        timeout: 300,
        uri: 'http://api.example.com',
        env_keys: ['API_KEY'],
        headers: {
          Authorization: 'Bearer token',
        },
      });
    });

    it('should create builtin extension config', () => {
      const formData = {
        name: 'developer',
        description: 'developer',
        type: 'builtin' as const,
        cmd: '',
        endpoint: '',
        enabled: true,
        timeout: 300,
        envVars: [],
        headers: [],
      };

      const config = createExtensionConfig(formData);

      expect(config).toEqual({
        type: 'builtin',
        name: 'developer',
        description: 'developer',
        timeout: 300,
      });
    });

    it('should preserve available tools metadata', () => {
      const formData = {
        name: 'developer',
        description: 'developer',
        type: 'builtin' as const,
        cmd: '',
        endpoint: '',
        enabled: true,
        timeout: 300,
        envVars: [],
        headers: [],
        available_tools: ['shell', 'read_file'],
      };

      const config = createExtensionConfig(formData);

      expect(config).toEqual({
        type: 'builtin',
        name: 'developer',
        description: 'developer',
        timeout: 300,
        available_tools: ['shell', 'read_file'],
      });
    });

    it('should omit empty available tools metadata', () => {
      const formData = {
        name: 'developer',
        description: 'developer',
        type: 'builtin' as const,
        cmd: '',
        endpoint: '',
        enabled: true,
        timeout: 300,
        envVars: [],
        headers: [],
        available_tools: [],
      };

      const config = createExtensionConfig(formData);

      expect(config).not.toHaveProperty('available_tools');
    });
  });

  describe('splitCmdAndArgs', () => {
    it.each([
      ['python script.py', { cmd: 'python', args: ['script.py'] }],
      ['python script.py --flag', { cmd: 'python', args: ['script.py', '--flag'] }],
      [
        "java -classpath '/path/with spaces/lib.jar' Main",
        { cmd: 'java', args: ['-classpath', '/path/with spaces/lib.jar', 'Main'] },
      ],
      [
        '"/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java" -classpath "/path/with spaces/lib.jar" Main',
        {
          cmd: '/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java',
          args: ['-classpath', '/path/with spaces/lib.jar', 'Main'],
        },
      ],
      [
        'node --max-old-space-size=4096 app.js',
        { cmd: 'node', args: ['--max-old-space-size=4096', 'app.js'] },
      ],
      ['  python   script.py  ', { cmd: 'python', args: ['script.py'] }],
      ['', { cmd: '', args: [] }],
    ])('splits %j correctly', (input, expected) => {
      expect(splitCmdAndArgs(input)).toEqual(expected);
    });

    describe('on Windows', () => {
      beforeEach(() => {
        vi.stubGlobal('window', { electron: { platform: 'win32' } });
      });
      afterEach(() => {
        vi.unstubAllGlobals();
      });

      it('preserves backslashes in a Windows path', () => {
        expect(splitCmdAndArgs('C:\\Users\\name\\path\\to\\extension.js')).toEqual({
          cmd: 'C:\\Users\\name\\path\\to\\extension.js',
          args: [],
        });
      });

      it('preserves backslashes in cmd and args', () => {
        expect(splitCmdAndArgs('node C:\\Users\\name\\ext.js')).toEqual({
          cmd: 'node',
          args: ['C:\\Users\\name\\ext.js'],
        });
      });

      it('treats apostrophes as literal Windows command-line characters', () => {
        expect(splitCmdAndArgs("node C:\\Users\\O'Connor\\ext.js --owner=O'Connor")).toEqual({
          cmd: 'node',
          args: ["C:\\Users\\O'Connor\\ext.js", "--owner=O'Connor"],
        });
      });

      it('handles a quoted Windows path containing spaces', () => {
        expect(splitCmdAndArgs('"C:\\Program Files\\app\\ext.js"')).toEqual({
          cmd: 'C:\\Program Files\\app\\ext.js',
          args: [],
        });
      });

      it('preserves a quoted UNC path', () => {
        expect(splitCmdAndArgs(String.raw`node "\\server\share with space\extension.js"`)).toEqual({
          cmd: 'node',
          args: [String.raw`\\server\share with space\extension.js`],
        });
      });

      it('preserves a quoted path ending in a backslash', () => {
        expect(splitCmdAndArgs(String.raw`node "C:\dir with space\\"`)).toEqual({
          cmd: 'node',
          args: ['C:\\dir with space\\'],
        });
      });

      it('roundtrips quoted Windows paths and metacharacters', () => {
        const cmd = 'C:\\Program Files\\nodejs\\npx.cmd';
        const args = ["C:\\Users\\O'Connor\\extension.js", '-"c"', 'a&b', 'quoted "value"\\'];
        expect(splitCmdAndArgs(combineCmdAndArgs(cmd, args))).toEqual({ cmd, args });
      });
    });
  });

  describe('extractCommand', () => {
    it('should extract command from extension link', () => {
      const link = 'goose://extension/add?name=Test&cmd=python&arg=script.py&arg=--flag';
      expect(extractCommand(link)).toBe('python script.py --flag');
    });

    it('should handle encoded arguments', () => {
      const link = 'goose://extension/add?cmd=echo&arg=hello%20world&arg=--test%3Dvalue';
      expect(extractCommand(link)).toBe('echo hello world --test=value');
    });

    it('should handle missing command', () => {
      const link = 'goose://extension/add?name=Test';
      expect(extractCommand(link)).toBe('Unknown Command');
    });

    it('should handle command without arguments', () => {
      const link = 'goose://extension/add?cmd=python';
      expect(extractCommand(link)).toBe('python');
    });
  });

  describe('extractExtensionName', () => {
    it('should extract extension name from link', () => {
      const link = 'goose://extension/add?name=Test%20Extension&cmd=python';
      expect(extractExtensionName(link)).toBe('Test Extension');
    });

    it('should handle missing name', () => {
      const link = 'goose://extension/add?cmd=python';
      expect(extractExtensionName(link)).toBe('Unknown Extension');
    });

    it('should decode URL encoded names', () => {
      const link = 'goose://extension/add?name=My%20Special%20Extension%21';
      expect(extractExtensionName(link)).toBe('My Special Extension!');
    });
  });

  describe('DEFAULT_EXTENSION_TIMEOUT', () => {
    it('should have correct default timeout value', () => {
      expect(DEFAULT_EXTENSION_TIMEOUT).toBe(300);
    });
  });
});

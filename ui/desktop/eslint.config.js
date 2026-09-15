const js = require('@eslint/js');
const tsParser = require('@typescript-eslint/parser');
const tsPlugin = require('@typescript-eslint/eslint-plugin');
const reactPlugin = require('eslint-plugin-react');
const reactHooksPlugin = require('eslint-plugin-react-hooks');

// Custom rule to detect window.location.href usage
const noWindowLocationHref = {
  meta: {
    type: 'problem',
    docs: {
      description: 'Disallow direct usage of window.location.href in Electron apps',
      recommended: true,
    },
  },
  create(context) {
    return {
      AssignmentExpression(node) {
        if (
          node.left.type === 'MemberExpression' &&
          node.left.object.type === 'MemberExpression' &&
          node.left.object.object.type === 'Identifier' &&
          node.left.object.object.name === 'window' &&
          node.left.object.property.name === 'location' &&
          node.left.property.name === 'href'
        ) {
          context.report({
            node,
            message: 'Do not use window.location.href directly in Electron apps. Use setView from the project instead'
          });
        }
      }
    };
  }
};

module.exports = [
  js.configs.recommended,
  {
    ignores: ['src/api/**'],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 'latest',
        sourceType: 'module',
        ecmaFeatures: {
          jsx: true,
        },
      },
      globals: {
        // Electron/Node.js globals
        process: 'readonly',
        __dirname: 'readonly',
        require: 'readonly',
        module: 'readonly',
        Buffer: 'readonly',
        TextDecoder: 'readonly',
        TextEncoder: 'readonly',
        // Browser globals
        window: 'readonly',
        document: 'readonly',
        navigator: 'readonly',
        localStorage: 'readonly',
        fetch: 'readonly',
        console: 'readonly',
        setTimeout: 'readonly',
        clearInterval: 'readonly',
        setInterval: 'readonly',
        clearTimeout: 'readonly',
        CustomEvent: 'readonly',
        Element: 'readonly',
        HTMLElement: 'readonly',
        HTMLInputElement: 'readonly',
        HTMLSelectElement: 'readonly',
        HTMLTextAreaElement: 'readonly',
        HTMLButtonElement: 'readonly',
        HTMLDivElement: 'readonly',
        HTMLCanvasElement: 'readonly',
        HTMLIFrameElement: 'readonly',
        MessageEvent: 'readonly',
        StorageEvent: 'readonly',
        File: 'readonly',
        FileList: 'readonly',
        FileReader: 'readonly',
        DOMParser: 'readonly',
        URL: 'readonly',
        URLSearchParams: 'readonly',
        Response: 'readonly',
        ReadableStream: 'readonly',
        ReadableStreamDefaultController: 'readonly',
        WritableStream: 'readonly',
        AbortController: 'readonly',
        AbortSignal: 'readonly',
        RequestCredentials: 'readonly',
        HeadersInit: 'readonly',
        KeyboardEvent: 'readonly',
        MouseEvent: 'readonly', // Add MouseEvent
        Event: 'readonly', // Add Event
        Node: 'readonly', // Add Node
        React: 'readonly',
        handleAction: 'readonly',
        requestAnimationFrame: 'readonly',
        cancelAnimationFrame: 'readonly',
        ResizeObserver: 'readonly',
        MutationObserver: 'readonly',
        NodeFilter: 'readonly',
        Text: 'readonly',
        AudioContext: 'readonly',
        AnalyserNode: 'readonly',
        MediaRecorder: 'readonly',
        MediaStream: 'readonly',
        AudioWorkletNode: 'readonly',
        DOMException: 'readonly',
        MediaDeviceInfo: 'readonly',
        MediaTrackConstraints: 'readonly',
        Blob: 'readonly',
        FormData: 'readonly',
      },
    },
    plugins: {
      '@typescript-eslint': tsPlugin,
      'react': reactPlugin,
      'react-hooks': reactHooksPlugin,
      'custom': {
        rules: {
          'no-window-location-href': noWindowLocationHref
        }
      }
    },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      ...reactPlugin.configs.recommended.rules,
      ...reactHooksPlugin.configs.recommended.rules,
      // Customize rules
      'react/react-in-jsx-scope': 'off',
      'react/prop-types': 'off', // We use TypeScript for prop validation
      'react/no-unknown-property': ['error', {
        ignore: ['dark:fill'] // Allow Tailwind dark mode syntax
      }],
      'react/no-unescaped-entities': 'off', // Allow quotes in JSX
      '@typescript-eslint/explicit-module-boundary-types': 'off',
      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/no-unused-vars': ['warn', {
        argsIgnorePattern: '^_',
        varsIgnorePattern: '^_'
      }],
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'warn',
      '@typescript-eslint/no-var-requires': 'warn', // Downgrade to warning for Electron main process
      'no-undef': 'error',
      'no-useless-catch': 'warn',
      'custom/no-window-location-href': 'error'
    },
    settings: {
      react: {
        version: 'detect',
      },
    },
  },
];

import { DEFAULT_GOOSE_MCP_HOST_CAPABILITIES } from '@aaif/goose-acp-client';
import { methods, PROTOCOL_VERSION, type InitializeResponse } from '@agentclientprotocol/sdk';
import { createWebSocketStream } from '@agentclientprotocol/sdk/experimental/ws-client';
import packageJson from '../../package.json';
import { GOOSE_SERVE_EXITED_USER_MESSAGE } from '../gooseServeLeaseRegistry';
import {
  handleAcpGooseSessionNotification,
  handleAcpProviderDeviceCodeNotification,
  handleAcpSessionNotification,
} from './chatNotifications';
import { requestAcpElicitation } from './elicitationRequests';
import {
  connectGooseAcpClient,
  type GooseAcpCallbacks,
  type GooseAcpClient,
} from './gooseAcpClient';
import { requestAcpPermission } from './permissionRequests';
import { requestAcpRecipeParams } from './recipeParamRequests';
import { AppEvents } from '../constants/events';

type AcpConnection = {
  client: GooseAcpClient;
  initializeResponse: InitializeResponse;
};

type AcpRecoveryListener = (recovering: boolean) => void;

const ACP_INITIALIZE_TIMEOUT_MS = 10_000;
const ACP_RECONNECT_BASE_DELAY_MS = 500;
const ACP_RECONNECT_MAX_DELAY_MS = 30_000;
const ACP_V1_PROTOCOL_VERSION: 1 = PROTOCOL_VERSION;

let currentConnection: AcpConnection | null = null;
let pendingConnection: Promise<AcpConnection> | null = null;
let connectionGeneration = 0;
let recovering = false;
const recoveryListeners = new Set<AcpRecoveryListener>();

export async function getAcpClient(): Promise<GooseAcpClient> {
  return (await getConnection()).client;
}

export async function getAcpInitializeResponse(): Promise<InitializeResponse> {
  return (await getConnection()).initializeResponse;
}

export function reconnectAcpAfterSystemResume(): void {
  recoverConnection(true);
}

export function reconnectAcpToNewBackend(): void {
  connectionGeneration += 1;
  const previousConnection = currentConnection;
  currentConnection = null;
  pendingConnection = null;
  setRecovering(false);
  previousConnection?.client.connection.close();
  window.dispatchEvent(new CustomEvent(AppEvents.BACKEND_SWITCHED));
}

export function isAcpRecovering(): boolean {
  return recovering;
}

export function subscribeToAcpRecovery(listener: AcpRecoveryListener): () => void {
  recoveryListeners.add(listener);
  return () => {
    recoveryListeners.delete(listener);
  };
}

function setRecovering(nextRecovering: boolean): void {
  if (recovering === nextRecovering) {
    return;
  }

  recovering = nextRecovering;
  for (const listener of recoveryListeners) {
    listener(recovering);
  }
}

function recoverConnection(immediate: boolean): void {
  if (!currentConnection && !pendingConnection) {
    return;
  }

  setRecovering(true);
  const previousConnection = currentConnection;
  connectionGeneration += 1;
  currentConnection = null;
  pendingConnection = null;
  previousConnection?.client.connection.close();

  const generation = connectionGeneration;
  const recoveryAttempt = immediate
    ? openConnection(generation).catch((error) => {
        if (generation !== connectionGeneration || isGooseServeExitedError(error)) {
          throw error;
        }
        return retryWithBackoff(generation);
      })
    : retryWithBackoff(generation);
  pendingConnection = recoveryAttempt;
  void recoveryAttempt.then(
    () => {
      if (generation === connectionGeneration) {
        setRecovering(false);
      }
    },
    () => {
      if (pendingConnection === recoveryAttempt) {
        pendingConnection = null;
      }
      if (generation === connectionGeneration) {
        setRecovering(false);
      }
    }
  );
}

async function getConnection(): Promise<AcpConnection> {
  if (currentConnection) {
    return currentConnection;
  }

  if (!pendingConnection) {
    const generation = connectionGeneration;
    let connectionAttempt: Promise<AcpConnection>;
    connectionAttempt = openConnection(generation).catch((error) => {
      if (pendingConnection === connectionAttempt) {
        pendingConnection = null;
      }
      throw error;
    });
    pendingConnection = connectionAttempt;
  }

  return pendingConnection;
}

async function openConnection(generation: number): Promise<AcpConnection> {
  const wsUrl = await window.electron.getAcpUrl();
  if (!wsUrl) {
    throw new Error('ACP URL is not available');
  }

  // Electron treats an explicitly passed undefined protocol as a subprotocol.
  const stream = createWebSocketStream(wsUrl, { protocols: [] });
  const client = connectGooseAcpClient(stream, createClientCallbacks());

  try {
    const initializeResponse = await withTimeout(
      client.connection.agent.request(methods.agent.initialize, {
        protocolVersion: ACP_V1_PROTOCOL_VERSION,
        _meta: {
          'goose/useLoginShellPath': true,
        },
        clientCapabilities: {
          elicitation: { form: {} },
          _meta: {
            goose: {
              mcpHostCapabilities: DEFAULT_GOOSE_MCP_HOST_CAPABILITIES,
              customNotifications: true,
              recipeParameterRequests: true,
            },
          },
        },
        clientInfo: {
          name: packageJson.name,
          version: packageJson.version,
        },
      }),
      ACP_INITIALIZE_TIMEOUT_MS,
      `ACP initialize timed out after ${ACP_INITIALIZE_TIMEOUT_MS}ms`
    );

    if (generation !== connectionGeneration) {
      throw new Error('ACP connection attempt is no longer current');
    }

    const connection = { client, initializeResponse };
    currentConnection = connection;
    const handleClose = () => {
      if (currentConnection === connection) {
        recoverConnection(false);
      }
    };
    connection.client.connection.closed.then(handleClose, handleClose);
    return connection;
  } catch (error) {
    client.connection.close(error);
    throw error;
  }
}

async function retryWithBackoff(generation: number): Promise<AcpConnection> {
  for (let attempt = 0; generation === connectionGeneration; attempt += 1) {
    const maximumDelay = Math.min(
      ACP_RECONNECT_MAX_DELAY_MS,
      ACP_RECONNECT_BASE_DELAY_MS * 2 ** attempt
    );
    await delay(Math.floor(Math.random() * maximumDelay));

    if (generation !== connectionGeneration) {
      break;
    }

    try {
      return await openConnection(generation);
    } catch (error) {
      if (generation !== connectionGeneration || isGooseServeExitedError(error)) {
        throw error;
      }
    }
  }

  throw new Error('ACP connection attempt is no longer current');
}

function isGooseServeExitedError(error: unknown): boolean {
  return error instanceof Error && error.message.includes(GOOSE_SERVE_EXITED_USER_MESSAGE);
}

function delay(delayMs: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, delayMs));
}

function createClientCallbacks(): GooseAcpCallbacks {
  return {
    requestPermission: requestAcpPermission,
    unstable_createElicitation: requestAcpElicitation,
    unstable_sessionRecipeRequestParams: requestAcpRecipeParams,
    sessionUpdate: handleAcpSessionNotification,
    unstable_sessionUpdate: handleAcpGooseSessionNotification,
    unstable_providerDeviceCode: handleAcpProviderDeviceCodeNotification,
  };
}

async function withTimeout<T>(promise: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  let timeoutId: ReturnType<typeof setTimeout> | null = null;
  const timeout = new Promise<T>((_, reject) => {
    timeoutId = setTimeout(() => reject(new Error(message)), timeoutMs);
  });

  try {
    return await Promise.race([promise, timeout]);
  } finally {
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
    }
  }
}

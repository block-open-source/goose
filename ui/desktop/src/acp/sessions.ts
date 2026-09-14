import {
  methods,
  type ForkSessionRequest,
  type ListSessionsRequest,
  type LoadSessionResponse,
  type NewSessionRequest,
  type SessionInfo,
} from '@agentclientprotocol/sdk';
import type { GooseExtension, SessionExportFormat, SessionImportSource } from '@aaif/goose-acp-client';
import { getAcpClient } from './acpConnection';
import type { ExtensionLoadResult } from '../types/extensions';
import type { Session } from '../types/session';
import type { Recipe } from '../recipe';

interface GooseSessionInfoMeta {
  messageCount?: number;
  createdAt?: string;
  lastMessageAt?: string;
  archivedAt?: string;
  projectId?: string;
  providerId?: string;
  modelId?: string;
  sessionType?: Session['session_type'];
  userSetName?: boolean;
  hasRecipe?: boolean;
  lastMessageSnippet?: string;
}

export interface SessionListItem {
  id: string;
  name: string;
  workingDir: string;
  updatedAt: string;
  messageCount: number;
  lastMessageAt?: string;
  createdAt: string;
  archivedAt?: string;
  projectId?: string;
  providerId?: string;
  modelId?: string;
  userSetName?: boolean;
  hasRecipe?: boolean;
  sessionType?: Session['session_type'];
}

export interface SessionListPage {
  sessions: SessionListItem[];
  nextCursor: string | null;
}

export interface LoadSessionMeta {
  recipe?: Recipe | null;
  userRecipeValues?: Record<string, string> | null;
  extensionResults?: ExtensionLoadResult[] | null;
  workingDir?: string;
}

export interface AcpLoadSessionResult {
  sessionInfo: SessionInfo;
  response: LoadSessionResponse;
  meta: LoadSessionMeta;
}

const inFlightSessionLoads = new Map<string, Promise<AcpLoadSessionResult>>();

function parseSessionResponseMeta(rawMeta: unknown): LoadSessionMeta {
  const meta = (rawMeta ?? {}) as LoadSessionMeta;
  return {
    recipe: meta.recipe,
    userRecipeValues: meta.userRecipeValues,
    extensionResults: meta.extensionResults,
    workingDir: typeof meta.workingDir === 'string' ? meta.workingDir : undefined,
  };
}

export function parseLoadMeta(response: LoadSessionResponse): LoadSessionMeta {
  return parseSessionResponseMeta(response._meta);
}

function sessionInfoMeta(s: SessionInfo): GooseSessionInfoMeta {
  return (s._meta ?? {}) as GooseSessionInfoMeta;
}

export function sessionInfoToSession(s: SessionInfo, loadMeta: LoadSessionMeta = {}): Session {
  const meta = sessionInfoMeta(s);
  const createdAt = meta.createdAt ?? s.updatedAt ?? '';
  const updatedAt = s.updatedAt ?? createdAt;
  const modelConfig: Session['model_config'] = meta.modelId
    ? {
        model_name: meta.modelId,
        toolshim: false,
      }
    : null;

  return {
    id: String(s.sessionId),
    name: s.title ?? '',
    working_dir: loadMeta.workingDir ?? s.cwd,
    created_at: createdAt,
    updated_at: updatedAt,
    last_message_at: meta.lastMessageAt,
    message_count: meta.messageCount ?? 0,
    extension_data: {},
    archived_at: meta.archivedAt,
    project_id: meta.projectId,
    provider_name: meta.providerId,
    model_config: modelConfig,
    session_type: meta.sessionType,
    recipe: loadMeta.recipe as Session['recipe'],
    user_recipe_values: loadMeta.userRecipeValues,
    user_set_name: meta.userSetName,
    last_message_snippet: meta.lastMessageSnippet,
  };
}

function sessionInfoToListItem(s: SessionInfo): SessionListItem {
  const meta = sessionInfoMeta(s);
  return {
    id: String(s.sessionId),
    name: s.title ?? '',
    workingDir: s.cwd,
    updatedAt: s.updatedAt ?? '',
    messageCount: meta.messageCount ?? 0,
    lastMessageAt: meta.lastMessageAt,
    createdAt: meta.createdAt ?? s.updatedAt ?? '',
    archivedAt: meta.archivedAt,
    projectId: meta.projectId,
    providerId: meta.providerId,
    modelId: meta.modelId,
    userSetName: meta.userSetName,
    hasRecipe: meta.hasRecipe,
    sessionType: meta.sessionType,
  };
}

export interface SessionListFilter {
  keyword?: string;
  includeAcp: boolean;
}

const SESSION_LIST_TYPES = ['user', 'scheduled'] as const;
const SESSION_LIST_TYPES_WITH_ACP = [...SESSION_LIST_TYPES, 'acp'] as const;

export async function acpListSessions(
  cursor?: string | null,
  filter: SessionListFilter = { includeAcp: false }
): Promise<SessionListPage> {
  const client = await getAcpClient();
  const request: ListSessionsRequest = {};
  if (cursor) {
    request.cursor = cursor;
  }
  const meta: Record<string, unknown> = {
    types: filter.includeAcp ? SESSION_LIST_TYPES_WITH_ACP : SESSION_LIST_TYPES,
  };
  const keyword = filter.keyword?.trim();
  if (keyword) {
    meta.query = keyword;
  }
  request._meta = meta;
  const response = await client.connection.agent.request(methods.agent.session.list, request);
  return {
    sessions: response.sessions.map(sessionInfoToListItem),
    nextCursor: response.nextCursor ?? null,
  };
}

export async function acpListRecentSessions(maxSessions: number): Promise<SessionListItem[]> {
  if (maxSessions <= 0) {
    return [];
  }

  const client = await getAcpClient();
  const response = await client.connection.agent.request(methods.agent.session.list, {
    _meta: { types: SESSION_LIST_TYPES },
  });
  return response.sessions.slice(0, maxSessions).map(sessionInfoToListItem);
}

export async function acpGetSessionListItem(sessionId: string): Promise<SessionListItem> {
  const client = await getAcpClient();
  const response = await client.goose.sessionInfo_unstable({ sessionId });
  return sessionInfoToListItem(response.session);
}

export async function acpLoadSession(sessionId: string): Promise<AcpLoadSessionResult> {
  const pendingLoad = inFlightSessionLoads.get(sessionId);
  if (pendingLoad) {
    return pendingLoad;
  }

  const loadPromise = loadAcpSession(sessionId);
  inFlightSessionLoads.set(sessionId, loadPromise);
  try {
    return await loadPromise;
  } finally {
    if (inFlightSessionLoads.get(sessionId) === loadPromise) {
      inFlightSessionLoads.delete(sessionId);
    }
  }
}

export function isAcpSessionLoadInFlight(sessionId: string): boolean {
  return inFlightSessionLoads.has(sessionId);
}

async function loadAcpSession(sessionId: string): Promise<AcpLoadSessionResult> {
  const client = await getAcpClient();
  const initialSessionInfoResponse = await client.goose.sessionInfo_unstable({ sessionId });
  const initialSessionInfo = initialSessionInfoResponse.session;
  const useLegacyAgentLoop = await window.electron.getSetting('useLegacyAgentLoop');
  const response = await client.connection.agent.request(methods.agent.session.load, {
    sessionId,
    cwd: initialSessionInfo.cwd,
    mcpServers: [],
    _meta: { goose: { unrolledAgentLoop: !useLegacyAgentLoop } },
  });
  // Loading can populate missing provider/model metadata.
  const sessionInfoResponse = await client.goose.sessionInfo_unstable({ sessionId });

  return {
    sessionInfo: sessionInfoResponse.session,
    response,
    meta: parseLoadMeta(response),
  };
}

export interface AcpNewSessionResult {
  sessionId: string;
  sessionInfo: SessionInfo;
  meta: LoadSessionMeta;
}

export interface AcpRecipeOptions {
  recipeId?: string;
  recipeDeeplink?: string;
  recipeParameterScopeId?: string;
}

export async function acpNewSession(
  cwd: string,
  gooseExtensions: GooseExtension[],
  recipe?: AcpRecipeOptions
): Promise<AcpNewSessionResult> {
  const client = await getAcpClient();
  const meta: Record<string, unknown> = { client: 'goose-desktop' };
  if (gooseExtensions.length > 0) {
    meta.enabledExtensions = gooseExtensions;
  }
  if (recipe?.recipeId) {
    meta.recipeId = recipe.recipeId;
  } else if (recipe?.recipeDeeplink) {
    meta.recipeDeeplink = recipe.recipeDeeplink;
  }
  if (recipe?.recipeParameterScopeId) {
    meta.recipeParameterScopeId = recipe.recipeParameterScopeId;
  }
  const request: NewSessionRequest = { cwd, mcpServers: [], _meta: meta };
  const response = await client.connection.agent.request(methods.agent.session.new, request);
  const sessionId = String(response.sessionId);
  const sessionInfoResponse = await client.goose.sessionInfo_unstable({ sessionId });

  return {
    sessionId,
    sessionInfo: sessionInfoResponse.session,
    meta: parseSessionResponseMeta(response._meta),
  };
}

export async function acpDeleteSession(sessionId: string): Promise<void> {
  const client = await getAcpClient();
  await client.connection.agent.request(methods.agent.session.delete, { sessionId });
}

export async function acpCloseSession(sessionId: string): Promise<void> {
  const client = await getAcpClient();
  await client.connection.agent.request(methods.agent.session.close, { sessionId });
}

export async function acpRenameSession(sessionId: string, title: string): Promise<void> {
  const client = await getAcpClient();
  await client.goose.sessionRename_unstable({ sessionId, title });
}

export async function acpUpdateWorkingDir(sessionId: string, workingDir: string): Promise<void> {
  const client = await getAcpClient();
  await client.goose.sessionWorkingDirUpdate_unstable({ sessionId, workingDir });
}

export async function acpTruncateSessionConversation(
  sessionId: string,
  truncateFrom: number
): Promise<void> {
  const client = await getAcpClient();
  await client.goose.sessionConversationTruncate_unstable({ sessionId, truncateFrom });
}

export async function acpForkSession(
  sessionId: string,
  conversationBefore?: number
): Promise<string> {
  const client = await getAcpClient();
  const sessionInfo = await client.goose.sessionInfo_unstable({ sessionId });
  const { cwd } = sessionInfo.session;
  const request: ForkSessionRequest = { sessionId, cwd };
  if (conversationBefore !== undefined) {
    request._meta = { conversationBefore };
  }
  const response = await client.connection.agent.request(methods.agent.session.fork, request);
  return String(response.sessionId);
}

export async function acpExportSession(
  sessionId: string,
  format: SessionExportFormat = 'json'
): Promise<string> {
  const client = await getAcpClient();
  const response = await client.goose.sessionExport_unstable({ sessionId, format });
  return response.data;
}

export async function acpImportSession(input: string, source: SessionImportSource): Promise<void> {
  const client = await getAcpClient();
  await client.goose.sessionImport_unstable({ input, source });
}

export async function acpShareSessionNostr(sessionId: string, relays: string[]) {
  const client = await getAcpClient();
  return await client.goose.sessionShareNostr_unstable({ sessionId, relays });
}

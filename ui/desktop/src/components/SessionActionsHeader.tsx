import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { flushSync } from 'react-dom';
import {
  Activity,
  ChevronDown,
  ChevronRight,
  Copy,
  Edit2,
  FileJson,
  LoaderCircle,
} from 'lucide-react';
import { toast } from 'react-toastify';
import { AppEvents } from '../constants/events';
import { defineMessages, useIntl } from '../i18n';
import { getDiagnosticsReport } from '../acp/diagnostics';
import { acpExportSession, acpForkSession, acpRenameSession } from '../acp/sessions';
import { getSessionDisplayName } from '../sessions';
import type { Session } from '../types/session';
import { errorMessage } from '../utils/conversionUtils';
import { cn } from '../utils';
import { Button } from './ui/button';
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from './ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from './ui/dropdown-menu';

const i18n = defineMessages({
  actionsLabel: {
    id: 'sessionActionsHeader.actionsLabel',
    defaultMessage: 'Session actions',
  },
  renameSession: {
    id: 'sessionActionsHeader.renameSession',
    defaultMessage: 'Rename session',
  },
  duplicateSession: {
    id: 'sessionActionsHeader.duplicateSession',
    defaultMessage: 'Duplicate session',
  },
  viewJson: {
    id: 'sessionActionsHeader.viewJson',
    defaultMessage: 'View session JSON',
  },
  viewModelInteractions: {
    id: 'sessionActionsHeader.viewModelInteractions',
    defaultMessage: 'View recent model interactions',
  },
  renameTitle: {
    id: 'sessionActionsHeader.renameTitle',
    defaultMessage: 'Rename Session',
  },
  renamePlaceholder: {
    id: 'sessionActionsHeader.renamePlaceholder',
    defaultMessage: 'Enter session name',
  },
  cancel: {
    id: 'sessionActionsHeader.cancel',
    defaultMessage: 'Cancel',
  },
  save: {
    id: 'sessionActionsHeader.save',
    defaultMessage: 'Save',
  },
  saving: {
    id: 'sessionActionsHeader.saving',
    defaultMessage: 'Saving...',
  },
  renamed: {
    id: 'sessionActionsHeader.renamed',
    defaultMessage: 'Session renamed',
  },
  renameFailed: {
    id: 'sessionActionsHeader.renameFailed',
    defaultMessage: 'Failed to rename session: {error}',
  },
  duplicated: {
    id: 'sessionActionsHeader.duplicated',
    defaultMessage: 'Session duplicated',
  },
  duplicateFailed: {
    id: 'sessionActionsHeader.duplicateFailed',
    defaultMessage: 'Failed to duplicate session: {error}',
  },
  jsonTitle: {
    id: 'sessionActionsHeader.jsonTitle',
    defaultMessage: 'Session JSON',
  },
  modelInteractionsTitle: {
    id: 'sessionActionsHeader.modelInteractionsTitle',
    defaultMessage: 'Recent model interactions',
  },
  loadingJson: {
    id: 'sessionActionsHeader.loadingJson',
    defaultMessage: 'Loading JSON...',
  },
  jsonFailed: {
    id: 'sessionActionsHeader.jsonFailed',
    defaultMessage: 'Failed to load session JSON: {error}',
  },
  modelInteractionsFailed: {
    id: 'sessionActionsHeader.modelInteractionsFailed',
    defaultMessage: 'Failed to load model interactions: {error}',
  },
  close: {
    id: 'sessionActionsHeader.close',
    defaultMessage: 'Close',
  },
  copyJson: {
    id: 'sessionActionsHeader.copyJson',
    defaultMessage: 'Copy JSON',
  },
  copiedJson: {
    id: 'sessionActionsHeader.copiedJson',
    defaultMessage: 'Session JSON copied',
  },
  copiedModelInteractions: {
    id: 'sessionActionsHeader.copiedModelInteractions',
    defaultMessage: 'Recent model interactions copied',
  },
  fullTextTitle: {
    id: 'sessionActionsHeader.fullTextTitle',
    defaultMessage: 'Text value',
  },
  copyText: {
    id: 'sessionActionsHeader.copyText',
    defaultMessage: 'Copy text',
  },
  copiedText: {
    id: 'sessionActionsHeader.copiedText',
    defaultMessage: 'Text copied',
  },
});

const LONG_STRING_THRESHOLD = 180;
const STRING_PREVIEW_START = 96;
const STRING_PREVIEW_END = 56;

interface SessionActionsHeaderProps {
  session?: Session;
  onSessionChange: (updater: (session: Session) => Session) => void;
  className?: string;
}

interface ParsedSessionJson {
  value: unknown;
  pretty: string;
}

interface FullTextSelection {
  path: string;
  value: string;
}

type JsonDialogKind = 'session' | 'modelInteractions';

function parseSessionJson(json: string): ParsedSessionJson {
  const value = JSON.parse(json) as unknown;
  return {
    value,
    pretty: JSON.stringify(value, null, 2),
  };
}

function parseJsonLine(line: string): unknown {
  try {
    return JSON.parse(line) as unknown;
  } catch {
    return line;
  }
}

function isJsonRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function getNodePath(parentPath: string, key: string, isArrayItem: boolean): string {
  if (isArrayItem) {
    return `${parentPath}[${key}]`;
  }

  return /^[A-Za-z_$][\w$]*$/.test(key)
    ? `${parentPath}.${key}`
    : `${parentPath}[${JSON.stringify(key)}]`;
}

function getStringPreview(value: string): string {
  if (value.length <= LONG_STRING_THRESHOLD) {
    return JSON.stringify(value);
  }

  return JSON.stringify(
    `${value.slice(0, STRING_PREVIEW_START)} ... ${value.slice(-STRING_PREVIEW_END)}`
  );
}

function JsonPrimitiveValue({
  value,
  path,
  onOpenText,
}: {
  value: unknown;
  path: string;
  onOpenText: (selection: FullTextSelection) => void;
}) {
  if (typeof value === 'string') {
    const isLong = value.length > LONG_STRING_THRESHOLD;
    const preview = getStringPreview(value);

    if (isLong) {
      return (
        <button
          type="button"
          className="min-w-0 rounded-sm text-left text-blue-600 underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-border-active dark:text-blue-300 break-all"
          onClick={() => onOpenText({ path, value })}
          title={path}
        >
          {preview}
        </button>
      );
    }

    return (
      <span className="min-w-0 text-emerald-700 dark:text-emerald-300 break-all">{preview}</span>
    );
  }

  if (typeof value === 'number') {
    return <span className="text-purple-700 dark:text-purple-300">{value}</span>;
  }

  if (typeof value === 'boolean') {
    return <span className="text-amber-700 dark:text-amber-300">{String(value)}</span>;
  }

  if (value === null) {
    return <span className="text-text-secondary">null</span>;
  }

  return <span className="text-text-secondary">{String(value)}</span>;
}

function JsonTreeNode({
  label,
  value,
  depth,
  path,
  isArrayItem = false,
  onOpenText,
}: {
  label?: string;
  value: unknown;
  depth: number;
  path: string;
  isArrayItem?: boolean;
  onOpenText: (selection: FullTextSelection) => void;
}) {
  const isArray = Array.isArray(value);
  const isRecord = isJsonRecord(value);
  const isContainer = isArray || isRecord;
  const [isOpen, setIsOpen] = useState(depth < 3);

  const labelNode =
    label === undefined ? null : (
      <span className="text-text-secondary">{isArrayItem ? label : JSON.stringify(label)}:</span>
    );

  if (!isContainer) {
    return (
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-1 px-1 py-0.5">
        {labelNode}
        <JsonPrimitiveValue value={value} path={path} onOpenText={onOpenText} />
      </div>
    );
  }

  const entries = isArray
    ? value.map((item, index) => [String(index), item] as const)
    : Object.entries(value);
  const openToken = isArray ? '[' : '{';
  const closeToken = isArray ? ']' : '}';
  const countLabel = isArray
    ? `${entries.length} ${entries.length === 1 ? 'item' : 'items'}`
    : `${entries.length} ${entries.length === 1 ? 'key' : 'keys'}`;

  return (
    <div className="min-w-0">
      <button
        type="button"
        className="flex max-w-full items-baseline gap-1 rounded-sm px-1 py-0.5 text-left transition-colors hover:bg-background-primary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-border-active"
        onClick={() => entries.length > 0 && setIsOpen((open) => !open)}
      >
        {entries.length > 0 ? (
          isOpen ? (
            <ChevronDown className="mt-0.5 size-3.5 shrink-0 text-text-secondary" />
          ) : (
            <ChevronRight className="mt-0.5 size-3.5 shrink-0 text-text-secondary" />
          )
        ) : (
          <span className="size-3.5 shrink-0" />
        )}
        <span className="min-w-0 flex flex-wrap items-baseline gap-x-1">
          {labelNode}
          <span>{openToken}</span>
          {!isOpen && entries.length > 0 && (
            <span className="text-text-secondary">{countLabel}</span>
          )}
          {(!isOpen || entries.length === 0) && <span>{closeToken}</span>}
        </span>
      </button>

      {isOpen && entries.length > 0 && (
        <div className="ml-3 border-l border-border-primary/70 pl-3">
          {entries.map(([key, childValue]) => (
            <JsonTreeNode
              key={`${path}.${key}`}
              label={key}
              value={childValue}
              depth={depth + 1}
              path={getNodePath(path, key, isArray)}
              isArrayItem={isArray}
              onOpenText={onOpenText}
            />
          ))}
          <div className="px-1 py-0.5">{closeToken}</div>
        </div>
      )}
    </div>
  );
}

function JsonTree({
  value,
  onOpenText,
}: {
  value: unknown;
  onOpenText: (selection: FullTextSelection) => void;
}) {
  return (
    <div className="min-w-0 font-mono text-xs leading-5 text-text-primary">
      <JsonTreeNode value={value} depth={0} path="root" onOpenText={onOpenText} />
    </div>
  );
}

export default function SessionActionsHeader({
  session,
  onSessionChange,
  className,
}: SessionActionsHeaderProps) {
  const intl = useIntl();
  const [isRenameOpen, setIsRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState('');
  const [isRenaming, setIsRenaming] = useState(false);
  const [isJsonOpen, setIsJsonOpen] = useState(false);
  const [jsonDialogKind, setJsonDialogKind] = useState<JsonDialogKind>('session');
  const [jsonValue, setJsonValue] = useState<unknown>(null);
  const [jsonText, setJsonText] = useState('');
  const [isJsonLoading, setIsJsonLoading] = useState(false);
  const [isModelInteractionsLoading, setIsModelInteractionsLoading] = useState(false);
  const [isDuplicating, setIsDuplicating] = useState(false);
  const [fullTextSelection, setFullTextSelection] = useState<FullTextSelection | null>(null);
  const jsonLoadRequestId = useRef(0);

  const title = useMemo(() => (session ? getSessionDisplayName(session) : ''), [session]);

  useEffect(() => {
    if (session && isRenameOpen) {
      setRenameValue(getSessionDisplayName(session));
    }
  }, [isRenameOpen, session]);

  const handleRename = useCallback(async () => {
    if (!session || isRenaming) return;

    const trimmedName = renameValue.trim();
    if (!trimmedName) return;

    if (trimmedName === session.name) {
      setIsRenameOpen(false);
      return;
    }

    setIsRenaming(true);
    try {
      await acpRenameSession(session.id, trimmedName);
      flushSync(() => setIsRenameOpen(false));
      document.body.style.removeProperty('pointer-events');
      onSessionChange((current) => ({ ...current, name: trimmedName, user_set_name: true }));
      window.dispatchEvent(
        new CustomEvent(AppEvents.SESSION_RENAMED, {
          detail: { sessionId: session.id, newName: trimmedName, userInitiated: true },
        })
      );
      toast.success(intl.formatMessage(i18n.renamed));
    } catch (error) {
      toast.error(
        intl.formatMessage(i18n.renameFailed, {
          error: errorMessage(error, 'Unknown error'),
        })
      );
    } finally {
      setIsRenaming(false);
    }
  }, [intl, isRenaming, onSessionChange, renameValue, session]);

  const handleDuplicate = useCallback(async () => {
    if (!session || isDuplicating) return;

    setIsDuplicating(true);
    try {
      await acpForkSession(session.id);
      window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
      toast.success(intl.formatMessage(i18n.duplicated));
    } catch (error) {
      toast.error(
        intl.formatMessage(i18n.duplicateFailed, {
          error: errorMessage(error, 'Unknown error'),
        })
      );
    } finally {
      setIsDuplicating(false);
    }
  }, [intl, isDuplicating, session]);

  const handleViewJson = useCallback(async () => {
    if (!session) return;

    const loadRequestId = ++jsonLoadRequestId.current;
    setIsJsonOpen(true);
    setJsonDialogKind('session');
    setJsonValue(null);
    setJsonText('');
    setIsModelInteractionsLoading(false);
    setIsJsonLoading(true);
    try {
      const json = await acpExportSession(session.id);
      const parsed = parseSessionJson(json);
      if (jsonLoadRequestId.current !== loadRequestId) return;
      setJsonValue(parsed.value);
      setJsonText(parsed.pretty);
    } catch (error) {
      if (jsonLoadRequestId.current !== loadRequestId) return;
      setIsJsonOpen(false);
      toast.error(
        intl.formatMessage(i18n.jsonFailed, {
          error: errorMessage(error, 'Unknown error'),
        })
      );
    } finally {
      if (jsonLoadRequestId.current === loadRequestId) {
        setIsJsonLoading(false);
      }
    }
  }, [intl, session]);

  const handleViewModelInteractions = useCallback(async () => {
    if (!session) return;

    const loadRequestId = ++jsonLoadRequestId.current;
    setIsJsonOpen(true);
    setJsonDialogKind('modelInteractions');
    setJsonValue(null);
    setJsonText('');
    setIsJsonLoading(false);
    setIsModelInteractionsLoading(true);
    try {
      const report = await getDiagnosticsReport(session.id, 'full');
      const interactions = report.logs.llm.map((log) => ({
        path: log.path,
        truncated: log.truncated,
        entries: log.content
          .split('\n')
          .filter((line) => line.trim().length > 0)
          .map(parseJsonLine),
      }));
      const text = JSON.stringify(interactions, null, 2);
      if (jsonLoadRequestId.current !== loadRequestId) return;
      setJsonValue(interactions);
      setJsonText(text);
    } catch (error) {
      if (jsonLoadRequestId.current !== loadRequestId) return;
      setIsJsonOpen(false);
      toast.error(
        intl.formatMessage(i18n.modelInteractionsFailed, {
          error: errorMessage(error, 'Unknown error'),
        })
      );
    } finally {
      if (jsonLoadRequestId.current === loadRequestId) {
        setIsModelInteractionsLoading(false);
      }
    }
  }, [intl, session]);

  const handleCopyJson = useCallback(async () => {
    if (!jsonText) return;
    await navigator.clipboard.writeText(jsonText);
    toast.success(
      intl.formatMessage(
        jsonDialogKind === 'modelInteractions' ? i18n.copiedModelInteractions : i18n.copiedJson
      )
    );
  }, [intl, jsonDialogKind, jsonText]);

  const handleCopyFullText = useCallback(async () => {
    if (!fullTextSelection) return;
    await navigator.clipboard.writeText(fullTextSelection.value);
    toast.success(intl.formatMessage(i18n.copiedText));
  }, [fullTextSelection, intl]);

  const handleJsonOpenChange = useCallback((open: boolean) => {
    setIsJsonOpen(open);
    if (!open) {
      jsonLoadRequestId.current += 1;
      setIsJsonLoading(false);
      setIsModelInteractionsLoading(false);
      setFullTextSelection(null);
    }
  }, []);

  const handleRenameKeyDown = useCallback(
    (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === 'Enter') {
        void handleRename();
      }
    },
    [handleRename]
  );

  if (!session) {
    return null;
  }

  const isDialogOpen = isRenameOpen || isJsonOpen || fullTextSelection !== null;

  return (
    <>
      <div
        className={cn(
          'no-drag absolute top-[14px] left-1/2 max-w-[min(36rem,calc(100vw-13rem))] -translate-x-1/2',
          isDialogOpen ? 'z-30' : 'z-50',
          className
        )}
      >
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="no-drag flex h-7 max-w-full items-center gap-1 rounded-md px-2.5 text-text-primary transition-colors hover:bg-background-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-border-active"
              aria-label={intl.formatMessage(i18n.actionsLabel)}
            >
              <span className="truncate text-xs font-medium">{title}</span>
              <ChevronDown className="size-3.5 text-text-secondary" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="center" className="w-56">
            <DropdownMenuItem onSelect={() => setTimeout(() => setIsRenameOpen(true), 0)}>
              <Edit2 className="size-4" />
              {intl.formatMessage(i18n.renameSession)}
            </DropdownMenuItem>
            <DropdownMenuItem disabled={isDuplicating} onSelect={() => void handleDuplicate()}>
              {isDuplicating ? (
                <LoaderCircle className="size-4 animate-spin" />
              ) : (
                <Copy className="size-4" />
              )}
              {intl.formatMessage(i18n.duplicateSession)}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void handleViewJson()}>
              {isJsonLoading ? (
                <LoaderCircle className="size-4 animate-spin" />
              ) : (
                <FileJson className="size-4" />
              )}
              {intl.formatMessage(i18n.viewJson)}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void handleViewModelInteractions()}>
              {isModelInteractionsLoading ? (
                <LoaderCircle className="size-4 animate-spin" />
              ) : (
                <Activity className="size-4" />
              )}
              {intl.formatMessage(i18n.viewModelInteractions)}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <Dialog open={isRenameOpen} onOpenChange={setIsRenameOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{intl.formatMessage(i18n.renameTitle)}</DialogTitle>
          </DialogHeader>
          <input
            type="text"
            value={renameValue}
            onChange={(event) => setRenameValue(event.target.value)}
            onKeyDown={handleRenameKeyDown}
            placeholder={intl.formatMessage(i18n.renamePlaceholder)}
            className="w-full rounded-lg border border-border-primary bg-background-primary p-3 text-text-primary outline-none focus:ring-2 focus:ring-border-active"
            disabled={isRenaming}
            maxLength={200}
            autoFocus
          />
          <DialogFooter>
            <Button variant="outline" onClick={() => setIsRenameOpen(false)} disabled={isRenaming}>
              {intl.formatMessage(i18n.cancel)}
            </Button>
            <Button
              onClick={() => void handleRename()}
              disabled={isRenaming || !renameValue.trim()}
            >
              {isRenaming ? intl.formatMessage(i18n.saving) : intl.formatMessage(i18n.save)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={isJsonOpen} onOpenChange={handleJsonOpenChange}>
        <DialogContent className="grid max-h-[85vh] grid-rows-[auto_minmax(0,1fr)_auto] sm:max-w-4xl">
          <DialogHeader>
            <DialogTitle>
              {intl.formatMessage(
                jsonDialogKind === 'modelInteractions'
                  ? i18n.modelInteractionsTitle
                  : i18n.jsonTitle
              )}
            </DialogTitle>
          </DialogHeader>
          <div className="min-h-0 overflow-hidden rounded-lg border border-border-primary bg-background-secondary">
            {isJsonLoading || isModelInteractionsLoading ? (
              <div className="flex h-64 items-center justify-center gap-2 text-sm text-text-secondary">
                <LoaderCircle className="size-4 animate-spin" />
                {intl.formatMessage(i18n.loadingJson)}
              </div>
            ) : (
              <div className="max-h-[60vh] overflow-auto p-3">
                <JsonTree value={jsonValue} onOpenText={setFullTextSelection} />
              </div>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setIsJsonOpen(false)}>
              {intl.formatMessage(i18n.close)}
            </Button>
            <Button
              onClick={() => void handleCopyJson()}
              disabled={!jsonText || isJsonLoading || isModelInteractionsLoading}
            >
              {intl.formatMessage(i18n.copyJson)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={!!fullTextSelection}
        onOpenChange={(open) => !open && setFullTextSelection(null)}
      >
        <DialogContent className="grid max-h-[80vh] grid-rows-[auto_minmax(0,1fr)_auto] sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>{intl.formatMessage(i18n.fullTextTitle)}</DialogTitle>
          </DialogHeader>
          {fullTextSelection && (
            <div className="min-h-0 space-y-3">
              <code className="block truncate rounded-md bg-background-secondary px-3 py-2 text-xs text-text-secondary">
                {fullTextSelection.path}
              </code>
              <pre className="max-h-[55vh] overflow-auto whitespace-pre-wrap break-words rounded-lg border border-border-primary bg-background-secondary p-4 text-xs leading-5 text-text-primary">
                {fullTextSelection.value}
              </pre>
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setFullTextSelection(null)}>
              {intl.formatMessage(i18n.close)}
            </Button>
            <Button onClick={() => void handleCopyFullText()} disabled={!fullTextSelection}>
              {intl.formatMessage(i18n.copyText)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

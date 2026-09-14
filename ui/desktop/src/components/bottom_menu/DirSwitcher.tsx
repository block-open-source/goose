import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Check, FolderDot, FolderOpen, GitBranch, Plus } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../ui/Tooltip';
import { Input } from '../ui/input';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '../ui/dropdown-menu';
import { toast } from 'react-toastify';
import { defineMessages, useIntl } from '../../i18n';

const i18n = defineMessages({
  failedToUpdateWorkingDir: {
    id: 'dirSwitcher.failedToUpdateWorkingDir',
    defaultMessage: 'Failed to update working directory',
  },
  currentDirectory: {
    id: 'dirSwitcher.currentDirectory',
    defaultMessage: 'Current directory',
  },
  gitWorktrees: {
    id: 'dirSwitcher.gitWorktrees',
    defaultMessage: 'Git worktrees',
  },
  recentDirectories: {
    id: 'dirSwitcher.recentDirectories',
    defaultMessage: 'Recent directories',
  },
  chooseDirectory: {
    id: 'dirSwitcher.chooseDirectory',
    defaultMessage: 'Choose directory…',
  },
  openInFinder: {
    id: 'dirSwitcher.openInFinder',
    defaultMessage: 'Open in file manager',
  },
  noWorktreesFound: {
    id: 'dirSwitcher.noWorktreesFound',
    defaultMessage: 'No worktrees found',
  },
  enterPath: {
    id: 'dirSwitcher.enterPath',
    defaultMessage: 'Enter path',
  },
  enterPathPlaceholder: {
    id: 'dirSwitcher.enterPathPlaceholder',
    defaultMessage: 'Enter an absolute path (e.g. /home/goose/workspace)',
  },
  enterPathInvalid: {
    id: 'dirSwitcher.enterPathInvalid',
    defaultMessage: 'Working directory must be an absolute path',
  },
});

const isAbsolutePath = (p: string): boolean =>
  p.startsWith('/') || p.startsWith('\\\\') || /^[A-Za-z]:[\\/]/.test(p);

const splitDirPath = (dir: string): { name: string; parent: string } => {
  const normalized = dir.replace(/[\\/]+$/, '');
  const parts = normalized.split(/[\\/]/);
  const name = parts.pop() || dir;
  const parent = parts.join('/');
  return { name, parent };
};

const DirNameLabel: React.FC<{ dir: string }> = ({ dir }) => {
  const { name, parent } = splitDirPath(dir);
  return (
    <div className="flex flex-col min-w-0 flex-1">
      <span className="truncate text-sm text-text-primary">{name}</span>
      {parent && <span className="truncate text-xs text-text-secondary/70">{parent}</span>}
    </div>
  );
};

interface DirSwitcherProps {
  className: string;
  sessionId: string | undefined;
  workingDir: string;
  onWorkingDirChange?: (newDir: string) => Promise<void> | void;
  onRestartStart?: () => void;
  onRestartEnd?: () => void;
}

export const DirSwitcher: React.FC<DirSwitcherProps> = ({
  className,
  sessionId,
  workingDir,
  onWorkingDirChange,
  onRestartStart,
  onRestartEnd,
}) => {
  const intl = useIntl();
  const [isTooltipOpen, setIsTooltipOpen] = useState(false);
  const [isDirectoryChooserOpen, setIsDirectoryChooserOpen] = useState(false);
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const [recentDirs, setRecentDirs] = useState<string[]>([]);
  const [worktreeDirs, setWorktreeDirs] = useState<string[]>([]);
  const [customDirInput, setCustomDirInput] = useState('');
  const refreshVersionRef = useRef(0);

  const refreshMenuData = useCallback(async () => {
    const version = ++refreshVersionRef.current;
    setRecentDirs([]);
    setWorktreeDirs([]);

    const [recent, worktrees] = await Promise.all([
      window.electron.listRecentDirs().catch(() => []),
      window.electron.listGitWorktreeDirs(workingDir).catch(() => []),
    ]);

    if (version !== refreshVersionRef.current) return;

    setRecentDirs(recent);
    setWorktreeDirs(worktrees);
  }, [workingDir]);

  useEffect(() => {
    if (!isMenuOpen) {
      return;
    }

    void refreshMenuData();
  }, [isMenuOpen, refreshMenuData]);

  const applyDirectoryChange = async (newDir: string) => {
    if (sessionId) {
      onRestartStart?.();

      try {
        await onWorkingDirChange?.(newDir);
      } catch (error) {
        console.error('[DirSwitcher] Failed to update working directory:', error);
        toast.error(intl.formatMessage(i18n.failedToUpdateWorkingDir));
        return;
      } finally {
        onRestartEnd?.();
      }
    } else {
      await onWorkingDirChange?.(newDir);
    }

    // Only record the directory after the backend confirmed the change, so a
    // rejected path does not pollute the recent-directories list.
    window.electron.addRecentDir(newDir);
    setRecentDirs((previous) => [newDir, ...previous.filter((dir) => dir !== newDir)].slice(0, 10));
  };

  const handleDirectoryChange = async () => {
    if (isDirectoryChooserOpen) return;
    setIsDirectoryChooserOpen(true);

    let result;
    try {
      result = await window.electron.directoryChooser();
    } finally {
      setIsDirectoryChooserOpen(false);
    }

    if (result.canceled || result.filePaths.length === 0) {
      return;
    }

    const newDir = result.filePaths[0];
    await applyDirectoryChange(newDir);
  };

  const handleSelectDirectory = async (newDir: string) => {
    if (newDir === workingDir) {
      setIsMenuOpen(false);
      return;
    }

    setIsMenuOpen(false);
    await applyDirectoryChange(newDir);
  };

  const handleDirectoryClick = async (event: React.MouseEvent) => {
    if (isDirectoryChooserOpen) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }

    const isCmdOrCtrlClick = event.metaKey || event.ctrlKey;

    if (isCmdOrCtrlClick) {
      event.preventDefault();
      event.stopPropagation();
      await window.electron.openDirectoryInExplorer(workingDir);
    }
  };

  const filteredWorktreeDirs = useMemo(
    () => worktreeDirs.filter((dir) => dir && dir !== workingDir),
    [worktreeDirs, workingDir]
  );

  const filteredRecentDirs = useMemo(
    () => recentDirs.filter((dir) => dir && dir !== workingDir),
    [recentDirs, workingDir]
  );

  return (
    <TooltipProvider>
      <Tooltip
        open={isTooltipOpen && !isDirectoryChooserOpen && !isMenuOpen}
        onOpenChange={(open) => {
          if (!isDirectoryChooserOpen && !isMenuOpen) setIsTooltipOpen(open);
        }}
      >
        <DropdownMenu open={isMenuOpen} onOpenChange={setIsMenuOpen}>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <button
                className={`z-[100] ${isDirectoryChooserOpen ? 'opacity-50' : 'hover:cursor-pointer hover:text-text-primary'} text-text-primary/70 text-xs flex items-center transition-colors pl-1 [&>svg]:size-4 ${className}`}
                onClick={handleDirectoryClick}
                disabled={isDirectoryChooserOpen}
              >
                <FolderDot className="mr-1" size={16} />
                <div className="max-w-[200px] truncate">
                  {workingDir.replace(/\/+$/, '').split('/').pop() || workingDir}
                </div>
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <DropdownMenuContent className="w-[28rem]" side="top" align="start">
            <DropdownMenuLabel>{intl.formatMessage(i18n.currentDirectory)}</DropdownMenuLabel>
            <DropdownMenuItem
              onSelect={() => void window.electron.openDirectoryInExplorer(workingDir)}
            >
              <FolderOpen className="mr-2 h-4 w-4 flex-shrink-0" />
              <DirNameLabel dir={workingDir} />
              <Check className="ml-auto h-4 w-4 flex-shrink-0" />
            </DropdownMenuItem>

            <DropdownMenuSeparator />
            <DropdownMenuLabel>{intl.formatMessage(i18n.enterPath)}</DropdownMenuLabel>
            <div className="px-2 py-1.5">
              <Input
                value={customDirInput}
                placeholder={intl.formatMessage(i18n.enterPathPlaceholder)}
                onChange={(e) => setCustomDirInput(e.target.value)}
                onKeyDown={(e) => {
                  // Stop Radix menu typeahead from stealing focus while typing
                  // a path (e.g. the initial "C" of a Windows drive path).
                  e.stopPropagation();
                  if (e.key === 'Enter' && customDirInput.trim()) {
                    const newDir = customDirInput.trim();
                    if (!isAbsolutePath(newDir)) {
                      toast.error(intl.formatMessage(i18n.enterPathInvalid));
                      return;
                    }
                    setCustomDirInput('');
                    setIsMenuOpen(false);
                    void applyDirectoryChange(newDir);
                  }
                }}
              />
            </div>

            <DropdownMenuSeparator />
            <DropdownMenuLabel>{intl.formatMessage(i18n.gitWorktrees)}</DropdownMenuLabel>
            {filteredWorktreeDirs.length > 0 ? (
              filteredWorktreeDirs.map((dir) => (
                <DropdownMenuItem
                  key={`worktree-${dir}`}
                  onSelect={() => void handleSelectDirectory(dir)}
                >
                  <GitBranch className="mr-2 h-4 w-4 flex-shrink-0" />
                  <DirNameLabel dir={dir} />
                </DropdownMenuItem>
              ))
            ) : (
              <DropdownMenuItem disabled>
                <GitBranch className="mr-2 h-4 w-4" />
                <span>{intl.formatMessage(i18n.noWorktreesFound)}</span>
              </DropdownMenuItem>
            )}

            {filteredRecentDirs.length > 0 && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuLabel>{intl.formatMessage(i18n.recentDirectories)}</DropdownMenuLabel>
                {filteredRecentDirs.map((dir) => (
                  <DropdownMenuItem
                    key={`recent-${dir}`}
                    onSelect={() => void handleSelectDirectory(dir)}
                  >
                    <FolderDot className="mr-2 h-4 w-4 flex-shrink-0" />
                    <DirNameLabel dir={dir} />
                  </DropdownMenuItem>
                ))}
              </>
            )}

            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void handleDirectoryChange()}>
              <Plus className="mr-2 h-4 w-4" />
              <span>{intl.formatMessage(i18n.chooseDirectory)}</span>
            </DropdownMenuItem>
            <DropdownMenuItem
              onSelect={() => void window.electron.openDirectoryInExplorer(workingDir)}
            >
              <FolderOpen className="mr-2 h-4 w-4" />
              <span>{intl.formatMessage(i18n.openInFinder)}</span>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <TooltipContent side="top">{workingDir}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
};

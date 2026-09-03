let leasedWorkingDir: string | null = null;

export const getInitialWorkingDir = (): string =>
  leasedWorkingDir ?? (window.appConfig?.get('GOOSE_WORKING_DIR') as string) ?? '';

export const setWorkingDir = (workingDir: string | null): void => {
  leasedWorkingDir = workingDir?.trim() ? workingDir : null;
};

export const refreshWorkingDir = async (): Promise<string> => {
  try {
    const workingDir = await window.electron.getWorkingDir();
    if (workingDir) {
      setWorkingDir(workingDir);
    }
  } catch {
    // ignore
  }
  return getInitialWorkingDir();
};

export const resolveWorkingDir = (
  externalWorkingDir: string | undefined,
  requestedWorkingDir: string | undefined,
  homeDir: string
): string => externalWorkingDir?.trim() || requestedWorkingDir || homeDir;

import type { GooseServeExitSignal, GooseServeResult, Logger } from './gooseServe';

export const GOOSE_SERVE_EXITED_USER_MESSAGE =
  "This window's Goose backend stopped. Close this window and open a new chat to start a new backend. If this keeps happening, restart Goose Desktop.";

export interface GooseServeLease {
  acpUrl: string;
  secretKey: string;
  workingDir: string;
  cleanup: () => Promise<void>;
  windowIds: Set<number>;
  cleanedUp: boolean;
  exited: boolean;
  exitCode: number | null;
  exitSignal: GooseServeExitSignal;
}

export class GooseServeLeaseRegistry {
  private leasesByWindowId = new Map<number, GooseServeLease>();

  constructor(private readonly logger: Logger) {}

  create(result: GooseServeResult, secretKey: string, workingDir: string): GooseServeLease {
    const lease: GooseServeLease = {
      acpUrl: result.acpUrl,
      secretKey,
      workingDir,
      cleanup: result.cleanup,
      windowIds: new Set<number>(),
      cleanedUp: false,
      exited: false,
      exitCode: null,
      exitSignal: null,
    };

    const markExited = ({
      code,
      signal,
      logUnexpected,
    }: {
      code?: number | null;
      signal?: GooseServeExitSignal;
      logUnexpected: boolean;
    }) => {
      const firstExit = !lease.exited;
      lease.exited = true;
      if (code !== undefined) {
        lease.exitCode = code;
      }
      if (signal !== undefined) {
        lease.exitSignal = signal;
      }

      if (logUnexpected && firstExit && !lease.cleanedUp) {
        this.logger.error('Goose ACP server exited unexpectedly', {
          code: lease.exitCode,
          signal: lease.exitSignal,
          windowIds: [...lease.windowIds],
        });
      }
    };

    result.process.once('exit', (code, signal) => {
      markExited({ code, signal, logUnexpected: true });
    });

    if (result.hasExited()) {
      const exitDetails = result.getExitDetails();
      markExited({ code: exitDetails.code, signal: exitDetails.signal, logUnexpected: false });
    }

    return lease;
  }

  createExternal(
    acpUrl: string,
    secretKey: string,
    workingDir: string,
    cleanup: () => Promise<void> = async () => undefined
  ): GooseServeLease {
    return {
      acpUrl,
      secretKey,
      workingDir,
      cleanup,
      windowIds: new Set<number>(),
      cleanedUp: false,
      exited: false,
      exitCode: null,
      exitSignal: null,
    };
  }

  get(windowId: number): GooseServeLease | null {
    return this.leasesByWindowId.get(windowId) ?? null;
  }

  getAcpUrl(windowId: number): string | null {
    const lease = this.get(windowId);
    if (!lease) {
      return null;
    }
    if (lease.exited) {
      throw new Error(GOOSE_SERVE_EXITED_USER_MESSAGE);
    }
    return lease.acpUrl;
  }

  getWorkingDir(windowId: number): string | null {
    return this.get(windowId)?.workingDir ?? null;
  }

  getSecretKey(windowId: number): string | null {
    const lease = this.get(windowId);
    if (!lease) {
      return null;
    }
    if (lease.exited) {
      throw new Error(GOOSE_SERVE_EXITED_USER_MESSAGE);
    }
    return lease.secretKey;
  }

  attachWindow(windowId: number, lease: GooseServeLease) {
    lease.windowIds.add(windowId);
    this.leasesByWindowId.set(windowId, lease);
  }

  async releaseWindow(windowId: number) {
    const lease = this.leasesByWindowId.get(windowId);
    this.leasesByWindowId.delete(windowId);

    if (!lease) {
      return;
    }

    lease.windowIds.delete(windowId);
    if (lease.windowIds.size === 0) {
      await this.cleanupLease(lease);
    }
  }

  async cleanupLease(lease: GooseServeLease) {
    if (lease.cleanedUp) {
      return;
    }

    lease.cleanedUp = true;
    for (const windowId of lease.windowIds) {
      this.leasesByWindowId.delete(windowId);
    }
    lease.windowIds.clear();

    try {
      await lease.cleanup();
    } catch (error) {
      this.logger.error('Failed to cleanup goose serve backend:', error);
    }
  }

  activeLeaseCount(): number {
    return this.uniqueLeases().length;
  }

  async cleanupAll() {
    await Promise.all(this.uniqueLeases().map((lease) => this.cleanupLease(lease)));
  }

  private uniqueLeases(): GooseServeLease[] {
    return [...new Set(this.leasesByWindowId.values())];
  }
}

import type {
  CreateScheduleRequest_unstable,
  InspectRunningJobResponse_unstable,
  KillRunningJobResponse_unstable,
  RunScheduleNowResponse_unstable,
  ScheduledJobDto,
  SessionInfo,
} from '@aaif/goose-acp-client';
import { getAcpClient } from './acpConnection';

let inFlightListSchedules: Promise<ScheduledJobDto[]> | null = null;
const inFlightListScheduleSessions = new Map<string, Promise<SessionInfo[]>>();

function acpErrorMessage(error: unknown): string | null {
  if (typeof error !== 'object' || error === null) {
    return null;
  }

  const candidate = 'error' in error && isRecord(error.error) ? error.error : error;
  if (!isRecord(candidate)) {
    return null;
  }
  if (typeof candidate.data === 'string') {
    return candidate.data;
  }
  return typeof candidate.message === 'string' ? candidate.message : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function normalizeAcpError(error: unknown, fallback: string): Error {
  const message = acpErrorMessage(error);
  if (message) {
    return new Error(message);
  }
  if (error instanceof Error) {
    return error;
  }
  return new Error(fallback);
}

function clearInFlightScheduleReads(): void {
  inFlightListSchedules = null;
  inFlightListScheduleSessions.clear();
}

export async function acpListSchedules(): Promise<ScheduledJobDto[]> {
  const pending = inFlightListSchedules;
  if (pending) {
    return pending;
  }

  const listPromise = (async () => {
    const client = await getAcpClient();
    const response = await client.goose.schedulesList_unstable({});
    return response.jobs;
  })().catch((error) => {
    throw normalizeAcpError(error, 'Failed to list schedules');
  });

  inFlightListSchedules = listPromise;

  try {
    return await listPromise;
  } finally {
    if (inFlightListSchedules === listPromise) {
      inFlightListSchedules = null;
    }
  }
}

export async function acpCreateSchedule(
  request: CreateScheduleRequest_unstable
): Promise<ScheduledJobDto> {
  try {
    const client = await getAcpClient();
    const response = await client.goose.schedulesCreate_unstable(request);
    clearInFlightScheduleReads();
    return response.job;
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to create schedule');
  }
}

export async function acpDeleteSchedule(scheduleId: string): Promise<void> {
  try {
    const client = await getAcpClient();
    await client.goose.schedulesDelete_unstable({ scheduleId });
    clearInFlightScheduleReads();
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to delete schedule');
  }
}

export async function acpListScheduleSessions(
  scheduleId: string,
  limit: number
): Promise<SessionInfo[]> {
  const key = `${scheduleId}:${limit}`;
  const pending = inFlightListScheduleSessions.get(key);
  if (pending) {
    return pending;
  }

  const listPromise = (async () => {
    const client = await getAcpClient();
    const response = await client.goose.schedulesSessionsList_unstable({ scheduleId, limit });
    return response.sessions;
  })().catch((error) => {
    throw normalizeAcpError(error, 'Failed to list schedule sessions');
  });

  inFlightListScheduleSessions.set(key, listPromise);

  try {
    return await listPromise;
  } finally {
    if (inFlightListScheduleSessions.get(key) === listPromise) {
      inFlightListScheduleSessions.delete(key);
    }
  }
}

export async function acpRunScheduleNow(
  scheduleId: string
): Promise<RunScheduleNowResponse_unstable> {
  try {
    const client = await getAcpClient();
    const response = await client.goose.schedulesRunNow_unstable({ scheduleId });
    clearInFlightScheduleReads();
    return response;
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to run schedule now');
  }
}

export async function acpPauseSchedule(scheduleId: string): Promise<void> {
  try {
    const client = await getAcpClient();
    await client.goose.schedulesPause_unstable({ scheduleId });
    clearInFlightScheduleReads();
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to pause schedule');
  }
}

export async function acpUnpauseSchedule(scheduleId: string): Promise<void> {
  try {
    const client = await getAcpClient();
    await client.goose.schedulesUnpause_unstable({ scheduleId });
    clearInFlightScheduleReads();
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to unpause schedule');
  }
}

export async function acpUpdateSchedule(
  scheduleId: string,
  cron: string
): Promise<ScheduledJobDto> {
  try {
    const client = await getAcpClient();
    const response = await client.goose.schedulesUpdate_unstable({ scheduleId, cron });
    clearInFlightScheduleReads();
    return response.job;
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to update schedule');
  }
}

export async function acpKillRunningJob(jobId: string): Promise<KillRunningJobResponse_unstable> {
  try {
    const client = await getAcpClient();
    const response = await client.goose.schedulesRunningJobKill_unstable({ jobId });
    clearInFlightScheduleReads();
    return response;
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to kill running job');
  }
}

export async function acpInspectRunningJob(
  jobId: string
): Promise<InspectRunningJobResponse_unstable> {
  try {
    const client = await getAcpClient();
    return await client.goose.schedulesRunningJobInspect_unstable({ jobId });
  } catch (error) {
    throw normalizeAcpError(error, 'Failed to inspect running job');
  }
}

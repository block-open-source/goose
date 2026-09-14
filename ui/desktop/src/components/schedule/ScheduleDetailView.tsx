import React, { useState, useEffect, useCallback } from 'react';
import type { ScheduledJobDto, SessionInfo } from '@aaif/goose-acp-client';
import { Button } from '../ui/button';
import { ScrollArea } from '../ui/scroll-area';
import BackButton from '../ui/BackButton';
import { Card } from '../ui/card';
import {
  acpListScheduleSessions,
  acpRunScheduleNow,
  acpPauseSchedule,
  acpUnpauseSchedule,
  acpUpdateSchedule,
  acpListSchedules,
  acpKillRunningJob,
  acpInspectRunningJob,
} from '../../acp/schedules';
import { ScheduleModal, NewSchedulePayload } from './ScheduleModal';
import { toastError, toastSuccess } from '../../toasts';
import { Loader2, Pause, Play, Edit, Square, Eye } from 'lucide-react';
import cronstrue from 'cronstrue';
import { formatToLocalDateWithTimezone } from '../../utils/date';
import { trackScheduleRunNow, getErrorType } from '../../utils/analytics';
import { errorMessage } from '../../utils/conversionUtils';
import { defineMessages, useIntl } from '../../i18n';
import { useNavigation } from '../../hooks/useNavigation';

const i18n = defineMessages({
  scheduleNotFound: { id: 'scheduleDetailView.scheduleNotFound', defaultMessage: 'Schedule Not Found' },
  noScheduleId: { id: 'scheduleDetailView.noScheduleId', defaultMessage: 'No schedule ID provided. Return to schedules list.' },
  scheduleDetails: { id: 'scheduleDetailView.scheduleDetails', defaultMessage: 'Schedule Details' },
  viewingScheduleId: { id: 'scheduleDetailView.viewingScheduleId', defaultMessage: 'Viewing Schedule ID: {id}' },
  scheduleInformation: { id: 'scheduleDetailView.scheduleInformation', defaultMessage: 'Schedule Information' },
  loadingSchedule: { id: 'scheduleDetailView.loadingSchedule', defaultMessage: 'Loading schedule...' },
  errorPrefix: { id: 'scheduleDetailView.errorPrefix', defaultMessage: 'Error: {error}' },
  currentlyRunning: { id: 'scheduleDetailView.currentlyRunning', defaultMessage: 'Currently Running' },
  paused: { id: 'scheduleDetailView.paused', defaultMessage: 'Paused' },
  scheduleLabel: { id: 'scheduleDetailView.scheduleLabel', defaultMessage: 'Schedule:' },
  cronExpression: { id: 'scheduleDetailView.cronExpression', defaultMessage: 'Cron Expression:' },
  recipeSource: { id: 'scheduleDetailView.recipeSource', defaultMessage: 'Recipe Source:' },
  lastRun: { id: 'scheduleDetailView.lastRun', defaultMessage: 'Last Run:' },
  currentSession: { id: 'scheduleDetailView.currentSession', defaultMessage: 'Current Session:' },
  processStarted: { id: 'scheduleDetailView.processStarted', defaultMessage: 'Process Started:' },
  actions: { id: 'scheduleDetailView.actions', defaultMessage: 'Actions' },
  runScheduleNow: { id: 'scheduleDetailView.runScheduleNow', defaultMessage: 'Run Schedule Now' },
  editSchedule: { id: 'scheduleDetailView.editSchedule', defaultMessage: 'Edit Schedule' },
  unpauseSchedule: { id: 'scheduleDetailView.unpauseSchedule', defaultMessage: 'Unpause Schedule' },
  pauseSchedule: { id: 'scheduleDetailView.pauseSchedule', defaultMessage: 'Pause Schedule' },
  inspectRunningJob: { id: 'scheduleDetailView.inspectRunningJob', defaultMessage: 'Inspect Running Job' },
  killRunningJob: { id: 'scheduleDetailView.killRunningJob', defaultMessage: 'Kill Running Job' },
  cannotModifyRunning: { id: 'scheduleDetailView.cannotModifyRunning', defaultMessage: 'Cannot trigger or modify a schedule while it\'s already running.' },
  pausedWarning: { id: 'scheduleDetailView.pausedWarning', defaultMessage: 'This schedule is paused and will not run automatically. Use "Run Schedule Now" to trigger it manually or unpause to resume automatic execution.' },
  recentSessions: { id: 'scheduleDetailView.recentSessions', defaultMessage: 'Recent Sessions' },
  loadingSessions: { id: 'scheduleDetailView.loadingSessions', defaultMessage: 'Loading sessions...' },
  noSessions: { id: 'scheduleDetailView.noSessions', defaultMessage: 'No sessions found for this schedule.' },
  sessionId: { id: 'scheduleDetailView.sessionId', defaultMessage: 'Session ID: {id}' },
  created: { id: 'scheduleDetailView.created', defaultMessage: 'Created: {date}' },
  messages: { id: 'scheduleDetailView.messages', defaultMessage: 'Messages: {count}' },
  dir: { id: 'scheduleDetailView.dir', defaultMessage: 'Dir: {path}' },
  idLabel: { id: 'scheduleDetailView.idLabel', defaultMessage: 'ID:' },
  jobCancelled: { id: 'scheduleDetailView.jobCancelled', defaultMessage: 'Job Cancelled' },
  jobCancelledMsg: { id: 'scheduleDetailView.jobCancelledMsg', defaultMessage: 'The job was cancelled while starting up.' },
  scheduleCompleted: { id: 'scheduleDetailView.scheduleCompleted', defaultMessage: 'Run completed' },
  completedSession: { id: 'scheduleDetailView.completedSession', defaultMessage: 'Session: {sessionId}' },
  runScheduleError: { id: 'scheduleDetailView.runScheduleError', defaultMessage: 'Run Schedule Error' },
  scheduleUnpaused: { id: 'scheduleDetailView.scheduleUnpaused', defaultMessage: 'Schedule Unpaused' },
  unpausedMsg: { id: 'scheduleDetailView.unpausedMsg', defaultMessage: 'Unpaused "{id}"' },
  schedulePaused: { id: 'scheduleDetailView.schedulePaused', defaultMessage: 'Schedule Paused' },
  pausedMsg: { id: 'scheduleDetailView.pausedMsg', defaultMessage: 'Paused "{id}"' },
  pauseUnpauseError: { id: 'scheduleDetailView.pauseUnpauseError', defaultMessage: 'Pause/Unpause Error' },
  jobKilled: { id: 'scheduleDetailView.jobKilled', defaultMessage: 'Job Killed' },
  killJobError: { id: 'scheduleDetailView.killJobError', defaultMessage: 'Kill Job Error' },
  jobInspection: { id: 'scheduleDetailView.jobInspection', defaultMessage: 'Job Inspection' },
  inspectNoInfo: { id: 'scheduleDetailView.inspectNoInfo', defaultMessage: 'No detailed information available' },
  inspectJobError: { id: 'scheduleDetailView.inspectJobError', defaultMessage: 'Inspect Job Error' },
  scheduleUpdated: { id: 'scheduleDetailView.scheduleUpdated', defaultMessage: 'Schedule Updated' },
  updatedMsg: { id: 'scheduleDetailView.updatedMsg', defaultMessage: 'Updated "{id}"' },
  updateScheduleError: { id: 'scheduleDetailView.updateScheduleError', defaultMessage: 'Update Schedule Error' },
  scheduleNotFoundError: { id: 'scheduleDetailView.scheduleNotFoundError', defaultMessage: 'Schedule not found' },
});

interface ScheduleDetailViewProps {
  scheduleId: string | null;
  onNavigateBack: () => void;
}

function sessionMeta(session: SessionInfo): Record<string, unknown> {
  return typeof session._meta === 'object' && session._meta !== null ? session._meta : {};
}

function metaString(session: SessionInfo, key: string): string | undefined {
  const value = sessionMeta(session)[key];
  return typeof value === 'string' ? value : undefined;
}

function metaNumber(session: SessionInfo, key: string): number | undefined {
  const value = sessionMeta(session)[key];
  return typeof value === 'number' ? value : undefined;
}

const ScheduleDetailView: React.FC<ScheduleDetailViewProps> = ({ scheduleId, onNavigateBack }) => {
  const intl = useIntl();
  const setView = useNavigation();
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [isLoadingSessions, setIsLoadingSessions] = useState(false);
  const [sessionsError, setSessionsError] = useState<string | null>(null);

  const [scheduleDetails, setScheduleDetails] = useState<ScheduledJobDto | null>(null);
  const [isLoadingSchedule, setIsLoadingSchedule] = useState(false);
  const [scheduleError, setScheduleError] = useState<string | null>(null);

  const [isActionLoading, setIsActionLoading] = useState(false);

  const [isModalOpen, setIsModalOpen] = useState(false);

  const fetchSessions = async (sId: string) => {
    setIsLoadingSessions(true);
    setSessionsError(null);
    try {
      const data = await acpListScheduleSessions(sId, 20);
      setSessions(data);
    } catch (err) {
      setSessionsError(errorMessage(err, 'Failed to fetch sessions'));
    } finally {
      setIsLoadingSessions(false);
    }
  };

  const fetchSchedule = useCallback(async (sId: string) => {
    setIsLoadingSchedule(true);
    setScheduleError(null);
    try {
      const allSchedules = await acpListSchedules();
      const schedule = allSchedules.find((s) => s.id === sId);
      if (schedule) {
        setScheduleDetails(schedule);
      } else {
        setScheduleError(intl.formatMessage(i18n.scheduleNotFoundError));
      }
    } catch (err) {
      setScheduleError(errorMessage(err, 'Failed to fetch schedule'));
    } finally {
      setIsLoadingSchedule(false);
    }
  }, [intl]);

  useEffect(() => {
    if (scheduleId) {
      fetchSessions(scheduleId);
      fetchSchedule(scheduleId);
    }
  }, [scheduleId, fetchSchedule]);

  const openSession = useCallback((sessionId: string) => {
    setView('pair', {
      disableAnimation: true,
      resumeSessionId: sessionId,
    });
  }, [setView]);

  const handleRunNow = async () => {
    if (!scheduleId) return;
    setIsActionLoading(true);
    try {
      const result = await acpRunScheduleNow(scheduleId);
      trackScheduleRunNow(true);
      if (result.status === 'completed' && result.sessionId) {
        toastSuccess({ title: intl.formatMessage(i18n.scheduleCompleted), msg: intl.formatMessage(i18n.completedSession, { sessionId: result.sessionId }) });
      }
      await fetchSessions(scheduleId);
      await fetchSchedule(scheduleId);
    } catch (err) {
      const errorMsg = errorMessage(err, 'Failed to trigger schedule');
      trackScheduleRunNow(false, getErrorType(err));
      toastError({
        title: intl.formatMessage(i18n.runScheduleError),
        msg: errorMsg,
      });
    } finally {
      setIsActionLoading(false);
    }
  };

  const handlePauseToggle = async () => {
    if (!scheduleId || !scheduleDetails) return;
    setIsActionLoading(true);
    try {
      if (scheduleDetails.paused) {
        await acpUnpauseSchedule(scheduleId);
        toastSuccess({ title: intl.formatMessage(i18n.scheduleUnpaused), msg: intl.formatMessage(i18n.unpausedMsg, { id: scheduleId }) });
      } else {
        await acpPauseSchedule(scheduleId);
        toastSuccess({ title: intl.formatMessage(i18n.schedulePaused), msg: intl.formatMessage(i18n.pausedMsg, { id: scheduleId }) });
      }
      await fetchSchedule(scheduleId);
    } catch (err) {
      const errorMsg = errorMessage(err, 'Operation failed');
      toastError({
        title: intl.formatMessage(i18n.pauseUnpauseError),
        msg: errorMsg,
      });
    } finally {
      setIsActionLoading(false);
    }
  };

  const handleKill = async () => {
    if (!scheduleId) return;
    setIsActionLoading(true);
    try {
      const result = await acpKillRunningJob(scheduleId);
      toastSuccess({ title: intl.formatMessage(i18n.jobKilled), msg: result.message });
      await fetchSchedule(scheduleId);
    } catch (err) {
      const errorMsg = errorMessage(err, 'Failed to kill job');
      toastError({
        title: intl.formatMessage(i18n.killJobError),
        msg: errorMsg,
      });
    } finally {
      setIsActionLoading(false);
    }
  };

  const handleInspect = async () => {
    if (!scheduleId) return;
    setIsActionLoading(true);
    try {
      const result = await acpInspectRunningJob(scheduleId);
      if (result.sessionId) {
        const duration = result.runningDurationSeconds
          ? `${Math.floor(result.runningDurationSeconds / 60)}m ${result.runningDurationSeconds % 60}s`
          : 'Unknown';
        toastSuccess({
          title: intl.formatMessage(i18n.jobInspection),
          msg: `Session: ${result.sessionId}\nRunning for: ${duration}`,
        });
      } else {
        toastSuccess({ title: intl.formatMessage(i18n.jobInspection), msg: intl.formatMessage(i18n.inspectNoInfo) });
      }
    } catch (err) {
      const errorMsg = errorMessage(err, 'Failed to inspect job');
      toastError({
        title: intl.formatMessage(i18n.inspectJobError),
        msg: errorMsg,
      });
    } finally {
      setIsActionLoading(false);
    }
  };

  const handleModalSubmit = async (payload: NewSchedulePayload | string) => {
    if (!scheduleId) return;
    setIsActionLoading(true);
    try {
      await acpUpdateSchedule(scheduleId, payload as string);
      toastSuccess({ title: intl.formatMessage(i18n.scheduleUpdated), msg: intl.formatMessage(i18n.updatedMsg, { id: scheduleId }) });
      await fetchSchedule(scheduleId);
      setIsModalOpen(false);
    } catch (err) {
      const errorMsg = errorMessage(err, 'Failed to update schedule');
      toastError({
        title: intl.formatMessage(i18n.updateScheduleError),
        msg: errorMsg,
      });
    } finally {
      setIsActionLoading(false);
    }
  };

  if (!scheduleId) {
    return (
      <div className="h-screen w-full flex flex-col items-center justify-center bg-white dark:bg-gray-900 text-text-primary p-8">
        <BackButton onClick={onNavigateBack} />
        <h1 className="text-2xl font-medium text-text-primary mt-4">{intl.formatMessage(i18n.scheduleNotFound)}</h1>
        <p className="text-text-secondary mt-2">
          {intl.formatMessage(i18n.noScheduleId)}
        </p>
      </div>
    );
  }

  const readableCron = scheduleDetails
    ? (() => {
        try {
          return cronstrue.toString(scheduleDetails.cron);
        } catch {
          return scheduleDetails.cron;
        }
      })()
    : '';

  return (
    <div className="h-screen w-full flex flex-col bg-background-primary text-text-primary">
      <div className="px-8 pt-6 pb-4 border-b border-border-primary flex-shrink-0">
        <BackButton onClick={onNavigateBack} />
        <h1 className="text-4xl font-light mt-1 mb-1 pt-8">{intl.formatMessage(i18n.scheduleDetails)}</h1>
        <p className="text-sm text-text-secondary mb-1">{intl.formatMessage(i18n.viewingScheduleId, { id: scheduleId })}</p>
      </div>

      <ScrollArea className="flex-grow">
        <div className="p-8 space-y-6">
          <section>
            <h2 className="text-xl font-semibold text-text-primary mb-3">{intl.formatMessage(i18n.scheduleInformation)}</h2>
            {isLoadingSchedule && (
              <div className="flex items-center text-text-secondary">
                <Loader2 className="mr-2 h-4 w-4 animate-spin" /> {intl.formatMessage(i18n.loadingSchedule)}
              </div>
            )}
            {scheduleError && (
              <p className="text-text-danger text-sm p-3 bg-background-danger border border-border-danger rounded-md">
                {intl.formatMessage(i18n.errorPrefix, { error: scheduleError })}
              </p>
            )}
            {scheduleDetails && (
              <Card className="p-4 bg-background-primary shadow mb-6">
                <div className="space-y-2">
                  <div className="flex flex-col md:flex-row md:items-center justify-between">
                    <h3 className="text-base font-semibold text-text-primary">
                      {scheduleDetails.id}
                    </h3>
                    <div className="mt-2 md:mt-0 flex items-center gap-2">
                      {scheduleDetails.currentlyRunning && (
                        <div className="text-sm text-green-500 dark:text-green-400 font-semibold flex items-center">
                          <span className="inline-block w-2 h-2 bg-green-500 dark:bg-green-400 rounded-full mr-1 animate-pulse"></span>
                          {intl.formatMessage(i18n.currentlyRunning)}
                        </div>
                      )}
                      {scheduleDetails.paused && (
                        <div className="text-sm text-orange-500 dark:text-orange-400 font-semibold flex items-center">
                          <Pause className="w-3 h-3 mr-1" />
                          {intl.formatMessage(i18n.paused)}
                        </div>
                      )}
                    </div>
                  </div>
                  <p className="text-sm text-text-primary">
                    <span className="font-semibold">{intl.formatMessage(i18n.scheduleLabel)}</span> {readableCron}
                  </p>
                  <p className="text-sm text-text-primary">
                    <span className="font-semibold">{intl.formatMessage(i18n.cronExpression)}</span> {scheduleDetails.cron}
                  </p>
                  <p className="text-sm text-text-primary">
                    <span className="font-semibold">{intl.formatMessage(i18n.recipeSource)}</span> {scheduleDetails.source}
                  </p>
                  <p className="text-sm text-text-primary">
                    <span className="font-semibold">{intl.formatMessage(i18n.lastRun)}</span>{' '}
                    {formatToLocalDateWithTimezone(scheduleDetails.lastRun)}
                  </p>
                  {scheduleDetails.currentlyRunning && scheduleDetails.currentSessionId && (
                    <p className="text-sm text-text-primary">
                      <span className="font-semibold">{intl.formatMessage(i18n.currentSession)}</span>{' '}
                      {scheduleDetails.currentSessionId}
                    </p>
                  )}
                  {scheduleDetails.currentlyRunning && scheduleDetails.jobStartTime && (
                    <p className="text-sm text-text-primary">
                      <span className="font-semibold">{intl.formatMessage(i18n.processStarted)}</span>{' '}
                      {formatToLocalDateWithTimezone(scheduleDetails.jobStartTime)}
                    </p>
                  )}
                </div>
              </Card>
            )}
          </section>

          <section>
            <h2 className="text-xl font-semibold text-text-primary mb-3">{intl.formatMessage(i18n.actions)}</h2>
            <div className="flex flex-col md:flex-row gap-2">
              <Button
                onClick={handleRunNow}
                disabled={isActionLoading || scheduleDetails?.currentlyRunning}
                className="w-full md:w-auto"
              >
                {intl.formatMessage(i18n.runScheduleNow)}
              </Button>

              {scheduleDetails && !scheduleDetails.currentlyRunning && (
                <>
                  <Button
                    onClick={() => setIsModalOpen(true)}
                    variant="outline"
                    className="w-full md:w-auto flex items-center gap-2 text-blue-600 dark:text-blue-400 border-blue-300 dark:border-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/20"
                    disabled={isActionLoading}
                  >
                    <Edit className="w-4 h-4" />
                    {intl.formatMessage(i18n.editSchedule)}
                  </Button>
                  <Button
                    onClick={handlePauseToggle}
                    variant="outline"
                    className={`w-full md:w-auto flex items-center gap-2 ${
                      scheduleDetails.paused
                        ? 'text-green-600 dark:text-green-400 border-green-300 dark:border-green-600 hover:bg-green-50 dark:hover:bg-green-900/20'
                        : 'text-orange-600 dark:text-orange-400 border-orange-300 dark:border-orange-600 hover:bg-orange-50 dark:hover:bg-orange-900/20'
                    }`}
                    disabled={isActionLoading}
                  >
                    {scheduleDetails.paused ? (
                      <>
                        <Play className="w-4 h-4" />
                        {intl.formatMessage(i18n.unpauseSchedule)}
                      </>
                    ) : (
                      <>
                        <Pause className="w-4 h-4" />
                        {intl.formatMessage(i18n.pauseSchedule)}
                      </>
                    )}
                  </Button>
                </>
              )}

              {scheduleDetails?.currentlyRunning && (
                <>
                  <Button
                    onClick={handleInspect}
                    variant="outline"
                    className="w-full md:w-auto flex items-center gap-2 text-blue-600 dark:text-blue-400 border-blue-300 dark:border-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/20"
                    disabled={isActionLoading}
                  >
                    <Eye className="w-4 h-4" />
                    {intl.formatMessage(i18n.inspectRunningJob)}
                  </Button>
                  <Button
                    onClick={handleKill}
                    variant="outline"
                    className="w-full md:w-auto flex items-center gap-2 text-red-600 dark:text-red-400 border-red-300 dark:border-red-600 hover:bg-red-50 dark:hover:bg-red-900/20"
                    disabled={isActionLoading}
                  >
                    <Square className="w-4 h-4" />
                    {intl.formatMessage(i18n.killRunningJob)}
                  </Button>
                </>
              )}
            </div>

            {scheduleDetails?.currentlyRunning && (
              <p className="text-sm text-amber-600 dark:text-amber-400 mt-2">
                {intl.formatMessage(i18n.cannotModifyRunning)}
              </p>
            )}

            {scheduleDetails?.paused && (
              <p className="text-sm text-orange-600 dark:text-orange-400 mt-2">
                {intl.formatMessage(i18n.pausedWarning)}
              </p>
            )}
          </section>

          <section>
            <h2 className="text-xl font-semibold text-text-primary mb-4">{intl.formatMessage(i18n.recentSessions)}</h2>
            {isLoadingSessions && <p className="text-text-secondary">{intl.formatMessage(i18n.loadingSessions)}</p>}
            {sessionsError && (
              <p className="text-text-danger text-sm p-3 bg-background-danger border border-border-danger rounded-md">
                {intl.formatMessage(i18n.errorPrefix, { error: sessionsError })}
              </p>
            )}
            {!isLoadingSessions && sessions.length === 0 && (
              <p className="text-text-secondary text-center py-4">
                {intl.formatMessage(i18n.noSessions)}
              </p>
            )}

            {sessions.length > 0 && (
              <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                {sessions.map((session) => {
                  const sessionId = String(session.sessionId);
                  const sessionName = session.title ?? sessionId;
                  const createdAt = metaString(session, 'createdAt') ?? session.updatedAt;
                  const messageCount = metaNumber(session, 'messageCount');

                  return (
                    <Card
                      key={sessionId}
                      className="p-4 bg-background-primary shadow cursor-pointer hover:shadow-lg transition-shadow duration-200"
                      onClick={() => openSession(sessionId)}
                    >
                      <h3
                        className="text-sm font-semibold text-text-primary truncate"
                        title={sessionName}
                      >
                        {sessionName || intl.formatMessage(i18n.sessionId, { id: sessionId })}
                      </h3>
                      <p className="text-xs text-text-secondary mt-1">
                        {intl.formatMessage(i18n.created, { date: createdAt ? formatToLocalDateWithTimezone(createdAt) : 'N/A' })}
                      </p>
                      {messageCount !== undefined && (
                        <p className="text-xs text-text-secondary mt-1">
                          {intl.formatMessage(i18n.messages, { count: messageCount })}
                        </p>
                      )}
                      {session.cwd && (
                        <p
                          className="text-xs text-text-secondary mt-1 truncate"
                          title={session.cwd}
                        >
                          {intl.formatMessage(i18n.dir, { path: session.cwd })}
                        </p>
                      )}
                      <p className="text-xs text-text-secondary mt-1">
                        {intl.formatMessage(i18n.idLabel)} <span className="font-mono">{sessionId}</span>
                      </p>
                    </Card>
                  );
                })}
              </div>
            )}
          </section>
        </div>
      </ScrollArea>

      <ScheduleModal
        isOpen={isModalOpen}
        onClose={() => setIsModalOpen(false)}
        onSubmit={handleModalSubmit}
        schedule={scheduleDetails}
        isLoadingExternally={isActionLoading}
        apiErrorExternally={null}
        initialDeepLink={null}
      />
    </div>
  );
};

export default ScheduleDetailView;

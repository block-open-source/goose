import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router';
import McpAppRenderer from '../McpApps/McpAppRenderer';
import { listMcpApps } from '../../acp/mcp-apps';
import { acpCloseSession, acpNewSession } from '../../acp/sessions';
import { formatAppName } from '../../utils/conversionUtils';
import { errorMessage } from '../../utils/conversionUtils';
import { defineMessages, useIntl } from '../../i18n';

const i18n = defineMessages({
  failedToLoad: {
    id: 'standaloneAppView.failedToLoad',
    defaultMessage: 'Failed to Load App',
  },
  initializing: {
    id: 'standaloneAppView.initializing',
    defaultMessage: 'Initializing app...',
  },
  missingParams: {
    id: 'standaloneAppView.missingParams',
    defaultMessage: 'Missing required parameters',
  },
});

export default function StandaloneAppView() {
  const intl = useIntl();
  const [searchParams] = useSearchParams();
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [cachedHtml, setCachedHtml] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const resourceUri = searchParams.get('resourceUri');
  const extensionName = searchParams.get('extensionName');
  const appName = searchParams.get('appName');
  const workingDir = searchParams.get('workingDir');

  useEffect(() => {
    async function loadCachedHtml() {
      if (
        !resourceUri ||
        !extensionName ||
        resourceUri === 'undefined' ||
        extensionName === 'undefined'
      ) {
        setError(intl.formatMessage(i18n.missingParams));
        setLoading(false);
        return;
      }

      try {
        const apps = await listMcpApps();
        const cachedApp = apps.find(
          (app) => app.uri === resourceUri && app.mcpServers?.includes(extensionName)
        );

        if (cachedApp?.text) {
          setCachedHtml(cachedApp.text);
          setLoading(false);
        }
      } catch (err) {
        console.warn('Failed to load cached HTML:', err);
      }
    }

    loadCachedHtml();
  }, [resourceUri, extensionName, intl]);

  useEffect(() => {
    async function initSession() {
      if (!resourceUri || !extensionName || !workingDir) {
        return;
      }

      try {
        // This view names no extension set, so the backend uses the configured one.
        const { sessionId: sid } = await acpNewSession(workingDir, undefined);
        setSessionId(sid);
        setLoading(false);
      } catch (err) {
        console.error('Failed to initialize session:', err);
        if (!cachedHtml) {
          setError(errorMessage(err, 'Failed to initialize session'));
          setLoading(false);
        }
      }
    }

    initSession();
  }, [resourceUri, extensionName, workingDir, cachedHtml]);

  useEffect(() => {
    if (appName) {
      document.title = formatAppName(appName);
    }
  }, [appName]);

  // Cleanup session when component unmounts
  useEffect(() => {
    return () => {
      if (sessionId) {
        acpCloseSession(sessionId).catch((err: unknown) => {
          console.warn('Failed to stop agent on unmount:', err);
        });
      }
    };
  }, [sessionId]);

  if (error && !cachedHtml) {
    return (
      <div
        style={{
          width: '100vw',
          height: '100vh',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          flexDirection: 'column',
          gap: '16px',
          padding: '24px',
        }}
      >
        <h2 style={{ color: 'var(--text-error, #ef4444)' }}>
          {intl.formatMessage(i18n.failedToLoad)}
        </h2>
        <p style={{ color: 'var(--color-text-secondary, #6b7280)' }}>{error}</p>
      </div>
    );
  }

  if (loading && !cachedHtml) {
    return (
      <div
        style={{
          width: '100vw',
          height: '100vh',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <p style={{ color: 'var(--color-text-secondary, #6b7280)' }}>
          {intl.formatMessage(i18n.initializing)}
        </p>
      </div>
    );
  }

  if (cachedHtml || sessionId) {
    return (
      <div style={{ width: '100vw', height: '100vh', overflow: 'hidden' }}>
        <McpAppRenderer
          resourceUri={resourceUri!}
          extensionName={extensionName!}
          sessionId={sessionId || null}
          displayMode="standalone"
          cachedHtml={cachedHtml || undefined}
        />
      </div>
    );
  }

  return (
    <div
      style={{
        width: '100vw',
        height: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <p style={{ color: 'var(--color-text-secondary, #6b7280)' }}>Initializing app...</p>
    </div>
  );
}

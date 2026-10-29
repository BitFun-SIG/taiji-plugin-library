import React, { useCallback, useEffect, useState } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import { MarkdownRenderer } from '@/infrastructure/markdown';
import { Empty, Icon, IconButton, LoadingState, PageHeader, ScrollArea, Tooltip } from '@openbitfun/ui';
import type { SessionUsageReport, UsageContextCategory } from '@/infrastructure/api/service-api/SessionAPI';
import {
  buildSessionUsageExportMarkdown,
  isUnusedSession,
  getUsageFileNameFromPath,
  getUsageExportRedactPathsPreference,
  subscribeUsageExportRedactPathsPreference,
} from './usageReportUtils';
import { SessionUsageSummary } from './SessionUsageSummary';
import { SessionUsageEmptyState } from './SessionUsageEmptyState';
import { openSessionContextPreview } from '../../services/openSessionContextPreview';
import { notificationService } from '@/shared/notification-system';
import { getActiveSurfaceScope, isSurfaceChangedError } from '@/infrastructure/peer-device/deviceSurface';
import type { SessionUsagePanelTab } from './sessionUsagePanelTypes';
import './SessionUsagePanel.scss';

interface SessionUsagePanelProps {
  report?: SessionUsageReport;
  markdown?: string;
  sessionId?: string;
  sessionTitle?: string;
  isLoading?: boolean;
  error?: string;
  workspacePath?: string;
  workspaceId?: string;
  /** Accepted for saved panels from versions with detail tabs. */
  initialTab?: SessionUsagePanelTab;
}

export const SessionUsagePanel: React.FC<SessionUsagePanelProps> = ({
  report,
  markdown = '',
  sessionId,
  sessionTitle,
  isLoading = false,
  error,
  workspacePath,
  workspaceId,
}) => {
  const { t } = useI18n('flow-chat');
  const [copied, setCopied] = useState(false);
  const [previewLoading, setPreviewLoading] = useState<UsageContextCategory | null>(null);
  const [redactExportPaths, setRedactExportPaths] = useState(getUsageExportRedactPathsPreference);

  useEffect(() => (
    subscribeUsageExportRedactPathsPreference(setRedactExportPaths)
  ), []);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(buildSessionUsageExportMarkdown(markdown, report, {
        redactPaths: redactExportPaths,
        t,
      }));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      setCopied(false);
    }
  }, [markdown, redactExportPaths, report, t]);

  const handlePreviewContext = async (category: UsageContextCategory) => {
    if (!report || previewLoading) return;
    const surface = getActiveSurfaceScope();
    setPreviewLoading(category);
    try {
      const result = await openSessionContextPreview({
        sessionId: sessionId ?? report.sessionId,
        reportSessionId: report.sessionId,
        workspaceId: workspaceId ?? report.workspace.workspaceId,
        title: `${t(`usage.context.categories.${category}`)}${category === 'toolDefinitions' ? '' : '.md'}`,
        category,
        emptyContent: t('usage.context.previewEmpty'),
      });
      if (surface.isCurrent() && result === 'unavailable') {
        notificationService.warning(t('usage.context.previewUnavailable'));
      }
    } catch (error) {
      if (surface.isCurrent() && !isSurfaceChangedError(error)) {
        notificationService.error(t('usage.context.previewFailed'));
      }
    } finally {
      setPreviewLoading(null);
    }
  };

  if (isLoading || error) {
    return (
      <div className="session-usage-panel session-usage-panel--status" data-openbitfun-product-component="session-usage-panel" data-openbitfun-product-part="root">
        {isLoading ? (
          <LoadingState role="status" aria-live="polite">{t('usage.loading.title')}</LoadingState>
        ) : (
          <Empty title={t('usage.panel.failed')} description={error} />
        )}
      </div>
    );
  }

  if (!report) {
    return (
      <ScrollArea data-openbitfun-product-component="session-usage-panel" data-openbitfun-product-part="root" data-openbitfun-state="fallback" className="session-usage-panel session-usage-panel--fallback">
        <div className="session-usage-panel__fallback-toolbar" data-openbitfun-product-component="session-usage-panel" data-openbitfun-product-part="header">
          <Tooltip content={copied ? t('usage.actions.copied') : t('usage.actions.copyMarkdown')}>
            <IconButton
              size="sm"
              onClick={handleCopy}
              aria-label={copied ? t('usage.actions.copied') : t('usage.actions.copyMarkdown')}
              icon={copied ? <Icon name="check-line" size="sm" /> : <Icon name="duplicate" size="sm" />}
            />
          </Tooltip>
        </div>
        <MarkdownRenderer content={markdown} />
      </ScrollArea>
    );
  }

  if (isUnusedSession(report)) {
    return (
      <div className="session-usage-panel session-usage-panel--status" data-openbitfun-product-component="session-usage-panel" data-openbitfun-product-part="root">
        <SessionUsageEmptyState />
      </div>
    );
  }

  const effectiveWorkspacePath = workspacePath ?? report.workspace.pathLabel;
  const workspaceName = effectiveWorkspacePath?.trim()
    ? getUsageFileNameFromPath(effectiveWorkspacePath.trim())
    : t('usage.unavailable');
  return (
    <ScrollArea data-openbitfun-product-component="session-usage-panel" data-openbitfun-product-part="root" className="session-usage-panel">
      <PageHeader
        className="session-usage-panel__header"
        data-openbitfun-product-component="session-usage-panel"
        data-openbitfun-product-part="header"
        level={2}
        align="center"
        title={<span className="session-usage-panel__title">{sessionTitle || t('usage.title')}</span>}
        action={
          <Tooltip content={copied ? t('usage.actions.copied') : t('usage.actions.copyMarkdown')}>
            <IconButton
              size="sm"
              onClick={handleCopy}
              aria-label={copied ? t('usage.actions.copied') : t('usage.actions.copyMarkdown')}
              icon={copied ? <Icon name="check-line" size="sm" /> : <Icon name="duplicate" size="sm" />}
            />
          </Tooltip>
        }
      />

      <SessionUsageSummary report={report} workspaceName={workspaceName}
        onPreviewContext={handlePreviewContext} previewLoading={previewLoading} />
    </ScrollArea>
  );
};

SessionUsagePanel.displayName = 'SessionUsagePanel';

import { Button, Checkbox, StatusPill, type StatusPillTone } from '@openbitfun/ui';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { confirmDialog } from '@/infrastructure/confirm-dialog';
import { useI18n } from '@/infrastructure/i18n';
import {
  legacyMigrationAPI,
  type LegacyMigrationScanView,
  type LegacyMigrationStatusView,
  type MigrationDomainId,
  type MigrationDomainState,
  type MigrationGroupId,
  type MigrationRunReport,
  type MigrationRunStatus,
  type MigrationSelection,
} from '@/infrastructure/api/service-api/LegacyMigrationAPI';
import { useNotification } from '@/shared/notification-system';
import { createLogger } from '@/shared/utils/logger';
import {
  ConfigLoadingState,
  ConfigMessage,
  ConfigPageContent,
  ConfigPageHeader,
  ConfigPageLayout,
  ConfigPageRow,
  ConfigPageSection,
  ConfigRetryState,
} from './common';
import './LegacyMigrationSettingsPage.scss';

const log = createLogger('LegacyMigrationSettings');

const MIGRATION_GROUPS: readonly MigrationGroupId[] = [
  'settings_and_credentials',
  'agents_skills_and_miniapps',
  'workspaces_sessions_and_tasks',
  'memory',
  'remote_connections_and_devices',
];

const DOMAIN_GROUPS: Partial<Record<MigrationDomainId, MigrationGroupId>> = {
  settings: 'settings_and_credentials',
  credentials: 'settings_and_credentials',
  skills: 'agents_skills_and_miniapps',
  miniapps: 'agents_skills_and_miniapps',
  agents: 'agents_skills_and_miniapps',
  workspace_sessions: 'workspaces_sessions_and_tasks',
  agent_coordination: 'workspaces_sessions_and_tasks',
  structured_memory: 'memory',
  file_memory: 'memory',
  remote_connect_devices: 'remote_connections_and_devices',
  remote_ssh: 'remote_connections_and_devices',
};

function reportTone(status: MigrationRunStatus): StatusPillTone {
  if (status === 'completed') return 'success';
  if (status === 'completed_with_warnings' || status === 'cancelled') return 'warning';
  if (status.startsWith('failed_')) return 'danger';
  return 'info';
}

function domainTone(state: MigrationDomainState): StatusPillTone {
  if (state === 'verified') return 'success';
  if (state === 'failed') return 'danger';
  if (state === 'skipped') return 'warning';
  return 'neutral';
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

export default function LegacyMigrationSettingsPage() {
  const { t, formatDate, formatNumber } = useI18n('settings/legacy-migration');
  const notification = useNotification();
  const [status, setStatus] = useState<LegacyMigrationStatusView | null>(null);
  const [scan, setScan] = useState<LegacyMigrationScanView | null>(null);
  const [report, setReport] = useState<MigrationRunReport | null>(null);
  const [selectedGroups, setSelectedGroups] = useState<Set<MigrationGroupId>>(
    () => new Set(MIGRATION_GROUPS),
  );
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [busy, setBusy] = useState<'scan' | 'prepare' | 'report' | 'preference' | null>(null);

  const loadStatus = useCallback(async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const next = await legacyMigrationAPI.getStatus();
      setStatus(next);
      setReport(next.latestReport);
    } catch (error) {
      log.error('Failed to load legacy migration status', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  const selection = useMemo<MigrationSelection>(() => ({
    groups: MIGRATION_GROUPS.filter((group) => selectedGroups.has(group)),
  }), [selectedGroups]);

  const toggleGroup = useCallback((group: MigrationGroupId) => {
    setSelectedGroups((current) => {
      const next = new Set(current);
      if (next.has(group)) next.delete(group);
      else next.add(group);
      return next;
    });
  }, []);

  const handleScan = useCallback(async () => {
    if (selection.groups.length === 0) {
      notification.warning(t('messages.emptySelection'));
      return;
    }
    setBusy('scan');
    try {
      const next = await legacyMigrationAPI.scan(selection);
      setScan(next);
      setStatus((current) => current ? {
        ...current,
        source: next.source,
        onboarding: { ...current.onboarding, lastScannedAtMs: next.scannedAtMs },
      } : current);
      notification.success(t('messages.scanComplete'));
    } catch (error) {
      log.error('Failed to scan legacy migration source', error);
      notification.error(errorMessage(error, t('messages.scanFailed')));
    } finally {
      setBusy(null);
    }
  }, [notification, selection, t]);

  const prepareMigration = useCallback(async (nextSelection: MigrationSelection) => {
    if (nextSelection.groups.length === 0) {
      notification.warning(t('messages.emptySelection'));
      return;
    }
    const confirmed = await confirmDialog({
      title: t('confirm.title'),
      message: t('confirm.message'),
      preview: (
        <ul className="openbitfun-legacy-migration__impact-list">
          <li>{t('confirm.agentImpact')}</li>
          <li>{t('confirm.terminalImpact')}</li>
          <li>{t('confirm.fileImpact')}</li>
          <li>{t('confirm.unsavedImpact')}</li>
          <li>{t('confirm.closeImpact')}</li>
        </ul>
      ),
      confirmText: t('actions.startMigration'),
      cancelText: t('actions.cancel'),
      type: 'warning',
    });
    if (!confirmed) return;

    setBusy('prepare');
    try {
      await legacyMigrationAPI.prepare(nextSelection);
      notification.info(t('messages.handoffStarted'));
    } catch (error) {
      log.error('Failed to prepare legacy migration handoff', error);
      notification.error(errorMessage(error, t('messages.prepareFailed')));
      setBusy(null);
    }
  }, [notification, t]);

  const handleLoadReport = useCallback(async () => {
    setBusy('report');
    try {
      const next = await legacyMigrationAPI.getReport();
      setReport(next);
      if (!next) notification.info(t('messages.noReport'));
    } catch (error) {
      log.error('Failed to load legacy migration report', error);
      notification.error(errorMessage(error, t('messages.reportFailed')));
    } finally {
      setBusy(null);
    }
  }, [notification, t]);

  const retrySelection = useMemo<MigrationSelection>(() => {
    if (!report) return { groups: [] };
    const failed = new Set<MigrationGroupId>();
    let crossReferenceFailed = false;
    for (const result of report.domainResults) {
      if (result.state !== 'failed') continue;
      const group = DOMAIN_GROUPS[result.domain];
      if (group) failed.add(group);
      else crossReferenceFailed = true;
    }
    return { groups: crossReferenceFailed && failed.size === 0 ? [...MIGRATION_GROUPS] : [...failed] };
  }, [report]);

  const restoreReminder = useCallback(async () => {
    setBusy('preference');
    try {
      const onboarding = await legacyMigrationAPI.setPromptPreference('remind_later');
      setStatus((current) => current ? { ...current, onboarding } : current);
      notification.success(t('messages.reminderRestored'));
    } catch (error) {
      log.error('Failed to restore legacy migration reminder', error);
      notification.error(errorMessage(error, t('messages.preferenceFailed')));
    } finally {
      setBusy(null);
    }
  }, [notification, t]);

  if (loading || loadFailed) {
    return (
      <ConfigPageLayout
        className="openbitfun-legacy-migration"
        data-openbitfun-component="config"
        data-openbitfun-part="root"
      >
        <ConfigPageHeader title={t('title')} subtitle={t('subtitle')} />
        <ConfigPageContent>
          {loading ? (
            <ConfigLoadingState label={t('messages.loading')} />
          ) : (
            <ConfigRetryState
              message={t('messages.loadFailed')}
              retryLabel={t('actions.retry')}
              onRetry={() => void loadStatus()}
            />
          )}
        </ConfigPageContent>
      </ConfigPageLayout>
    );
  }

  const source = status?.source ?? null;
  const lastScannedAtMs = scan?.scannedAtMs ?? status?.onboarding.lastScannedAtMs ?? null;

  return (
    <ConfigPageLayout
      className="openbitfun-legacy-migration"
      data-openbitfun-component="config"
      data-openbitfun-part="root"
    >
      <ConfigPageHeader title={t('title')} subtitle={t('subtitle')} />
      <ConfigPageContent>
        <ConfigMessage message={{ type: 'info', text: t('localOnlyNotice') }} />

        <ConfigPageSection title={t('sections.source.title')} description={t('sections.source.description')}>
          <ConfigPageRow label={t('fields.detected.label')} description={t('fields.detected.description')} align="center">
            <StatusPill tone={source ? (source.supported ? 'success' : 'warning') : 'neutral'}>
              {source
                ? t(source.supported ? 'source.supported' : 'source.unsupported')
                : t('source.notDetected')}
            </StatusPill>
          </ConfigPageRow>
          <ConfigPageRow label={t('fields.source.label')} description={t('fields.source.description')} align="center">
            <span className="openbitfun-legacy-migration__value">
              {source
                ? t('source.summary', {
                  product: source.productId,
                  version: source.productVersion || t('source.unknownVersion'),
                  platform: source.platform,
                })
                : t('source.none')}
            </span>
          </ConfigPageRow>
          <ConfigPageRow label={t('fields.lastScan.label')} description={t('fields.lastScan.description')} align="center">
            <span className="openbitfun-legacy-migration__value">
              {lastScannedAtMs
                ? formatDate(lastScannedAtMs, { dateStyle: 'medium', timeStyle: 'short' })
                : t('source.neverScanned')}
            </span>
          </ConfigPageRow>
          <ConfigPageRow label={t('fields.actions.label')} description={t('fields.actions.description')} align="center" wide>
            <div className="openbitfun-legacy-migration__actions">
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={busy === 'scan'}
                disabled={busy !== null || !source}
                onClick={() => void handleScan()}
              >
                {t('actions.scan')}
              </Button>
              <Button
                type="button"
                variant="primary"
                size="sm"
                loading={busy === 'prepare'}
                disabled={busy !== null || !source?.supported || source.alreadyMigrated}
                onClick={() => void prepareMigration(selection)}
              >
                {t('actions.startMigration')}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={busy === 'report'}
                disabled={busy !== null}
                onClick={() => void handleLoadReport()}
              >
                {t('actions.viewReport')}
              </Button>
            </div>
          </ConfigPageRow>
          {status?.onboarding.choice === 'do_not_remind' ? (
            <ConfigPageRow label={t('fields.reminder.label')} description={t('fields.reminder.description')} align="center">
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={busy === 'preference'}
                disabled={busy !== null}
                onClick={() => void restoreReminder()}
              >
                {t('actions.restoreReminder')}
              </Button>
            </ConfigPageRow>
          ) : null}
        </ConfigPageSection>

        <ConfigPageSection title={t('sections.scope.title')} description={t('sections.scope.description')}>
          {MIGRATION_GROUPS.map((group) => (
            <ConfigPageRow
              key={group}
              label={t(`groups.${group}.label`)}
              description={t(`groups.${group}.description`)}
              align="center"
            >
              <Checkbox
                checked={selectedGroups.has(group)}
                disabled={busy !== null}
                onCheckedChange={() => toggleGroup(group)}
                aria-label={t(`groups.${group}.label`)}
              />
            </ConfigPageRow>
          ))}
        </ConfigPageSection>

        {scan ? (
          <ConfigPageSection title={t('sections.scan.title')} description={t('sections.scan.description')}>
            {scan.findings.length === 0 ? (
              <ConfigPageRow label={t('scan.empty')}><span /></ConfigPageRow>
            ) : scan.findings.map((finding) => (
              <ConfigPageRow
                key={`${finding.domain}:${finding.code}`}
                label={t(`domains.${finding.domain}`)}
                description={t('scan.summary', {
                  count: formatNumber(finding.entityCount),
                  bytes: formatNumber(finding.logicalBytes),
                })}
                align="center"
              >
                <StatusPill tone={finding.migratable ? 'info' : 'warning'}>
                  {t(finding.migratable ? 'scan.migratable' : 'scan.blocked')}
                </StatusPill>
              </ConfigPageRow>
            ))}
          </ConfigPageSection>
        ) : null}

        <ConfigPageSection title={t('sections.report.title')} description={t('sections.report.description')}>
          {report ? (
            <>
              <ConfigPageRow label={t('report.result')} description={report.finishedAtMs
                ? formatDate(report.finishedAtMs, { dateStyle: 'medium', timeStyle: 'short' })
                : t('report.timeUnavailable')} align="center">
                <StatusPill tone={reportTone(report.status)}>{t(`statuses.${report.status}`)}</StatusPill>
              </ConfigPageRow>
              {report.domainResults.map((result) => (
                <ConfigPageRow
                  key={result.domain}
                  label={t(`domains.${result.domain}`)}
                  description={t('report.domainCounts', {
                    imported: formatNumber(result.imported),
                    skipped: formatNumber(result.skipped),
                    conflicts: formatNumber(result.conflicts),
                  })}
                  align="center"
                >
                  <StatusPill tone={domainTone(result.state)}>{t(`domainStates.${result.state}`)}</StatusPill>
                </ConfigPageRow>
              ))}
              {retrySelection.groups.length > 0 ? (
                <ConfigPageRow label={t('fields.retryFailed.label')} description={t('fields.retryFailed.description')} align="center">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={busy !== null}
                    onClick={() => void prepareMigration(retrySelection)}
                  >
                    {t('actions.retryFailed')}
                  </Button>
                </ConfigPageRow>
              ) : null}
            </>
          ) : (
            <ConfigPageRow label={t('report.none')}><span /></ConfigPageRow>
          )}
        </ConfigPageSection>
      </ConfigPageContent>
    </ConfigPageLayout>
  );
}

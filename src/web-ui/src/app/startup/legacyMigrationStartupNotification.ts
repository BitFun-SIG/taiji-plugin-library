import { legacyMigrationAPI, type MigrationRunStatus } from '@/infrastructure/api/service-api/LegacyMigrationAPI';
import { i18nService } from '@/infrastructure/i18n';
import { notificationService } from '@/shared/notification-system';

function notificationKind(status: MigrationRunStatus): 'success' | 'warning' | 'error' | 'info' {
  if (status === 'completed') return 'success';
  if (status === 'completed_with_warnings' || status === 'cancelled') return 'warning';
  if (status.startsWith('failed_')) return 'error';
  return 'info';
}

export async function showLegacyMigrationStartupNotification(
  storage: Pick<Storage, 'getItem' | 'setItem'> = sessionStorage,
): Promise<void> {
  const status = await legacyMigrationAPI.getStatus();
  const report = status.startupReport;
  if (!report) return;

  const noticeKey = `openbitfun:legacy-migration-notice:${report.runId}`;
  if (storage.getItem(noticeKey) === 'shown') return;

  await i18nService.loadNamespace('settings/legacy-migration');
  storage.setItem(noticeKey, 'shown');
  const namespace = 'settings/legacy-migration';
  const openReport = () => {
    void import('@/shared/services/ide-control').then(({ quickActions }) => {
      quickActions.openSettings({ pageId: 'data.migration' });
    });
  };
  const options = {
    title: i18nService.t(`${namespace}:startupNotification.title`),
    duration: 0,
    actions: [{
      label: i18nService.t(`${namespace}:actions.viewReport`),
      variant: 'primary' as const,
      onClick: openReport,
    }],
    metadata: {
      source: 'legacy-migration-startup-result',
      runId: report.runId,
      status: report.status,
    },
  };
  const message = i18nService.t(`${namespace}:startupNotification.statuses.${report.status}`);
  notificationService[notificationKind(report.status)](message, options);
}

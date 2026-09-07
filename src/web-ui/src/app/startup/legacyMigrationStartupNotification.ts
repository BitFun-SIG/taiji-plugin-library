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
  const startupError = status.startupError;
  const report = status.startupReport;
  if (!startupError && !report) return;

  await i18nService.loadNamespace('settings/legacy-migration');
  const namespace = 'settings/legacy-migration';
  const openMigrationSettings = () => {
    void import('@/shared/services/ide-control').then(({ quickActions }) => {
      quickActions.openSettings({ pageId: 'data.migration' });
    });
  };

  if (startupError) {
    const noticeKey = `openbitfun:legacy-migration-startup-error:${startupError.code}`;
    if (storage.getItem(noticeKey) === 'shown') return;

    storage.setItem(noticeKey, 'shown');
    notificationService.error(
      i18nService.t('startupNotification.launchFailed', { ns: namespace }),
      {
        title: i18nService.t('startupNotification.title', { ns: namespace }),
        duration: 0,
        actions: [{
          label: i18nService.t('startupNotification.openSettings', { ns: namespace }),
          variant: 'primary' as const,
          onClick: openMigrationSettings,
        }],
        metadata: {
          source: 'legacy-migration-startup-error',
          code: startupError.code,
          recoverable: startupError.recoverable,
        },
      },
    );
    return;
  }

  if (!report) return;

  const noticeKey = `openbitfun:legacy-migration-notice:${report.runId}`;
  if (storage.getItem(noticeKey) === 'shown') return;

  storage.setItem(noticeKey, 'shown');
  const options = {
    title: i18nService.t('startupNotification.title', { ns: namespace }),
    duration: 0,
    actions: [{
      label: i18nService.t('actions.viewReport', { ns: namespace }),
      variant: 'primary' as const,
      onClick: openMigrationSettings,
    }],
    metadata: {
      source: 'legacy-migration-startup-result',
      runId: report.runId,
      status: report.status,
    },
  };
  const message = i18nService.t(`startupNotification.statuses.${report.status}`, { ns: namespace });
  notificationService[notificationKind(report.status)](message, options);
}

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { showLegacyMigrationStartupNotification } from './legacyMigrationStartupNotification';

const mocks = vi.hoisted(() => ({
  getStatus: vi.fn(),
  loadNamespace: vi.fn(),
  t: vi.fn((key: string) => key),
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
  info: vi.fn(),
}));

vi.mock('@/infrastructure/api/service-api/LegacyMigrationAPI', () => ({
  legacyMigrationAPI: { getStatus: mocks.getStatus },
}));
vi.mock('@/infrastructure/i18n', () => ({
  i18nService: { loadNamespace: mocks.loadNamespace, t: mocks.t },
}));
vi.mock('@/shared/notification-system', () => ({
  notificationService: {
    success: mocks.success,
    warning: mocks.warning,
    error: mocks.error,
    info: mocks.info,
  },
}));

describe('legacy migration startup notification', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.loadNamespace.mockResolvedValue(undefined);
  });

  it('loads the lazy namespace and shows a completed report once', async () => {
    mocks.getStatus.mockResolvedValue({ startupReport: { runId: 'run-1', status: 'completed' } });
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    };

    await showLegacyMigrationStartupNotification(storage);
    await showLegacyMigrationStartupNotification(storage);

    expect(mocks.loadNamespace).toHaveBeenCalledWith('settings/legacy-migration');
    expect(mocks.success).toHaveBeenCalledTimes(1);
    expect(mocks.success.mock.calls[0][1].metadata).toEqual({
      source: 'legacy-migration-startup-result',
      runId: 'run-1',
      status: 'completed',
    });
  });
});

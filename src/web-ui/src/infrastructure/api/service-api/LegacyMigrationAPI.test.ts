import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LegacyMigrationAPI } from './LegacyMigrationAPI';

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock('./ApiClient', () => ({
  api: { invoke: invokeMock },
}));

describe('LegacyMigrationAPI', () => {
  beforeEach(() => invokeMock.mockReset());

  it('uses structured requests through the active ApiClient transport', async () => {
    invokeMock.mockResolvedValue({ runId: 'run-1', mode: 'execute' });
    const selection = { groups: ['memory'] as const };

    await new LegacyMigrationAPI().prepare({ groups: [...selection.groups] });

    expect(invokeMock).toHaveBeenCalledWith('prepare_legacy_migration', {
      request: { selection: { groups: ['memory'] } },
    });
  });

  it('does not expose an in-process execute command', () => {
    const migration = new LegacyMigrationAPI() as unknown as Record<string, unknown>;

    expect(migration.execute).toBeUndefined();
    expect(migration.run).toBeUndefined();
  });
});

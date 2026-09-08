import assert from 'node:assert/strict';
import { access, writeFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { runDesktopWithMigrationRestart } from './desktop-dev-migration.mjs';

const runId = '01234567-89ab-4cde-8fab-0123456789ab';
const write = (directory, file, value) => writeFile(path.join(directory, file), JSON.stringify(value));

test('normal exit does not restart and removes its temporary channel', async () => {
  let directory;
  let calls = 0;
  await runDesktopWithMigrationRestart(async (args, env) => {
    calls++;
    assert.deepEqual(args, []);
    directory = env.OPENBITFUN_DEV_MIGRATION_DIR;
  });
  assert.equal(calls, 1);
  await assert.rejects(access(directory), { code: 'ENOENT' });
});

test('build failures remain failures when no migration was launched', async () => {
  const failure = new Error('build failed');
  await assert.rejects(runDesktopWithMigrationRestart(async () => { throw failure; }), failure);
});

test('migration completion waits for child exit then restores the development host with the run id', async () => {
  let calls = 0;
  let directory;
  let running = true;
  await runDesktopWithMigrationRestart(async (args, env) => {
    calls++;
    if (calls === 1) {
      directory = env.OPENBITFUN_DEV_MIGRATION_DIR;
      await write(directory, 'handoff.json', { runId, pid: 123 });
      // A handoff may also make Tauri report its stopped frontend as a failure.
      throw new Error('frontend stopped');
    }
    assert.equal(running, false);
    assert.deepEqual(args, ['--legacy-migration-run-id', runId]);
    assert.notEqual(env.OPENBITFUN_DEV_MIGRATION_DIR, directory);
  }, {
    isAlive: (pid) => { assert.equal(pid, 123); return running; },
    wait: async () => {
      await write(directory, 'restart.json', { runId });
      running = false;
    },
  });
  assert.equal(calls, 2);
  await assert.rejects(access(directory), { code: 'ENOENT' });
});

test('crashed or mismatched migrators cannot silently restart Desktop', async () => {
  for (const restart of [null, { runId: 'wrong-run' }]) {
    await assert.rejects(runDesktopWithMigrationRestart(async (_, env) => {
      await write(env.OPENBITFUN_DEV_MIGRATION_DIR, 'handoff.json', { runId, pid: 123 });
      if (restart) await write(env.OPENBITFUN_DEV_MIGRATION_DIR, 'restart.json', restart);
    }, { isAlive: () => false }), /without completing its restart handoff/);
  }
});

import { mkdtemp, readFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

async function readOptionalJson(file) {
  try {
    return JSON.parse(await readFile(file, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

function isProcessAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === 'ESRCH') return false;
    throw error;
  }
}

// Tauri stops its frontend server when Desktop hands off to Data Migrator.
// Keep the development supervisor alive and re-enter Tauri after the migrator
// requests a restart, restoring both Vite and the Rust watcher.
export async function runDesktopWithMigrationRestart(run, {
  info = () => {},
  isAlive = isProcessAlive,
  wait = () => delay(250),
} = {}) {
  let restartArgs = [];
  for (;;) {
    const directory = await mkdtemp(path.join(os.tmpdir(), 'openbitfun-dev-migration-'));
    try {
      let failure;
      try {
        await run(restartArgs, { OPENBITFUN_DEV_MIGRATION_DIR: directory });
      } catch (error) {
        failure = error;
      }
      const handoff = await readOptionalJson(path.join(directory, 'handoff.json'));
      if (!handoff) {
        if (failure) throw failure;
        return;
      }
      if (!UUID.test(handoff.runId) || !Number.isSafeInteger(handoff.pid) || handoff.pid <= 0) {
        throw new Error('Invalid development migration handoff');
      }
      info('Waiting for Data Migrator; Desktop will reopen automatically when it finishes');
      while (isAlive(handoff.pid)) await wait();
      const restart = await readOptionalJson(path.join(directory, 'restart.json'));
      if (restart?.runId !== handoff.runId) {
        throw new Error('Data Migrator exited without completing its restart handoff; run desktop:dev to retry');
      }
      restartArgs = ['--legacy-migration-run-id', handoff.runId];
      info('Restarting Desktop through the development launcher');
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  }
}

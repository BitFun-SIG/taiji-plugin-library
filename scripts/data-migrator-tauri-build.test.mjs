import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

import { prepareDataMigratorTauriConfig } from './data-migrator-tauri-build.mjs';
import { productBuildEnvironment } from './product-customization/projections.mjs';
import { resolveProductDefinition } from './product-customization/resolver.mjs';

const ROOT = resolve(import.meta.dirname, '..');
const APP = join(ROOT, 'src', 'apps', 'data-migrator');
const ACME = join(ROOT, 'products', 'fixtures', 'acme', 'product.jsonc');

test('Data Migrator has an independent product and non-updating bundle identity', () => {
  const resolution = resolveProductDefinition({
    rootDir: ROOT,
    productConfig: ACME,
    member: 'dataMigrator',
  });
  const output = prepareDataMigratorTauriConfig(
    join(APP, 'tauri.conf.json'),
    resolution,
    mkdtempSync(join(tmpdir(), 'openbitfun-data-migrator-config-')),
  );
  const config = JSON.parse(readFileSync(output, 'utf8'));

  assert.equal(config.productName, 'Acme Data Migrator');
  assert.equal(config.mainBinaryName, 'acme-data-migrator');
  assert.equal(config.identifier, 'com.acme.data-migrator');
  assert.notEqual(config.identifier, 'com.acme.desktop');
  assert.deepEqual(config.build, { frontendDist: 'ui' });
  assert.equal(config.plugins?.updater, undefined);
  assert.equal(config.bundle.createUpdaterArtifacts, undefined);
  assert.deepEqual(Object.values(config.bundle.resources), ['THIRD_PARTY_NOTICES.md']);
  assert.deepEqual(config.app.windows.map(({ label }) => label), ['migrator']);
});

test('Data Migrator projection provides both trusted sibling binary names', () => {
  const resolution = resolveProductDefinition({
    rootDir: ROOT,
    productConfig: ACME,
    member: 'dataMigrator',
  });
  const environment = productBuildEnvironment(resolution);

  assert.equal(environment.OPENBITFUN_PRODUCT_BINARY_NAME, 'acme-data-migrator');
  assert.equal(environment.OPENBITFUN_DATA_MIGRATOR_BINARY_NAME, 'acme-data-migrator');
  assert.equal(environment.OPENBITFUN_DESKTOP_BINARY_NAME, 'acme-desktop');
});

test('Data Migrator dependency and command closure stays migration-only', () => {
  const manifest = readFileSync(join(APP, 'Cargo.toml'), 'utf8');
  const source = readFileSync(join(APP, 'src', 'app_state.rs'), 'utf8');
  const registration = readFileSync(join(APP, 'src', 'lib.rs'), 'utf8');
  const capability = readFileSync(join(APP, 'capabilities', 'migrator.json'), 'utf8');

  assert.match(manifest, /openbitfun-core[^\n]+features = \["legacy-migration"\]/);
  for (const forbidden of ['product-full', 'openbitfun-agent-runtime', 'plugin-runtime']) {
    assert.equal(manifest.includes(forbidden), false, `manifest must not include ${forbidden}`);
  }
  assert.match(source, /product_assembly_plan_for_profile\(DeliveryProfile::DataMigrator\)/);
  assert.match(registration, /tauri::generate_handler!/);
  assert.match(
    readFileSync(join(ROOT, 'scripts', 'data-migrator-tauri-build.mjs'), 'utf8'),
    /windowsHide:\s*true/,
  );
  for (const forbidden of ['fs:', 'shell:', 'updater:', 'dialog:']) {
    assert.equal(capability.includes(forbidden), false, `capability must not include ${forbidden}`);
  }
});

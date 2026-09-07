#!/usr/bin/env node
/** Builds the standalone, non-updating Data Migrator Tauri bundle. */
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { extractProductConfigArg } from './product-customization/cli.mjs';
import { productBuildEnvironment } from './product-customization/projections.mjs';
import { resolveProductDefinition } from './product-customization/resolver.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const APP_DIR = join(ROOT, 'src', 'apps', 'data-migrator');

export function prepareDataMigratorTauriConfig(
  baseConfigPath,
  resolution,
  outputDirectory = join(APP_DIR, 'gen'),
) {
  if (resolution.assembly.member !== 'dataMigrator') {
    throw new Error('Data Migrator packaging requires the dataMigrator product member.');
  }
  const config = JSON.parse(readFileSync(baseConfigPath, 'utf8'));
  const productName = resolution.productNames[resolution.assembly.fallbackLocale]
    ?? resolution.productNames[resolution.assembly.defaultLocale];
  config.productName = productName;
  config.mainBinaryName = resolution.assembly.binaryName;
  config.identifier = resolution.assembly.bundleId;
  config.build = {
    frontendDist: config.build?.frontendDist || 'ui',
  };
  if (config.plugins) {
    delete config.plugins.updater;
    if (Object.keys(config.plugins).length === 0) delete config.plugins;
  }
  if (config.bundle) delete config.bundle.createUpdaterArtifacts;

  mkdirSync(outputDirectory, { recursive: true });
  const output = join(
    outputDirectory,
    `tauri.${resolution.assembly.assemblyDigest}.generated.conf.json`,
  );
  writeFileSync(output, `${JSON.stringify(config, null, 2)}\n`, 'utf8');
  return output;
}

function tauriArguments(raw) {
  let offset = 0;
  while (raw[offset] === '--') offset += 1;
  return raw.slice(offset);
}

async function main() {
  const { productConfig, forwardArgs } = extractProductConfigArg(
    tauriArguments(process.argv.slice(2)),
  );
  const resolution = resolveProductDefinition({
    rootDir: ROOT,
    productConfig,
    member: 'dataMigrator',
  });
  Object.assign(process.env, productBuildEnvironment(resolution));
  process.env.CI = 'true';
  const generated = prepareDataMigratorTauriConfig(
    join(APP_DIR, 'tauri.conf.json'),
    resolution,
  );
  console.log(`[product] dataMigrator ${resolution.assembly.assemblyDigest}`);

  const tauriBin = join(ROOT, 'node_modules', '.bin', 'tauri');
  const result = spawnSync(tauriBin, ['build', '--config', generated, ...forwardArgs], {
    cwd: APP_DIR,
    env: process.env,
    stdio: 'inherit',
    shell: true,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  process.exit(result.status ?? 1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error?.stack || error);
    process.exit(1);
  });
}

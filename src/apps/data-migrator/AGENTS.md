# Data Migrator Agent Guide

Scope: this guide applies to `src/apps/data-migrator`.

This app is the offline, local-only host for importing legacy BitFun data. It
must remain a separate executable and WebView identity from Desktop.

## Guardrails

- Select `DeliveryProfile::DataMigrator` and only the Core
  `legacy-migration` feature. Do not add `product-full`, Agent Runtime,
  plugin runtime, normal session startup, updater, shell, or frontend
  filesystem capabilities.
- Accept only the handoff `run_id` on the command line. Derive the request path
  from `MigrationRoots`; never accept a request or executable path from UI or
  command-line input.
- Keep all filesystem, process, credential, and restart work in Rust. The UI
  may call only the typed commands registered in `src/lib.rs`.
- Report domain, phase, and counts. Do not invent progress percentages or emit
  secrets, user content, credential values, or absolute paths in errors.
- Cancellation is advisory and may be honored only at engine-declared safe
  boundaries. Closing during execution requests cancellation and keeps the
  window open until a safe boundary.
- The migrator never updates itself. Resolve Desktop as a verified sibling
  binary using product-definition projections and the trusted installation
  resolver.

## Verification

```bash
cargo test -p openbitfun-data-migrator
node --test scripts/data-migrator-tauri-build.test.mjs
```

Run `pnpm run check:core-boundaries` when dependencies or delivery-profile
selection change. Packaging, signing, and UI interaction are separate explicit
verification steps.

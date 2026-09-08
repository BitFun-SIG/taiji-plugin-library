# Legacy migration service

This crate owns local offline migration primitives, authenticated handoffs, and
writer-process inspection. Keep product selection and UI in their existing
assembly and app owners.

Process inspection must fail explicitly when inventory is unavailable. Never
interpret an inspection error as an empty writer list. On macOS, inspect executable
names with the system `ps`, preserving full bundle paths and excluding arguments.

## Focused verification

For process inventory, writer classification, and handoff changes:

```bash
cargo test -p openbitfun-legacy-migration --lib handoff::tests
```

The macOS inventory test exercises the real host process list and must run on
macOS; parser and classification fixtures run on all test platforms. These local
checks do not establish remote-workspace, remote-control, peer, or dispatch behavior.

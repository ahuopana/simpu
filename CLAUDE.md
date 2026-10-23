# Working on Simpu

Windows-only SSH client. Rust core + Tauri 2 + xterm.js UI. Read
`docs/adr/` before changing architecture; add a new ADR for any decision that
is hard to reverse.

## Layout
- `crates/simpu-core`: platform-independent logic. No Windows APIs here.
- OS-specific code goes behind traits in `simpu-core::platform` (ADR 0004).
- `docs/`: development guide and ADRs. README is for users only.

## Commands
- `cargo test --workspace` : all unit and integration tests (Linux container is fine)
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`

## Rules
- Until 1.0, push directly to `main`, but only code whose tests pass locally.
  After 1.0, use pull requests.
- Prefer automated tests over manual checks. If something is hard to test,
  build a test scaffold (fake server, fake platform adapter) instead.
- Secrets: use `credentials::Secret`; never log or serialize secret values.
- Telnet is plaintext: surface a warning wherever it is offered.
- Keep README for users; put developer material in `docs/`.

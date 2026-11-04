# The Wire-forked Codex

This branch (`wire/brand`) is OpenAI's Codex CLI with two changes, for the
Wire CLI's standalone install (rittmananalytics/wire#408):

- the welcome screen draws the Wire wordmark and stepped mark instead of the
  Codex blossom (`codex-rs/tui/src/empty_state_animation/renderer.rs`);
- the session header and status card read "Wire (Codex v...)" instead of
  "OpenAI Codex" (`codex-rs/tui/src/history_cell/session.rs`).

Everything else is upstream Codex. The binary keeps Codex's configuration,
sign-in and sessions under `~/.codex`, so `codex login` or `OPENAI_API_KEY`
serves it, and it reads the same `.codex/` hooks and agents in a repository.

## Build

```
rustup toolchain install 1.95.0        # the pin in codex-rs/rust-toolchain.toml
cd codex-rs && CARGO_NET_GIT_FETCH_WITH_CLI=true cargo build --release --bin codex
```

About 11 minutes on an Apple M5 Max for a clean build. The Wire repository's
`wire/scripts/build-agent.sh` runs this and packages `target/release/codex`
as `wire-agent-<version>-<os>-<arch>.tar.gz` for the installer.

## Keeping up with upstream

The change is small and deliberate: rebase `wire/brand` on an upstream tag
when a Wire release needs a newer Codex, rather than tracking `main`. The
upstream snapshot tests that name "OpenAI Codex" are expected to differ on
this branch and are not run for the Wire build.

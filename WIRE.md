# The Wire-forked Codex

This branch (`wire/brand-0.166.0`, cut from upstream tag `rust-v0.166.0`) is
OpenAI's Codex CLI with three changes, for the
Wire CLI's standalone install (rittmananalytics/wire#408):

- the welcome screen draws the Wire wordmark and stepped mark instead of the
  Codex blossom (`codex-rs/tui/src/empty_state_animation/renderer.rs`);
- the session header and status card read "Wire (Codex v...)" instead of
  "OpenAI Codex" (`codex-rs/tui/src/history_cell/session.rs`).

- a native Anthropic provider (`wire_api = "anthropic"`): Codex's Responses
  request is translated to the Messages API and Anthropic's event stream back
  into Responses events (`codex-rs/codex-api/src/anthropic.rs`, with unit
  tests), so Claude runs with tools and prompt caching and no proxy. Configure:

  ```
  -c 'model_providers.anthropic.base_url="https://api.anthropic.com/v1"'
  -c 'model_providers.anthropic.wire_api="anthropic"'
  -c 'model_providers.anthropic.env_http_headers={"x-api-key"="ANTHROPIC_API_KEY"}'
  -c model_provider=anthropic -m claude-sonnet-5-5
  ```

Everything else is upstream Codex. The Wire build stamps the workspace
version as `<wire version>+codex.0.166.0`, so `wire-agent --version` says what
it is. The Wire CLI runs the agent on an OpenAI API key, passed as
`CODEX_API_KEY` (which `codex exec` prefers to a stored login); ChatGPT
sign-in stays with OpenAI's own Codex until OpenAI confirms a modified client
may use it. Do not set `forced_login_method = "api"` for this: Codex logs a
stored ChatGPT session out when it sees it. The binary keeps Codex's configuration,
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

One branch per upstream release tag: `wire/brand-<codex version>`, the Wire
commits cherry-picked onto `rust-v<codex version>`. A Wire release that needs
a newer Codex gets a new branch; old branches stay so a past build can be
repeated. `wire/brand` (cut from main during the concept run) is kept for
history. The
upstream snapshot tests that name "OpenAI Codex" are expected to differ on
this branch and are not run for the Wire build.

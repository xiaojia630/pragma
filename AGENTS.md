# pragma

pragma makes coding agents follow the engineering specs a repo's docs already state. This repo
holds the `pragma` binary: a Rust workspace with two crates. User docs are in
[docs/pragma/](docs/pragma/README.md).

## Layout

- `crates/pragma-engine/` - the library:
  - doc discovery and paragraphs (`docs.rs`, `markdown.rs`, `frontmatter.rs`);
  - extraction with the user's model (`extract/`);
  - the clause index and the lock (`index.rs`, `lock.rs`, `lock_update.rs`);
  - rules and the ast-grep matcher (`rules.rs`, `matcher.rs`);
  - which findings an edit introduced (`introduced.rs`, `edit.rs`);
  - drift, audit and backtest (`drift.rs`, `audit.rs`, `backtest.rs`);
  - hook state (`gate.rs`, `events.rs`, `settings.rs`);
  - exceptions (`overrides.rs`).
- `crates/pragma-cli/` - the `pragma` binary:
  - one module per command (`*_cmd.rs`) and the JSON output types (`json.rs`);
  - integration tests in `tests/`, which run the built binary against temporary git repos.
- `docs/pragma/` - user docs. `docs/superpowers/` - the v1 spec and spike findings.
- `.pragma/` - pragma's own settings for this repo.
- `scripts/package-macos.sh` - builds the universal macOS binary and the preview zip.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
# Edit-check latency; p95 must stay under 20 ms:
cargo test --release -p pragma-cli --test latency -- --ignored --nocapture
sh scripts/package-macos.sh       # target/pragma-<version>-macos.zip
```

`rust-toolchain.toml` pins the Rust toolchain; rustup installs it on first use. CI
(`.github/workflows/ci.yml`) runs fmt, clippy and the tests on Linux and macOS.

## Rules

- **The edit check runs on the agent's edit path.** `pragma hook pre-tool-use` has a 20 ms
  typical budget and a 50 ms hard cap. It starts no process and uses no network.
- **Hooks fail open.** Any error lets the edit through (exit 0); only a real block exits 2.
- **Every command has `--json`.** Exit codes: 0 for success, 1 for an error, 2 only for a block.
- **Processes run with argv only**, never a shell string. Paths are normalized before matching.
- **One writer.** `.pragma/` is written only in the root checkout, never in a worktree and never
  during a hook.
- **Model calls use the user's account.** They go through the user's own `claude` CLI with
  `PRAGMA_JUDGE=1`, never through pragma's keys.
- **Tests stay offline.** They never call a real model or the network; `PRAGMA_CLAUDE_BIN` points
  them at a fake.
- **Dependencies** must be MIT or Apache-2.0. Commit `Cargo.lock`.
- **Commits** follow Conventional Commits (`feat(pragma-cli): …`, `fix(pragma-engine): …`).

## License

Apache-2.0, see [LICENSE.md](LICENSE.md).

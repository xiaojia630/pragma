# pragma v1 architecture

Status: approved 2026-10-15, then revised the same day to add value on existing code (5.10).
Builds on the brief in `CLAUDE.md`; where this spec changes a decision recorded there, it says so,
and `CLAUDE.md` reflects it.

## 1. Goal

Make every coding agent follow the engineering specs a repo already has: the markdown its people
write (AGENTS.md, CLAUDE.md, CONTRIBUTING, ADRs, design docs, glossaries, Cursor rules), plus a
built-in good-engineering pack. v1 serves one solo developer running Claude Code, locally first.

v1 ships the five things the brief locks: docs from minute one with a Rulebook, every turn
checked, bad edits blocked mid-task, send back to fix, and the Decisions view plus a backtest. It
also gives value on the code a repo already has, before any agent runs: a repo health report
(docs that point at code that's gone, code that already breaks the docs, and history) and
burn-down tasks that send agents to fix what's there.

## 2. Principles

1. **Write for humans; pragma translates for agents.** Docs stay ordinary prose. pragma never
   requires syntax that makes a doc worse to read. Frontmatter is optional and no magic comments
   exist. Deleting pragma loses enforcement, never knowledge.
2. **The ladder.** Every clause sits on one rung: `context`, `judged` or `blocking` (section 3).
   Clauses climb only with evidence.
3. **Blocking must be earned and exact.** A wrong block stops the agent with a wrong fix, so
   only deterministic checks block, and only on violations the edit itself introduces.
4. **Fast where the agent waits.** Lookup and block checks finish in under 20 ms, with a hard
   cap of 50 ms. Anything slower runs beside the agent, is cached by content hash, and fails
   open. Budgets are in section 7.
5. **Fail open, visibly.** When pragma cannot check, the agent proceeds and the UI says
   "unchecked" with the reason. Silence never means clean.
6. **Your model, not ours.** Model work runs on the user's own agent CLI and account
   (`claude -p`). Doc and code content never leaves the user's machines except to that CLI.
7. **Coexist.** pragma runs beside whatever other hooks, plugins and configs a user has, and
   changes nothing it does not own.

## 3. Concepts

| Term          | Meaning                                                                                     |
| ------------- | ------------------------------------------------------------------------------------------- |
| Spec doc      | A markdown file pragma reads as a source of norms (discovery rules in 5.2).                 |
| Paragraph     | The unit of extraction, anchored by doc path, heading path and a content hash.              |
| Clause        | One specific thing a paragraph asks for, with a rung, a scope, an optional check and a fix. |
| Scope         | Globs a clause applies to. Empty means the whole repo.                                      |
| Override      | A user correction to a clause, stored in `.pragma/` and anchored to the clause's paragraph. |
| Exception     | A finding the user marked "not a violation", stored in `.pragma/` and keyed to its code.    |
| Lock          | `.pragma/pragma.lock`: the blocking clauses, compiled. Generated, committed, authoritative. |
| Root checkout | The repo's main worktree. The only place pragma writes repo files (section 4).              |
| Turn          | The agent's work between a user prompt and the agent stopping.                              |
| Turn report   | The result of checking one turn: violations, flags, send-backs, and anything unchecked.     |
| Drift         | A doc naming a file, script or code name the repo no longer has (5.10).                     |
| Baseline      | The violations already in the code, found by an audit (5.10).                               |
| Pack          | The built-in good-engineering specs shipped with pragma, at the lowest precedence.          |

Rungs say *when* a clause is enforced; its check says *how*:

- **context**: handed to the agent. Never checked.
- **judged**: checked when the turn ends, by its code check if it has one, otherwise by the
  model. Never blocks.
- **blocking**: checked by code during each edit, and again at turn end to catch edits made
  through Bash. Requires a library check or a promoted pattern, and an entry in the lock.

The brief's "level" is the rung. Turning a clause off is an override, not a rung.

The clause record:

```
Clause {
  id        hash of (doc path, para_hash, ordinal within paragraph); changes when the paragraph does
  anchor    { path, heading_path, para_hash, lines, quote }; re-finds the paragraph after edits
  text      the instruction, normalized
  rung      context | judged | blocking
  scope     globs
  check?    { kind: library, rule, params } | { kind: pattern, lang, ast_grep, examples, promoted }
  fix       how to fix a violation
  origin    repo | pack
}
```

Things that must survive doc edits (overrides) use the anchor, never the id.

## 4. Files and state

### In the repo

```
.pragma/
  README.md        what this folder is, for humans who find it
  specs/**/*.md    extra specs written for pragma; Markdown, optional frontmatter (scope, rung)
  overrides.md     user corrections, promotions and exceptions; entries in frontmatter, notes in
                   the body
  config.md        repo settings in frontmatter: send-back mode and caps, model, doc
                   include/exclude globs, pack on/off
  pragma.lock      generated: the blocking clauses (the one generated file here)
```

**One writer per repo.** pragma writes `.pragma/` only in the root checkout, found through
`git rev-parse --git-common-dir`. It never writes `.pragma/` in a task worktree and never writes
during a hook. Every worktree reads the root checkout's working copy of `.pragma/`, committed or
not, so a Rulebook change applies to every running task at once and becomes durable when the
user commits it. If the root checkout has no `.pragma/` (or the repo is bare), a worktree uses its
own committed copy. Whichever copy applies is the *effective* `.pragma/`.

Other spec docs (AGENTS.md, ADRs and so on) are read from each worktree, so a branch sees its own
docs.

### On the host

`$PRAGMA_HOME` (default `~/.pragma/`, `%USERPROFILE%\.pragma\` on Windows):

- `bin/pragma`: a stable path to the binary, refreshed by the app (section 10). Hooks call this.
- `cache/`: extraction results keyed by `(para_hash, extractor version, model)`, shared by every
  repo and worktree on the host, pruned by age.

`<git common dir>/pragma/` (per repo, shared by its worktrees):

- `paused`: present when the user paused pragma for this repo on this host.
- `feedback.jsonl`: findings marked "not a violation", with their context, as candidates for the
  golden set.
- `writes.json`: hashes of the files pragma itself last wrote, for tamper detection.
- `extract.lock` and `extract.guard`: keep extraction to one run per repo at a time. The run
  holds an OS lock on `extract.guard`, released when it exits however it exits; `extract.lock`
  records its pid and start time for status.
- `backtest-cache.json`: the backtest's raw findings per commit (5.7), keyed by a hash of the
  lock's checks, so a run checks only commits it has not seen; a lock change starts it over.

`<git dir>/pragma/` (per checkout, so per task worktree):

- `index.json`: the checkout's full clause index, all rungs.
- `extract-progress.json`: this checkout's extraction run as it goes (paragraphs done of total),
  and when and why it stopped (a missing or logged-out model, a cancel), for status and the app.
- `allow.json`: clauses the user allowed for this task.
- `turns/<session>/<n>.base` and `<n>.json`: a turn's baseline tree id and its report. Reports
  are pruned after 30 days and disappear with the worktree.
- `drift.json`, `audit.json` and `audit-history.jsonl`: the latest drift and audit findings, and
  the counts per clause from every audit run (5.10). The app shows the root checkout's.
- `backtest.json`: the latest backtest of this checkout's history (5.7), with today's exceptions
  applied. The app shows the root checkout's.
- `events.jsonl`: the edit check's decisions, one JSON line each: blocked and escalated (with the
  clause, line, matched text and the check's time), unchecked (with the reason), and cleared (the
  session's next edit to the file no longer broke a clause that had blocked it). `pragma events`
  lists them with their clauses for the app's task view.
- `blocks/<session hash>.json`: a session's blocks per clause and file, counted for escalation
  (5.5), and which of them its next edit to that file has not yet cleared.

Turn reports and findings are files, not desktop database rows, so v1 needs no Drizzle migration
and works the same on SSH hosts.

## 5. Components

### 5.1 The `pragma` binary (Rust)

One binary holds all host-side logic. The app is UI plus a launcher; the terminal uses the same
binary directly, and CI can. Crates: `crates/pragma-engine` (library), `crates/pragma-cli`
(binary), `crates/pragma-pack` (the pack, 5.9), `crates/pragma-evals` (golden set, section 11).

Fast path (inside the 20 ms budget):

- `pragma hook pre-tool-use` reads Claude Code's hook JSON on stdin, rebuilds the file as it
  would be after the edit (from Edit's old and new strings, MultiEdit's edits, or Write's
  content; an edit whose old string is not found is allowed), runs the blocking checks scoped to
  that path on the before and after contents, and blocks only on violations that are new. A
  violation is keyed by rule, normalized matched text and occurrence count, never by line
  number. An internal 50 ms deadline allows the edit and records it as unchecked.
- `pragma context <path>` returns the clauses for a path, within the context budget (5.3).

Slow path (seconds, never on the edit path):

- `pragma extract` turns spec docs into clauses (5.2).
- `pragma lock` updates the lock (5.2).
- `pragma drift` checks docs against the repo (5.10).
- `pragma audit [--area <path> | --changed-from <ref>] [--judged]` checks the whole tree against
  the docs (5.10). `--changed-from` checks only what a branch changed since its merge base with
  `<ref>`, committed or not, to measure a fix task's progress; like `--area`, it is never saved.
- `pragma hook session-start | prompt-submit | stop | session-end` (5.3, 5.4).
- `pragma backtest [--since <n>]` (5.7).
- `pragma events [--limit <n>] [--since-ms <ms>]` lists this checkout's edit-check events
  (section 4) from the last 256 KB of the log, oldest first, each joined with its clause from
  the effective lock, so a task worktree shows its own task's stopped edits.
- `pragma suggest-rewording <clause>` drafts a clearer paragraph (5.6).
- `pragma hooks install | uninstall` (5.3).
- `pragma init` creates `.pragma/` in the root checkout; the app's enable flow calls it (5.6).
- `pragma lock check` validates the lock: scopes compile, and each pattern matches its bad
  example and not its good one.
- `pragma exception add <finding> --reason <reason>` records an exception (5.10).
- `pragma debug anchor <doc>:<line>` prints the anchor of the paragraph at that line.
- `pragma pause | resume` stops and restarts pragma for this repo on this machine (5.6).
- `pragma version` prints the binary version and the lock format it reads.
- `pragma eval` scores extraction and judging against the golden set (section 11).

Every command has `--json` output for the app. Exit codes: 0 for success and for every fail-open
path, 2 only for a block, 1 for a command-line error outside hooks.

**Checks.** Built on `ast-grep-core` (MIT, tree-sitter). Library rules are parameterized
templates; generated checks are ast-grep patterns; both run through one matcher, over an edit, a
diff or the whole tree. v1 languages: TypeScript and JavaScript, plus path and text rules for any
file. Each library rule declares whether it applies to edits, to diffs and audits, or both. The
starting library:

- **Protected paths**: no create, modify or delete under a glob through the edit tools (for
  example "do not hand-edit numbered Drizzle migrations"). Edits only, so generators run through
  Bash still work.
- **Import boundaries**: banned imports per path; area A must not import area B.
- **Forbidden patterns**: APIs and code shapes a doc rules out (`require(`, a non-null `!`
  after `asProvisioned(...)`).
- **File naming**: case and suffix rules per glob.

**Paths.** Paths from hook input are resolved against the hook's `cwd`, with `..` and symlinks
resolved, before scope matching, so a rule cannot be dodged by spelling a path differently.
Edits outside the repo are not checked, except edits to spec docs or the effective `.pragma/`,
which are flagged (5.4).

**Fast path rules.** Measured on this repo: starting Node takes 24 ms and `git --version` takes
13 ms, so the edit check starts no other process and uses no network; it finds the git directory
by reading `.git` files directly. Parsing the repo's largest TypeScript file (1,964 lines) takes
about 5 ms, and running four patterns over it one at a time takes about 10 ms, but about 3 ms as
one combined pass. So the edit check:

1. runs path rules first, which need no parsing;
2. skips parsing when the new content lacks a word a pattern needs (every literal identifier in
   a pattern must appear for it to match);
3. parses the new content once and runs all patterns in one combined pass;
4. parses the old content only when the new content has violations;
5. skips content rules for files over 512 KB and records the edit as unchecked.

That keeps typical edits at a few milliseconds and the largest file in this repo under 20 ms.

**How it is built.** A Cargo workspace at the repo root, with `rust-toolchain.toml` pinning Rust
1.98.0 with clippy and rustfmt, and the crates under `crates/`. The binary is named `pragma`.
Dependencies: `ast-grep-core` and `ast-grep-language` (TypeScript and JavaScript grammars only),
`clap`, `serde`, `serde_json`, `serde_norway` (YAML frontmatter), `globset`, `ignore`,
`pulldown-cmark`, `rayon`, `blake3`, `similar` and `anyhow`. Integration tests run the built
binary through Cargo's `CARGO_BIN_EXE_pragma` against temporary git repos. The JSON types are
generated into the app's slice as TypeScript with `ts-rs` in S5, with a CI check that they are
current.

### 5.2 Doc discovery and extraction

**Discovery.** Spec docs are:

- `AGENTS.md`, `CLAUDE.md` and `GEMINI.md` at any depth, `.github/copilot-instructions.md`,
  `.cursor/rules/**/*.mdc` and `.cursorrules`;
- `CONTRIBUTING.md` and `README.md` files (README content is mostly context);
- `docs/**/*.md` (ADRs, design docs, glossaries) and root-level glossaries such as `CONTEXT.md`;
- `.pragma/specs/**/*.md`;
- markdown files these docs link to, followed within the repo, so a doc tree that AGENTS.md
  points to (such as this repo's `agents/**`) is included;
- minus anything gitignored, `node_modules` and vendored directories, `CHANGELOG*`, `LICENSE*`,
  and the include and exclude globs in `.pragma/config.md`.

Docs marked superseded, deprecated or rejected (an ADR status line, or frontmatter `status`)
produce no clauses. Claude Code's `@path` imports are followed, and paragraphs with the same
hash collapse to one. Frontmatter is honored where it exists: `.mdc` `globs` become scope;
`.pragma/specs` may set `scope` and `rung`.

**Paragraphs.** A paragraph is a prose paragraph or a top-level list item with its sub-items. A
code block attaches to the paragraph before it. A table is one paragraph.

**Extraction.**

1. For each paragraph not in the cache, ask the model (batched, several paragraphs per call) for
   its clauses: text, proposed rung, scope, fix, and either a library rule with params or an
   ast-grep pattern with one bad and one good example.
2. Validate: library params against the rule's schema; scope globs by compiling them; patterns
   by compiling them and checking they match the bad example and not the good one. A clause
   whose check fails validation drops to `judged` with no check.
3. Default rung: a library match is `blocking`. Otherwise `judged` if the clause is a norm that
   can be checked from a diff, else `context`. Generated patterns start at `judged` until
   promoted (5.7).
4. Scope: a nested doc scopes to its directory; paths named in the text become proposed globs;
   frontmatter wins over both; overrides win over everything.
5. Precedence when clauses conflict: override > closer doc > repo-root doc > pack. Detected
   contradictions are listed in the Rulebook as rewording suggestions.

**Model calls.** `claude -p` with `PRAGMA_JUDGE=1`, no tools, and JSON output that must match a
schema. The model alias comes from `.pragma/config.md`; the default is picked in spike S0a. If
the CLI is missing or logged out, the pack and the lock keep enforcing, drift keeps working,
extraction and judging pause, and the Rulebook and badge say so.

**The lock.** `pragma lock` runs in the root checkout after extraction, an override change or a
promotion. It re-derives only entries whose paragraph, override or promotion changed; every
other entry keeps its exact bytes. For a paragraph whose hash matches the lock, the lock's
blocking clauses are authoritative: a fresh extraction can add judged and context clauses for
it, never new blocking ones. So model randomness and new machines cause no lock churn, and a
blocking check only changes through a lock diff the user can review.

Blocking clauses come only from the lock and the pack's own lock. A doc changed only in a task
branch yields judged and context clauses in that worktree, and its blocking clauses arrive once
the change reaches the root checkout and the lock is updated. As a side effect, an agent cannot
loosen a blocking rule by editing a doc in its own branch.

**When extraction runs.** On enable; when the app's file watcher sees a spec doc change
locally; and when `session-start` or `prompt-submit` notices changed doc hashes (a cheap mtime
and hash check), which starts a detached `pragma extract`. `extract.lock` keeps it to one run per
repo. A turn is checked against the index as it was when the turn started.

### 5.3 Agent hooks (Claude Code)

**Ownership.** The binary owns its hook entries. `pragma hooks install` and `uninstall` write
and remove them in the user's Claude config (`$CLAUDE_CONFIG_DIR` or `~/.claude/settings.json`),
tagged with a `PRAGMA_HOOK` marker. The app runs them on enable and on disable. The app's own
hook writer already keeps entries it does not own (`filterUserHooks` in
`packages/core/src/services/agent-plugins/api/plugins/helpers/hooks.ts`), so the two writers do
not clobber each other. Commands call `$PRAGMA_HOME/bin/pragma` and do nothing if it is missing.
Windows hook commands land with Windows packaging (S9); until then `pragma hooks install`
supports macOS and Linux and says so on Windows.

**The gate.** Every pragma hook exits silently unless all hold:

- the git toplevel of the hook's `cwd` has an effective `.pragma/` (section 4);
- the repo is not paused on this host;
- `PRAGMA_JUDGE` and `PRAGMA_DISABLE` are unset.

So pragma enforces in any Claude session in an enabled repo, whether started by the app or in a
terminal, and nowhere else. A plain directory without git is not supported.

| Event                   | Command                     | Does                                                          | Timeout |
| ----------------------- | --------------------------- | ------------------------------------------------------------- | ------- |
| SessionStart            | `pragma hook session-start` | Hands over repo-wide context; starts re-extraction if needed. | 5 s     |
| UserPromptSubmit        | `pragma hook prompt-submit` | Closes any unfinished turn; records the new baseline.         | 10 s    |
| PreToolUse (file edits) | `pragma hook pre-tool-use`  | Blocks new violations; area context on first touch (S0c).     | 1 s     |
| Stop                    | `pragma hook stop`          | Turn check and send-back (5.4, 5.5).                          | 120 s   |
| SessionEnd              | `pragma hook session-end`   | Closes the last turn and checks it in the background.         | 5 s     |

The file-edit matcher covers Edit, Write and MultiEdit; exact tool names are confirmed in S0c.
Notebook edits are not blocked in v1; the turn check still sees them. Subagents' edits pass
through the same PreToolUse hook, and the turn diff covers whatever they changed. When a hook
times out, Claude Code carries on, which is the fail-open behavior we want.

**Context hand-off.** Within a budget (about 2k tokens per session and 1k per area, tuned in
S4), clauses are ranked by scope specificity. Clauses from docs the agent already loads by
itself are skipped; for Claude Code that is CLAUDE.md files and their `@` imports. The first time
a turn touches an area, its clauses are handed over. Which events accept added context is
confirmed in S0c; the fallback is PostToolUse after the first edit in that area.

**What the agent sees on a block**, framed as review feedback, never as commands to run:

```
pragma blocked this edit.
Rule (AGENTS.md:142, "Agent Guardrails"): never use asProvisioned(...)!; use explicit null checks.
Found: src/core/features/tasks/browser/task-view.tsx:88  asProvisioned(task)!.id
Fix: const p = asProvisioned(task); if (!p) return; then use p.id.
If the user explicitly asked for this change, stop and tell them pragma blocked it; they can
allow it for this task.
```

### 5.4 Turn checks

- **Baseline.** On prompt submit, write the worktree, including untracked files and respecting
  `.gitignore`, to a git tree through a temporary index seeded from the real index (so only
  changed files are hashed). Store the tree id as the turn's base.
- **Unfinished turns.** If the previous turn has a baseline but no report (the user interrupted
  it, or the session ended), prompt submit closes it: the snapshot it takes is both that turn's
  end and the new turn's baseline, and the old turn is checked in the background without
  send-back. SessionEnd does the same for the last turn.
- **Diff.** On stop, snapshot again and diff the two trees. An empty diff ends the check at once.
  Binary files, files marked generated in `.gitattributes` (`linguist-generated`), package
  lockfiles and config excludes are skipped.
- **Code checks.** Blocking and judged clauses that have a check run on the diff, new violations
  only. This catches edits made through Bash, which PreToolUse cannot see. Edit-only rules
  (protected paths) do not run here.
- **Judged checks.** One model call per turn, split into parallel calls past a size budget, with
  the judged clauses in scope, the diff, and the turn's part of the transcript (from the hook's
  `transcript_path`), so rules about the order of work can be judged. Each violation must cite a
  clause id from the list and either a line the turn added or, for rules about how the work was
  done, a step in the transcript; anything else is dropped. Cached by hash of (clauses, diff).
  Past a size cap, judged clauses are marked unchecked for the turn.
- **Tamper flags.** Changes to spec docs or the effective `.pragma/` are never blocked and always
  flagged. pragma compares against `writes.json`, so its own writes are not flagged and changes
  made from outside the worktree are.
- **Drift flags.** A turn that deletes or moves a file a spec doc names is flagged, with the doc
  fix offered (5.10), so drift is caught as it happens.
- **Report.** Written to the turn report file and sent to the app (5.6).

### 5.5 Send back

- **Auto** (the default once the judge meets its precision bar, section 11). The Stop hook
  returns `{"decision":"block","reason":…}`, and Claude Code keeps going with the reason as its
  next instruction. The reason uses the block message format above for each violation.
- **Manual.** The Stop hook records and returns at once; the judge runs in the background; the
  app's "Send back" button types the fix request into the agent's session.
- **Caps.** At most 2 automatic send-backs per prompt, tracked with the hook's
  `stop_hook_active` flag and a counter. Mid-edit, the same clause blocking the same file 3 times
  in a turn lets the next edit through, but only when no other clause still blocks that edit:
  while one does, the clause stays blocked and counted, and it escalates on an edit nothing else
  blocks. Until turn checks exist the count is per session, not per turn, and only an escalation
  starts it over. Both are recorded as "escalated".
- **Escapes for the user.** "Allow for this task" (a clause, written to the worktree's
  `allow.json`) and "Not a violation" (an exception, 5.10). Pausing the repo stops everything on
  this host.

### 5.6 App

A new slice, `src/core/features/pragma/`, following the repo's slice rules: Wire contract in
`api/`, a node controller that runs the binary on the workspace's host with `--json`, browser UI
contributed through `contributions/`.

- **Live events.** When a session runs under the app (the same env guard the app's own hooks
  use: its hook-server port, nonce and session id), the binary also posts block and
  turn-report events to the app's hook server. The app reads report files for detail.
- **Agent status.** Claude Code runs Stop hooks in parallel, so the app's existing Stop hook
  reports the agent idle even when pragma sends it back. With pragma on, the app holds the task
  in "checking" until pragma's turn event arrives or the Stop timeout passes, and holds the
  "agent finished" notification until then. A send-back returns the task to "working".
- **Enable.** First open of a repo offers to enable pragma. It first shows the doc and paragraph
  count and roughly how many model calls extraction will take. Then it creates `.pragma/` in the
  root checkout and offers to commit it, installs hooks, and starts drift, extraction, the audit
  and the backtest.
- **Repo health.** The first-open report, and its home afterwards: docs drift, the audit
  baseline with its trend, and history from the backtest (5.7, 5.10). It leads to the drift and
  burn-down views.
- **Badge** on each task: checking, clean, blocked (n, live), violations (n), sent back,
  escalated, unchecked (reason).
- **Inline violations** on the diff lines in the existing diff view, each citing its paragraph
  with a link to the doc. A violation stores its file, line and matched text; when lines shift
  it is re-found by text, and shown as outdated if it cannot be. Each has "Not a violation" and
  "Allow for this task". Violations about how the work was done have no line; they appear in the
  turn's detail with the transcript step as evidence.
- **Rulebook** view: clauses grouped by doc and paragraph, the pack in its own section, with
  rung, scope, check type, backtest hits and exception count. Actions: turn off, change rung,
  edit scope, edit the check, reword (as an override), and suggest rewording the doc (opens a
  diff of the doc to accept or reject; never automatic). Lists stale overrides and exceptions,
  contradictions, paragraphs that failed extraction, and uncommitted changes in `.pragma/`.
- **Settings.** Pause is per host (`paused`). Send-back mode, caps, model, doc globs and the pack
  switch are repo settings in `.pragma/config.md`.
- **Telemetry.** pragma adds none in v1.

### 5.7 Backtest and promotion

On enable and on demand, in the background, walk the first-parent history of the current branch
(the last 200 commits by default). Each commit is diffed against its first parent, so a merge
counts its branch once. Run blocking checks and candidate patterns, counting only violations
each commit introduced. Results are cached by commit id. A commit that cannot be fully checked
(a partial clone lacks files it needs) is counted as incomplete, not cached, and adds no
violation, since its hits could be wrong either way; the report says how many, in place of an
all-clear.

- **History**: which rules recent commits broke most, and where. Shown in Repo health, in the
  Rulebook (hits per clause) and in the Decisions view, where a decision whose commits introduced
  violations gets a marker and its detail panel lists them.
- **Promotion**: a generated pattern that passed its examples (5.2) and has zero hits is
  promoted to blocking automatically. One with hits is offered in the Rulebook with its hits for
  the user to judge. A promotion is recorded in `overrides.md` and lands in the lock, so it shows
  as a diff.

### 5.8 Overrides and anchoring

Overrides live in the effective `overrides.md`. An override can turn a clause off or change its
rung, scope, wording or check (library params or pattern). An edited check re-runs its examples
and shows its backtest hits before it is saved; a check edited to block lands in the lock like a
promotion. An override names its clause by doc path, heading path, paragraph hash and quote.

When a doc changes: an exact hash match applies; a confident fuzzy match applies and is flagged
in the Rulebook; no match reverts to the derived default and is listed as stale. Pack clauses
never get rewording suggestions, only overrides. Exceptions live in the same file (5.10).

### 5.9 The good-engineering pack

About 20 to 40 clauses of widely agreed practice, written as markdown in `crates/pragma-pack/`.
Examples: no secrets or credentials in code; never skip, disable or weaken tests or checks to get
green; never swallow errors silently; no leftover debug code; no commented-out code; behavior
changes come with tests; keep changes to the task at hand. Maintainers extract it once and commit
the result with its own lock, so it needs no model on the user's side. It can be switched off in
`.pragma/config.md`. Its content is reviewed before S8 ships.

### 5.10 Existing code: drift, audit and burn-down

Value on the code a repo already has, before any agent runs. All three work from the terminal
first (S2) and in the app's Repo health (S5).

**Drift: docs checked against the repo.** `pragma drift` checks what spec docs name, with no
model:

- File and directory paths in code spans and links must exist, resolved against the doc's
  directory, the repo root and each package root (a directory with a `package.json`,
  `Cargo.toml`, `go.mod` or `pyproject.toml`).
- Commands such as `pnpm run <script>` must name a script in the matching `package.json`;
  `make <target>` must name a Makefile target.
- A single code name in a code span (for example `getTaskManagerStore`) must appear somewhere in
  tracked code. These are reported apart from the headline count, since they are less certain.
- ADRs, and docs marked superseded or historical, are skipped: they describe the past.
- When a missing file was moved (a file of the same name elsewhere, or a rename in git history),
  the finding carries the new path, and the fix is offered as a doc diff to accept, like a
  rewording suggestion. A finding with no clear fix can be sent to an agent as a task.
- A finding can be marked intended (a path given only as an example, say). That is recorded as
  an exception keyed by the doc's paragraph anchor and the reference.

Drift runs on enable, on demand, and after extraction, and takes seconds. Turns flag the drift
they cause (5.4).

**Audit: code checked against the docs.** `pragma audit` runs every clause that has a code check
over the whole tree instead of a diff, within each clause's scope, skipping generated and
vendored files. Its findings are the repo's baseline. Model-judged clauses run only when asked
(`--judged`), for a chosen area, with the number of model calls shown first; their results are
cached by (clause, file hash). Each run appends counts per clause to `audit-history.jsonl`, so
the trend shows. Blocking and turn checks only count new violations, so agent work cannot add to
the baseline: the count only goes down.

**Exceptions.** "Not a violation" asks why: a *deliberate exception* (the rule is right and this
code is sanctioned, such as a `createRequire` that a "no `require()`" rule did not foresee) or
*the check is wrong* (a false positive). Either way pragma writes an exception to the effective
`overrides.md`, keyed by the clause's anchor, the file and the normalized matched text, so the
finding stays suppressed in future turns and audits, on every machine. The finding and its
context also go to `feedback.jsonl`. An exception whose code changes or disappears is listed as
stale in the Rulebook.

**Burn-down.** Audit and drift findings are grouped by clause and area (top-level directory or
package) into fix tasks of at most 20 findings each. Starting one creates an ordinary task through
the app's existing task creation (`src/core/features/tasks/node/operations/createTask.ts`) with a
prompt that lists the findings, quotes the rule and its fix, and asks for no other changes. Tasks
run in parallel worktrees, and pragma checks every turn as usual. The burn-down view shows the
count per clause over time, tasks in flight, and what merged. Drift that is only a moved path is
fixed as a doc diff, with no agent.

## 6. Key flows

- **First open.** Enable (with cost shown) → pack and any committed lock active at once → drift
  in seconds → extraction with progress → lock updated → audit and backtest → Repo health.
- **Edit.** PreToolUse → gate → lock lookup by path → before and after check → allow, or block
  with the fix. Live "blocked" on the badge.
- **Turn end.** Stop → diff against baseline → code and judged checks → report → auto send-back
  within caps → badge, inline and status update.
- **Interrupted turn.** Next prompt (or session end) closes it → background check, no send-back.
- **Burn-down.** Pick finding groups → start fix tasks → agents fix them in parallel, every turn
  checked → merged → the next audit shows the drop.
- **Rulebook change.** Override written in the root checkout → lock updated → every worktree
  picks it up on its next hook call.
- **Doc edited.** Changed paragraphs re-extracted in the background → index (and, in the root
  checkout, lock) rebuilt → overrides re-anchored → drift re-run. Turns already running keep
  their index.

## 7. Budgets

| Path                         | Target                                               | Past the limit                |
| ---------------------------- | ---------------------------------------------------- | ----------------------------- |
| Edit check                   | under 20 ms typical                                  | 50 ms: allow, mark unchecked  |
| Session-start context        | under 100 ms                                         | 5 s timeout: no context       |
| Prompt-submit snapshot       | under 300 ms on a repo this size (measured in S4)    | 10 s timeout: turn unchecked  |
| Turn check (auto send-back)  | seconds, dominated by the judge                      | 120 s timeout: turn unchecked |
| Drift                        | seconds on a repo this size                          | background; resumes           |
| Audit (code checks)          | under a minute on a repo this size, files in parallel | background; resumes          |
| Extraction, backtest, judged audit | background                                     | resumes on the next run       |

## 8. Failure modes

| Situation                                    | Behavior                                         | Shown as                          |
| -------------------------------------------- | ------------------------------------------------ | --------------------------------- |
| Binary missing or older than the lock format | hooks do nothing                                 | "pragma needs installing/update"  |
| Edit check errors or passes 50 ms            | edit allowed                                     | unchecked edits in the report     |
| Model CLI missing or logged out              | pack, lock and drift work; extraction, judging pause | banner in Rulebook; badge     |
| Judge times out or returns invalid output    | turn ends normally                               | badge: unchecked (judged)         |
| Diff over the size cap                       | code checks run, judge skipped                   | badge: unchecked (diff too big)   |
| A paragraph fails extraction                 | skipped, retried next run                        | listed in the Rulebook            |
| Drift or audit interrupted                   | resumes on the next run                          | Repo health shows its last run    |
| Cache or index corrupt                       | rebuilt from docs                                | nothing                           |
| A hook is killed by its timeout              | Claude Code carries on                           | badge: unchecked                  |

## 9. Security

- Model calls run with `PRAGMA_JUDGE=1`, no tools, and isolated settings where the CLI allows
  (flags confirmed in S0c). Outputs are schema-validated; anything else is discarded.
- Repo text in prompts is delimited as data. Text sent to agents, including burn-down task
  prompts, is framed as review feedback quoting the doc.
- The binary reads hook input from stdin and never builds shell strings from paths or doc text.
  Paths are normalized before matching (5.1).
- Hook installation touches only entries with pragma's marker.

## 10. Distribution and development

- **Hosts.** The app bundles per-platform binaries (darwin-arm64, darwin-x64, linux-x64,
  linux-arm64, win-x64), signed and notarized with the app, and on launch refreshes
  `$PRAGMA_HOME/bin/pragma`. Terminal users can put that on their PATH. For SSH hosts the binary
  ships inside the workspace server's per-platform archive
  (`apps/workspace-server/docs/packaging.md`) and is installed to the host's `$PRAGMA_HOME/bin`.
  Packaging changes touch `build/`, a high-risk area, and are called out in that PR.
- **Versions.** The lock carries a format version. The binary refuses a newer format and says
  so; the app checks the host binary's version before relying on it.
- **Dev runs.** `pnpm run dev` builds the binary through an Nx dependency and sets
  `PRAGMA_HOME` to a dev directory, also passed into the sessions it launches, so dev runs never
  touch an installed pragma.
- **Toolchain.** Rust becomes a prerequisite for building the app, which changes the repo's
  "pnpm is the only prerequisite" promise. `rust-toolchain.toml` pins the version and lists its
  components, so cargo installs the toolchain on first use, locally and in CI. `pnpm run doctor`
  checks for rustup, and AGENTS.md is updated in S1.
- **Nx.** The crates are one Nx project, `crates/project.json`, with `nx:run-commands` targets
  like the app's: `build`, `test`, `lint` (clippy), `format` and `format:check` (rustfmt), so
  `pnpm run check` and CI's `nx affected` cover Rust. A `bench` target runs the latency
  benchmark in release mode, outside the default gate.
- **Release builds.** Per platform in the release workflows: macOS arm64 and x64, Linux x64 and
  arm64 (glibc, the workspace server's targets), and Windows x64.
- **Dependencies.** `ast-grep-core`, tree-sitter TS/JS grammars, `globset` and `serde`, with
  licenses checked and `Cargo.lock` committed. The grammars compile C during `cargo build`; per
  AGENTS.md that native build is flagged for review.

## 11. Quality

The product rests on trust, so quality is measured, not assumed.

- A golden set in `crates/pragma-evals/`: real docs, real diffs, expected verdicts. Seeded from
  this repo's `AGENTS.md` in spike S0a, and grown from findings in `feedback.jsonl` the user
  chooses to add.
- The first drift fixture is this repo's docs as they are today: a first pass found 42 of the 452
  file paths they name missing, across 16 docs.
- `pragma eval` reports extraction precision, judge precision and recall, and drift precision.
- Auto send-back is the default only once the judge reaches at least 0.8 precision on the golden
  set; until then the default is manual.
- In use, "the check is wrong" exceptions per clause are the live precision signal, shown in the
  Rulebook.
- Engine tests cover each library rule, the new-violations-only logic, path normalization,
  anchoring, exceptions, and lock determinism, plus a latency benchmark that fails above 20 ms
  p95 on a fixture repo. CLI tests run against temporary git repos. App UI gets browser tests.

## 12. Known limitations

- Edits made through Bash are caught at turn end, not blocked mid-edit.
- The user's own edits during a turn count as the agent's.
- Two sessions in one worktree share one diff; pragma assumes one agent per worktree, which is
  the app's default.
- Judged and context clauses can differ slightly between machines (model output); blocking
  clauses cannot (the lock).
- Docs changed only in a task branch add no blocking rules until they reach the root checkout.
- Drift's check of code names is a text search, so it can miss renamed symbols or match the
  wrong one.
- Chat (ACP) sessions depend on what spike S0b finds.

## 13. Build order

Each slice works end to end.

- **S0 spikes (throwaway, in parallel):** (a) extraction and judge quality on this repo's
  `AGENTS.md`, seeding the golden set and picking the default model; (b) how to intercept edits
  in chat (ACP) sessions, including bypass-permissions mode; (c) Claude Code hooks: file-edit
  tool names, added context per event, Stop blocking and how it interacts with the app's own Stop
  status hook, and the flags that isolate `claude -p`.
- **S1 engine:** Rust toolchain, Nx and CI targets, doctor check and AGENTS.md update; crates;
  lock format; the four library rules; new-violations-only logic; path normalization; block
  message; `$PRAGMA_HOME` and the stable binary path; hook install and uninstall with the gate;
  PreToolUse blocking from a hand-written lock; `pragma init`, `lock check`, `debug anchor` and
  `version`; latency benchmark.
- **S2 audit and drift, from the terminal:** `pragma drift` with paths, scripts, code names and
  moved-file fixes; `pragma audit` over the whole tree with baseline, history and exceptions;
  `pragma exception add`.
  Until S3 the audit runs a hand-written lock, so on this repo it is useful on day one.
- **S3 extraction:** discovery, paragraphs, cache, clauses, validation, scope, precedence, the
  index, the lock update rule, root-checkout writes, extraction triggers.
- **S4 turn checks:** baseline and unfinished turns, Stop, code and judged checks with output
  validation, tamper and drift flags, reports, auto send-back with caps, context hand-off with
  budget, live events to the app.
- **S5 app, first open and existing code:** enable flow, Repo health, the drift view with doc
  fixes, burn-down with task creation, "Not a violation" with exceptions, review and commit,
  repo and host settings, pause, bundling the binary into the packaged app.
- **S6 app, the agent loop:** agent status handling, badge, inline violations, turn reports,
  escalation, manual send-back, "Allow for this task", notifications, the context view.
- **S7 Rulebook:** overrides, anchoring, rewording suggestions, contradictions, stale overrides
  and exceptions, uncommitted changes.
- **S8 backtest and pack:** backtest, promotion, history in Repo health, Decisions view markers,
  the pack.
- **S9 SSH hosts:** binary in the workspace server archive, version checks.
- **S10 rename** to pragma.

Chat (ACP) support lands in whichever slice S0b points to.

## 14. Decisions this spec defaults (confirm or overturn)

1. Default rung: judged only for norms checkable from a diff; otherwise context.
2. A committed lock holds the blocking clauses (an exception to "every `.pragma/` file reads as
   a doc").
3. Blocking fires only on violations the edit introduces.
4. Checks are built on `ast-grep-core`; TS/JS first.
5. CLI only in v1; no napi module. **Changes the brief**, which said the app loads the engine
   through napi-rs.
6. All host-side logic, including extraction and judging, lives in the Rust binary; the app is
   UI and launcher.
7. pragma enforces in any Claude session in a repo with `.pragma/`, not only app-started ones.
8. Turn reports and findings are files in the git dir; no database migration in v1.
9. Send-back is automatic by default, capped at 2 per prompt, once the judge meets its precision
   bar.
10. SSH hosts are in v1 as S9, and are the first thing to cut if v1 runs long.
11. One writer per repo: pragma writes `.pragma/` only in the root checkout, and every worktree
    reads that working copy.
12. The lock is the only source of blocking clauses; a branch's doc changes add blocking rules
    only after they reach the root checkout.
13. Context hand-off skips docs the agent already loads by itself and stays within a size budget.
14. The user's escapes are "Allow for this task", "Not a violation" and pause; pragma adds no
    telemetry.
15. v1 gives value on existing code: drift and audit from the terminal right after the engine
    (S2), and Repo health with burn-down as the first app slice (S5).
16. "Not a violation" records an exception in `.pragma/overrides.md`, so a finding stays
    suppressed everywhere, and says whether the exception is deliberate or the check is wrong.

## 15. Not in v1

Agent scorecards, learning rules from corrections, a CI merge gate, team features, per-feature
requirement specs, blocking mid-edit for non-Claude agents (turn checks may still run for agents
that report turn end), languages beyond TS/JS for code checks, notebook edits, plain directories
without git, installed agent skills as spec sources, and model-judged audits of a whole repo at
once (only of a chosen area).

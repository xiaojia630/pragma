# pragma

pragma makes every coding agent follow your engineering specs: the conventions, decisions and
culture a team writes down in markdown, plus the rules code can check. It is one Rust binary,
`pragma`, a CLI.

## What v1 ships (locked)

First user: a solo developer running coding agents who cares about how good systems are built.

Since the app was removed (2026-10-16), the engine parts below are built in the CLI: reading
docs into rules, the lock, blocking mid-edit, drift, audit, backtest, and the block records.
The parts that assumed an app (the Rulebook view, the task badge and inline diff violations,
one-click send-back, the Decisions view, the repo health report and burn-down tasks) need a
new home in a CLI-first product: terminal output, a static report, a TUI or a local UI. That
is to be planned.

1. Your docs from minute one: pragma reads the markdown a repo already has (AGENTS.md,
   CLAUDE.md, CONTRIBUTING, ADRs, design docs, glossaries, Cursor rules) where it lives, and
   splits each doc into the specific things it asks for, linked back to its paragraph. A
   built-in "good engineering" pack sits underneath. A Rulebook view lets you browse them,
   switch them off and change their level.
2. Every agent turn checked: a badge on the task, and violations inline on the exact diff line,
   citing the paragraph they break and how to fix it.
3. Bad edits stopped mid-task: parts that code can check block the edit and hand the agent the
   fix (Claude Code hooks first).
4. Send back to fix: one click, or automatic, sends the agent a fix request.
5. Decisions view (it lived in the removed app) plus a backtest of code-checked rules over
   recent history on first open (built: `pragma backtest`).
6. Value on the code you already have: a repo health report on first open (docs that point at
   code that's gone, code that already breaks the docs, and history), and burn-down tasks that
   send agents to fix existing violations. The count can only go down.

Underneath: a Rust rules engine.
Not in v1: agent scorecards, learning rules from corrections, CI merge gate, team features,
per-feature requirement specs.

## Decided

- Specs, not just rules: the source of truth is the team's own markdown, read where it lives.
  Each clause is code-checkable (can block mid-edit), judged (checked after each turn) or
  context-only (handed to the agent). An agent gets only the clauses for the area it works in.
- Docs stay written for humans: pragma never requires syntax that hurts readability. Clauses are
  derived from the docs; corrections are overrides in `.pragma/`, anchored to their paragraph.
  pragma suggests doc rewording as a diff you accept, and never edits docs on its own.
- Rules engine in Rust: one `pragma` binary holds all host-side logic (agent hooks, extraction,
  judging, backtest, CLI). Every command has `--json` for tools and future UIs. Prebuilt per
  platform, like oxlint.
- Blocking clauses come only from a committed lock (`.pragma/pragma.lock`) and block only
  violations an edit introduces. Library checks block at once; generated checks earn blocking
  through the backtest.
- Anything an agent waits on (the lookup for a path, the block check on an edit) takes under
  20 ms, with a 50 ms hard cap. Slower work (judged clauses, model calls) runs beside the agent,
  is cached by content hash, and fails open.
- Judged clauses run on the agent's own model (for example `claude -p`), never on our API keys.
- Code-checkable clauses block mid-edit. Judged clauses never block: they run after each turn
  and send a fix request. The merge gate is the only hard stop.
- `.pragma/` is an open spec, in Markdown with optional frontmatter.

The full v1 architecture is `docs/superpowers/specs/2026-10-15-pragma-v1-architecture-design.md`,
and the v1 screen plan is `docs/superpowers/specs/2026-10-15-pragma-v1-screens.md`. Both were
written for the app; their engine sections still hold.

## Working here

- Push only to xiaojia630/pragma.
- Commits carry no `Co-Authored-By` lines: the author is the person who asked for the work.

@AGENTS.md

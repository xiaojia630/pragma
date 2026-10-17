# pragma commands

This page lists every `pragma` command with its flags, what it writes and where. It matches
`pragma --help` and `pragma <command> --help` in version 0.1.0. For the ideas behind the commands,
see [how it works](how-it-works.md). For install and a first run, see the [main page](README.md).

## What every command shares

- **Where it runs.** Run it in a git repo, in any folder of it. pragma finds the repo by walking
  up from the current folder to the first `.git`. Outside a repo, the commands that need one stop
  with `pragma: not inside a git repository`. `version`, `hooks` and `help` work anywhere.
- **`--json`.** Every command takes `--json` except `pragma hook pre-tool-use`. It prints one JSON
  document instead of text, for scripts. `pragma debug anchor` always prints JSON. The fields can
  change while pragma is a preview. For `lock` and `drift`, which have subcommands, put the flag
  after the subcommand: `pragma lock check --json`.
- **Exit codes.** 0 for success. 1 for any error, including a mistake in the command line; the
  message goes to stderr. 2 only when `pragma hook pre-tool-use` blocks an edit.
- **Reports are not gates.** `drift`, `audit` and `backtest` exit 0 even when they find problems.
- **`.pragma/` is written in the root checkout only.** The root checkout is the repo's main
  working tree. See [worktrees](how-it-works.md#worktrees).
- **Help.** `pragma --help` lists the commands. `pragma help <command>` is the same as
  `pragma <command> --help`. `pragma -V` prints the version.

## Environment variables

- `PRAGMA_HOME`: pragma's folder on your machine. Default `~/.pragma`. It holds `bin/pragma` and
  `cache/`. The hook looks for the binary at `$PRAGMA_HOME/bin/pragma`.
- `CLAUDE_CONFIG_DIR`: where Claude Code keeps its `settings.json`. Default `~/.claude`.
- `PRAGMA_DISABLE`: set to any non-empty value, it makes the hook do nothing.
- `PRAGMA_JUDGE`: pragma sets this on its own `claude` calls so its hook stays silent in them.

An empty value counts as unset.

## All commands

| Command                    | What it does                                              |
| -------------------------- | --------------------------------------------------------- |
| `pragma init`              | Create `.pragma/` in the root checkout.                   |
| `pragma status`            | Show what pragma is doing in this repo.                   |
| `pragma hooks install`     | Add pragma's hook to Claude Code's settings.              |
| `pragma hooks uninstall`   | Remove pragma's hook from Claude Code's settings.         |
| `pragma hooks status`      | Show whether the hook is installed.                       |
| `pragma pause`             | Stop pragma's hook for this repo on this machine.         |
| `pragma resume`            | Start it again.                                           |
| `pragma config show`       | Print the repo settings.                                  |
| `pragma config set`        | Change the doc include and exclude lists.                 |
| `pragma docs`              | List the docs pragma reads.                               |
| `pragma extract`           | Read the docs into rules with your `claude` CLI.          |
| `pragma lock`              | Copy the rules that block into `.pragma/pragma.lock`.     |
| `pragma lock show`         | List the rules in the lock.                               |
| `pragma lock check`        | Validate the lock.                                        |
| `pragma commit`            | Commit `.pragma/` and nothing else.                       |
| `pragma drift`             | Find docs that name files, scripts or code that are gone. |
| `pragma drift apply`       | Write the suggested fixes into the docs.                  |
| `pragma audit`             | Check the whole tree against the lock's rules.            |
| `pragma backtest`          | Count the violations recent commits added.                |
| `pragma exception add`     | Mark a finding as not a violation.                        |
| `pragma events`            | List what the edit check decided.                         |
| `pragma hook pre-tool-use` | The hook Claude Code calls. Not for direct use.           |
| `pragma debug anchor`      | Print the id and anchor of a doc paragraph.               |
| `pragma version`           | Print the version and the lock format.                    |

## Set up

### pragma init

```sh
pragma init [--json]
```

Creates `.pragma/` in the root checkout. This is what turns pragma on for a repo: the hook acts
only where a `.pragma/` folder exists. It creates these files and skips any that already exist, so
running it again is safe:

- `README.md`: a short note for people who find the folder.
- `config.md`: the repo settings, with empty doc include and exclude lists.
- `pragma.lock`: an empty lock.

Run it in a linked worktree and it still creates the folder in the root checkout.

Writes: those three files, and their hashes in `<git common dir>/pragma/writes.json`. Commit
`.pragma/` afterwards with [`pragma commit`](#pragma-commit).

### pragma status

```sh
pragma status [--json]
```

Shows what pragma is doing in this repo:

- whether `.pragma/` exists, and whether you are in a linked worktree;
- how many rules the lock holds, or why it cannot be read;
- the state of reading the docs: not yet read, running (paragraphs done of total), done (rules by
  level), or paused with the reason;
- whether the lock is behind the docs (run `pragma lock`);
- whether pragma is paused;
- whether the Claude Code hook is installed.

Writes nothing.

### pragma hooks install

```sh
pragma hooks install [--settings <SETTINGS>] [--json]
```

Adds pragma's hook to Claude Code's settings file, beside your own hooks.

- `--settings <SETTINGS>`: use this settings file. Default: `$CLAUDE_CONFIG_DIR/settings.json`,
  else `~/.claude/settings.json`.

It adds one `PreToolUse` hook for the tools `Edit|Write|MultiEdit`, with a 1 second timeout. The
hook's command starts with `PRAGMA_HOOK=1;`, which marks it as pragma's. It runs
`$PRAGMA_HOME/bin/pragma hook pre-tool-use` (`~/.pragma/bin/pragma` by default) and does nothing
if that file is missing or not executable.

- Installing twice leaves one entry.
- Everything else in the file stays as it was, in the same order.
- It creates the file and its folder if they do not exist.
- It refuses to overwrite a file that is not valid JSON, and exits 1.
- It follows a symlinked settings file and keeps the file's permissions.
- On Windows it stops with an error: the hook does not install there yet.

Writes: the settings file.

### pragma hooks uninstall

```sh
pragma hooks uninstall [--settings <SETTINGS>] [--json]
```

Removes the entries marked `PRAGMA_HOOK=1;`, and any group, event list or `hooks` object that
removing them left empty. Your other settings and hooks are not touched. If the settings file does
not exist, it says so and exits 0.

Writes: the settings file.

### pragma hooks status

```sh
pragma hooks status [--settings <SETTINGS>] [--json]
```

Says whether pragma's hook is in the settings file. With `--json` it also lists the hook events
pragma installs. Writes nothing.

### pragma pause

```sh
pragma pause [--json]
```

Stops pragma's hook for this repo on this machine, in every worktree of the repo. It creates the
file `<git common dir>/pragma/paused`. The file lives inside `.git`, so it is not committed and
does not affect anyone else. Commands you run yourself, like `audit`, still work.

### pragma resume

```sh
pragma resume [--json]
```

Removes the `paused` file. The hook acts again. Running it when nothing is paused is fine.

### pragma config show

```sh
pragma config show [--json]
```

Prints the repo settings from `.pragma/config.md`: `docs.include` and `docs.exclude`, each a list
of globs or `none`. Writes nothing.

### pragma config set

```sh
pragma config set [--docs-include <GLOB>]... [--docs-exclude <GLOB>]...
                  [--clear-include] [--clear-exclude] [--json]
```

Changes the doc lists. Only the keys you pass are rewritten.

- `--docs-include <GLOB>`: replace `docs.include` with these globs. Repeat the flag for more.
- `--docs-exclude <GLOB>`: replace `docs.exclude` with these globs. Repeat the flag for more.
- `--clear-include`: empty `docs.include`.
- `--clear-exclude`: empty `docs.exclude`.

The flags replace the list; they do not add to it. To keep an existing entry, pass it again.
`--docs-include` cannot be combined with `--clear-include`, nor `--docs-exclude` with
`--clear-exclude`. With none of the four flags the command stops with an error. Globs are checked
before anything is written. They match repo-relative paths: `*` stays within one folder and `**`
crosses folders.

Run it in the root checkout, in a repo that has `.pragma/`. Writes: the front matter of
`.pragma/config.md`. The text below the front matter, and any other keys, are kept. It prints the
new settings.

## Read your docs

### pragma docs

```sh
pragma docs [--include <GLOB>]... [--exclude <GLOB>]... [--preview] [--json]
```

Lists the docs pragma reads, with a paragraph count for each. With no flags it uses the lists
saved in `.pragma/config.md`.

- `--include <GLOB>`: also read docs matching this glob. Repeatable.
- `--exclude <GLOB>`: leave out docs matching this glob. Repeatable.
- `--preview`: use only the globs given on this command line, even none, in place of the saved
  ones.

Globs given here replace the saved ones for this run, so you can try a list before you save it
with `pragma config set`. The text output shows the ten docs with the most paragraphs, marks ADRs
and docs whose status says superseded, deprecated or rejected as `(historical)`, and counts the
docs your exclude globs left out. `--json` lists every doc. Works in any git repo, with or without
`.pragma/`. Writes nothing.

### pragma extract

```sh
pragma extract [--estimate] [--json]
```

Reads the spec docs into rules with your `claude` CLI, and writes this checkout's index.

- `--estimate`: count the docs, paragraphs and model calls a run needs, without calling the model.

A run:

1. finds the docs and splits them into paragraphs, skipping historical docs;
2. looks each distinct paragraph up in the cache at `$PRAGMA_HOME/cache/extract/`;
3. sends the rest to `claude -p`, 10 paragraphs per call, up to 4 calls at once, each call limited
   to 120 seconds;
4. validates what comes back and writes the index.

Each answer is cached the moment it arrives, so a run you stop or that fails resumes where it
left off. `Ctrl-C` stops the calls in flight. A paragraph whose call fails is listed as failed in
the index and tried again on the next run.

If `claude` is missing or logged out, the run stops with exit 1 and `pragma status` says why.
Only one run goes at a time per repo; a second one stops with
`extraction is already running in this repo`.

`extract` does not change `.pragma/pragma.lock`. Run [`pragma lock`](#pragma-lock) after it.

Writes:

- `$PRAGMA_HOME/cache/extract/`: one file per paragraph answered. Entries unused for 30 days are
  removed at the start of the next run.
- `<git dir>/pragma/index.json`: this checkout's index, written when the run finishes.
- `<git dir>/pragma/extract-progress.json`: progress and how the run ended.
- `<git common dir>/pragma/extract.lock` and `extract.guard`: held while a run is going.

`--estimate` writes nothing and calls no model. Works in any git repo, with or without `.pragma/`.

### pragma lock

```sh
pragma lock [--json]
```

Updates `.pragma/pragma.lock` from this checkout's index. This is the only command that puts
extracted rules into the lock. It needs the root checkout, a `.pragma/` folder, and an index from
`pragma extract`.

- For a paragraph that is unchanged, the lock keeps its entries byte for byte.
- A new or reworded paragraph brings in its blocking rules; a deleted one drops its rules.
- Hand-written entries are never touched.
- Only docs that git tracks count, so an untracked draft does not reach the lock.
- It refuses to write a lock that would fail `pragma lock check`.

It prints how many rules were added, changed, removed and kept, then asks you to review the diff
and commit it. With `--json` it prints the ids added, changed and removed, the number kept, and
the number of doc paragraphs it recorded as decided. Writes `.pragma/pragma.lock` only when
something changed.

### pragma lock show

```sh
pragma lock show [--json]
```

Lists the rules in the effective lock: id, text, the doc line it came from and the fix, then the
number of exceptions in `.pragma/overrides.md`. `--json` adds each rule's kind and scope. Writes
nothing.

### pragma lock check

```sh
pragma lock check [--json]
```

Validates the lock. It reports duplicate ids, scopes that do not compile, `forbid` globs that do
not compile, and patterns that do not compile, do not match their bad example, or match their good
one. It prints each problem and exits 1 if there are any. Use it after editing the lock by hand.
Writes nothing.

### pragma commit

```sh
pragma commit -m <MESSAGE> [--json]
```

Commits `.pragma/` and nothing else.

- `-m, --message <MESSAGE>`: the commit message. Required.

It stages `.pragma/` and runs `git commit -m <MESSAGE> -- .pragma`, so other changes in your
working tree stay as they are. It uses your git identity and runs your git hooks. If `.pragma/` has
no changes it says so and commits nothing. Root checkout only. Writes: one git commit.

## Check what you have

### pragma drift

```sh
pragma drift [--diff] [--json]
```

Checks the docs against the repo, with no model. It looks at inline code spans, inline links and
`@path` imports in each doc, and skips fenced code blocks and historical docs. It reports:

- a file or folder that does not exist, resolved from the doc's folder, the repo root and each
  package root (a folder with a `package.json`, `Cargo.toml`, `go.mod`, `pyproject.toml`,
  `composer.json`, `Gemfile`, `pom.xml`, `build.gradle`, `build.gradle.kts`, `deno.json` or
  `pubspec.yaml`). A name written without its extension counts as present when a file has that
  name plus an extension, so `design/Foo` finds `design/Foo.dc.html`. A trailing `/` says the doc
  means a folder, so it does not;
- a script command like `npm run <script>`, or the same form with another JavaScript package
  manager, that names a script no `package.json` defines, and `make <target>` for a target no
  Makefile defines;
- a path that starts with `/` and matches no file, like `/sw.js`. It may be a URL of a deployed
  site, not a file, so it is listed apart as less certain and left out of the count of missing
  files. In `--json` its `kind` is `maybe-url`;
- a code name in a code span, like `loadSettings`, that appears nowhere in the repo's code and
  config. These are listed apart as less certain.

When a missing file moved, the finding carries the new path. Each finding has an id.

- `--diff`: print the suggested fixes as a diff for `git apply`, instead of the report:
  `pragma drift --diff | git apply`.

Findings you marked as intended with [`pragma exception add`](#pragma-exception-add) are left out.
Works in any git repo, with or without `.pragma/`. Writes `<git dir>/pragma/drift.json`, the
latest result. It does not change your docs.

### pragma drift apply

```sh
pragma drift apply <IDS>... [--json]
```

Writes the suggested fix of each finding into its doc. Give it one or more finding ids from
`pragma drift`. It checks the docs again first, and applies a fix only if the finding is still
there and the text at that spot has not changed. It prints how many fixes it applied and which ids
it skipped (not found, no suggested fix, or the doc changed). This is the one place where pragma
edits your docs, and only when you ask.

Writes: the docs it fixes, and `<git dir>/pragma/drift.json`.

### pragma audit

```sh
pragma audit [--area <AREA>] [--changed-from <REF>] [--json]
```

Checks the whole tree against the lock's rules, not only a diff. Needs `.pragma/`. The result is
the repo's baseline of existing violations.

- `--area <AREA>`: only this file or folder, repo-relative.
- `--changed-from <REF>`: only the files changed since the merge base of `<REF>` and `HEAD`,
  committed or not, plus untracked files. Cannot be combined with `--area`.

It reads files that are tracked, or untracked and not ignored, inside a rule's scope. It skips
files marked `linguist-generated` in the root `.gitattributes`, and `node_modules`, `vendor` and
`third_party`. Files over 512 KB get path rules only. Findings you recorded as exceptions are
left out.

It prints each finding with an id, file, line and the matched code. Use the id with
[`pragma exception add`](#pragma-exception-add).

Writes, for a full audit only: `<git dir>/pragma/audit.json` and a new line in
`<git dir>/pragma/audit-history.jsonl` with the counts per rule. A run with `--area` or
`--changed-from` writes nothing and says `Not saved`.

### pragma backtest

```sh
pragma backtest [--since <N>] [--json]
```

Counts the violations recent commits introduced that today's lock would have stopped. Needs
`.pragma/`.

- `--since <N>`: how many first-parent commits of `HEAD` to check, at least 1. Default 200.

Each commit is compared with its first parent, so a merge counts its branch once. The repo's first
commit has nothing to compare with and is skipped. Only added or changed files count; deletions do
not. Rules that guard paths (protected paths) apply to edits only, so history never runs them. The
output says how many rules were checked. In a partial clone, a commit whose files are not local
cannot be fully checked: its hits are left out of the counts, the output says how many commits that
was ("2 commits could not be fully checked: their files are not in this clone."), the JSON carries
it as `incomplete_commits`, and those commits are checked again on the next run. With git 2.44 or
later pragma never fetches missing files; older git ignores that setting and may fetch them.

Results are cached per commit, so a second run checks only new commits. A change to the lock's
checks starts the cache over.

Writes: `<git dir>/pragma/backtest.json` (the latest report) and
`<git common dir>/pragma/backtest-cache.json` (the per-commit cache).

### pragma exception add

```sh
pragma exception add <ID> --reason <REASON> [--note <NOTE>] [--json]
```

Marks a finding as not a violation. `<ID>` comes from the last full `pragma audit` or from
`pragma drift` in this checkout.

- `--reason <REASON>`: required. One of:
  - `deliberate`: the rule is right, and this code is sanctioned anyway.
  - `check-is-wrong`: the check fired on code that does not break the rule.
  - `intended`: a drift finding that is meant, like an example path.
- `--note <NOTE>`: a free-text note stored with the exception.

Use `deliberate` or `check-is-wrong` for an audit finding, and `intended` for a drift finding. An
id that is in neither result stops with an error. The exception is keyed to the rule's paragraph,
the file and the matched code (audit), or to the doc's paragraph and the reference (drift). It
hides that finding from `audit`, `drift` and `backtest`. It does not change what the edit check
blocks. Commit it with `pragma commit` so the exception holds on every machine.

Writes: an entry under `exceptions:` in the front matter of `.pragma/overrides.md` in the root
checkout (the file is created if it does not exist, and the text below the front matter is yours),
a line in `<git common dir>/pragma/feedback.jsonl`, and a hash in `writes.json`.

## See what the edit check did

### pragma events

```sh
pragma events [--limit <N>] [--since-ms <MS>] [--json]
```

Lists what the edit check decided in this checkout, oldest first.

- `--limit <N>`: show at most this many, the most recent. Default 200.
- `--since-ms <MS>`: only events after this time, in milliseconds since the Unix epoch.

Each line is `time  kind  file:line  what`, where `what` is the rule's text or, for an unchecked
edit, the reason. The kinds are:

- `blocked`: the edit broke a rule and was stopped.
- `escalated`: the same rule had blocked the same file three times in one session, so this edit
  went through.
- `unchecked`: the edit went through without a full check, with the reason (a file over 512 KB, a
  lock that could not be read, or the 50 ms budget).
- `cleared`: the session's next edit to a file no longer broke the rule that had blocked it.

The command reads only the end of the log, the last 256 KB or about a thousand events, however
long the log grows. A repo without `.pragma/` gives an empty list. In a linked worktree you see
that worktree's events. Writes nothing.

### pragma hook pre-tool-use

```sh
pragma hook pre-tool-use
```

Claude Code's `PreToolUse` hook. Claude Code runs it, passing the tool call as JSON on stdin. You
do not run it yourself. It checks `Edit`, `Write` and `MultiEdit` calls against the lock:

- Exit 2, with the block message on stderr, when the edit adds a violation of a blocking rule.
  Claude Code stops the edit and shows the message to the agent.
- Exit 0 in every other case: a clean edit, a repo without `.pragma/`, a paused repo, a
  `PRAGMA_DISABLE` or `PRAGMA_JUDGE` environment, input it cannot read, an unreadable lock, a
  check past the 50 ms cap, or any internal error. The hook never stops an edit by accident.

Writes: lines in `<git dir>/pragma/events.jsonl` and the session's counts in
`<git dir>/pragma/blocks/`. It never writes `.pragma/`. See
[the edit check](how-it-works.md#the-edit-check).

## Other

### pragma debug anchor

```sh
pragma debug anchor <LOCATION> [--ordinal <ORDINAL>] [--json]
```

Prints, as JSON, the rule id and the anchor of the paragraph at `<LOCATION>`, written
`<doc>:<line>` (for example `AGENTS.md:5`). The anchor holds the doc path, heading path,
paragraph hash, line range and a quote. Use it when you write a rule into the lock by hand.

- `--ordinal <ORDINAL>`: which rule of that paragraph the id is for. Default 0.
- `--json`: accepted for consistency; the output is always JSON.

It exits 1 if there is no paragraph at that line. Writes nothing.

### pragma version

```sh
pragma version [--json]
```

Prints the version and the lock format the binary reads, for example
`pragma 0.1.0 (lock format 1)`. A binary refuses a lock with a newer format and tells you to
update. Writes nothing.

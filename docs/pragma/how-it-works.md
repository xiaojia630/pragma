# How pragma works

This page explains the ideas behind the commands: which docs pragma reads, how they become rules,
how a rule ends up stopping an edit, and where everything is stored. It describes version 0.1.0.
Things that are planned but not built are marked "not yet". For commands and flags, see the
[command reference](commands.md). For install, see the [main page](README.md).

## The path from a doc to a stopped edit

1. pragma finds your spec docs and splits them into paragraphs.
2. `pragma extract` asks your `claude` CLI what each new paragraph requires. The answers are the
   rules. They are cached.
3. The rules go into an index in your `.git` folder.
4. `pragma lock` copies the rules that code can check into `.pragma/pragma.lock`. You commit it.
5. While Claude Code works, a hook checks each edit against the lock and stops an edit that adds a
   violation.

Your docs stay as they are. pragma never asks you to write a doc differently, and it changes a doc
only when you run `pragma drift apply`.

## Which docs are read

pragma reads these files:

- `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `CONTRIBUTING.md` and `README.md`, in any folder;
- `.github/copilot-instructions.md`, `.cursorrules` and `.cursor/rules/**/*.mdc`;
- `CONTEXT.md` and `GLOSSARY.md` in the repo root;
- every `.md` file under the `docs/` folder in the repo root;
- every `.md` file under `.pragma/specs/`, for specs you write only for pragma;
- any markdown file these docs link to, or import with Claude Code's `@path`, inside the repo.

It leaves out files that `.gitignore` ignores, `node_modules`, `vendor`, `third_party`,
`CHANGELOG*`, `LICENSE*`, and `.pragma/` itself apart from `.pragma/specs/`. You can add or remove
docs with the include and exclude globs in `.pragma/config.md`; set them with
`pragma config set`, and try a list with `pragma docs` first.

**Historical docs give no rules.** A doc counts as historical when it sits in an `adr`, `adrs` or
`decisions` folder below `docs/`, or when its front matter or a status line near the top says
superseded, deprecated, rejected or historical. `pragma docs` marks them. They describe the past,
so pragma reads no rules from them and `pragma drift` does not check them.

**Where a rule applies.** A rule has a scope, a list of globs. An empty scope means the whole
repo.

- A nested `AGENTS.md` (or `CLAUDE.md`, `README.md` and the other known names) applies to its own
  folder. One directly in `.claude/`, `.github/` or `docs/` applies to the whole repo.
- Paths a paragraph names become the rule's scope.
- Front matter wins. For a `.mdc` file, `globs` sets the scope. For a file in `.pragma/specs/`,
  `scope` sets the scope and `rung` sets the level. A rule still blocks only if it has a check
  that code can run.

## Paragraphs

A paragraph is the unit pragma reads and links back to. It is one of:

- a prose paragraph;
- a top-level list item, with its sub-items;
- a table.

A code block belongs to the paragraph before it. A paragraph's identity is a hash of its text with
whitespace folded together. Reflowing a paragraph does not change the hash. Rewording it does, and
then pragma reads it again. Identical paragraphs in several docs are read once.

Every rule keeps an anchor to its paragraph: the doc path, the headings above it, the hash, the
line range and the first 120 characters. That is how a block message can say
`AGENTS.md:6, "Conventions"`.

## Extraction and its cache

`pragma extract` sends the paragraphs it has not read before to `claude -p`: 10 per call, up to 4
calls at once. For each paragraph the model proposes its rules. A proposed rule has:

- **text**: the instruction, in one sentence;
- **level**: blocking, judged or context (see below);
- **scope**: the globs it applies to;
- **check**: for a rule code can check, one of four kinds (see
  [writing a rule by hand](#writing-a-rule-by-hand));
- **fix**: how to fix a violation.

pragma then checks what came back and sets the final level. A scope glob must compile. A pattern
must compile, match its bad example and not its good example. A check that fails is dropped, and
its rule becomes judged. A blocking rule whose scope matches no file in the repo would block
nothing, so it becomes judged too. A protected-paths or file-naming rule needs a scope narrower
than the whole repo.

The model is `haiku` unless `.pragma/config.md` has a `model:` line in its front matter.

**The cache** is `$PRAGMA_HOME/cache/extract/` (`~/.pragma/cache/extract/`). One file holds the
answer for one paragraph, keyed by the paragraph's hash, the extractor version and the model.
Every repo and worktree on your machine shares it. Entries unused for 30 days are removed. You can
delete the folder at any time; the next run asks again. A changed paragraph has a new hash, so
editing a doc costs model calls only for the paragraphs you changed.

pragma does not read your docs by itself. Run `pragma extract` after you change them, then
`pragma lock`.

Model answers vary from run to run. That is why the edit check never reads the index: it reads
the lock, which does not vary.

## The three levels

Every rule sits on one level. The level says when the rule is enforced.

- **blocking**: code checks each edit. The rule needs a check and an entry in the lock. This works
  in version 0.1.0, through the Claude Code hook.
- **judged**: a reviewer could check it from a diff. It never blocks. Not yet checked: pragma keeps
  these rules in the index.
- **context**: background the agent should know. It is never checked. Not yet handed to the agent:
  pragma keeps these rules in the index.

Elsewhere this level is called a rung; the front matter of `.pragma/specs` files uses that word.

Extraction makes a rule blocking only when code can check it exactly:

- **protected paths**: no edit under a folder, like generated files or numbered migrations;
- **import boundaries**: this area must not import that module or area;
- **file naming**: case and suffix for the files in a folder.

A code pattern the model writes does not block. It starts as judged, because a wrong pattern would
stop the agent with a wrong fix. Promoting such a pattern after it passes the backtest is not yet
built. Rules you write into the lock by hand can use a pattern and do block; see
[writing a rule by hand](#writing-a-rule-by-hand).

## The index and the lock

**The index** is `<git dir>/pragma/index.json`. It holds every rule from `pragma extract` at every
level, for one checkout. It lives inside `.git`, so it is not committed. `pragma status` counts
its rules by level.

**The lock** is `.pragma/pragma.lock`. It holds the rules that block. It is generated, committed
and authoritative. Review its diffs like you review a lockfile.

Only `pragma lock` puts extracted rules into the lock, and only in the root checkout. The rule it
follows:

- A paragraph whose text is unchanged keeps its lock entries byte for byte, even if a fresh
  extraction would answer differently. A new machine or a cleared cache changes nothing.
- A changed paragraph drops its old rules and brings in the new ones. A deleted paragraph drops
  its rules.
- A doc that could not be read in full keeps its entries. A partial index never loosens the lock.
- A hand-written entry is never touched.
- Only docs that git tracks count, so a draft you have not added does not reach the lock.
- The lock also records which paragraphs it has decided on, including those with no blocking
  rule, so they are not decided twice.

A branch cannot loosen a rule by editing a doc. A doc changed only on a branch gives that branch
judged and context rules, and its blocking rules arrive after the change reaches the root
checkout and you run `pragma lock` there.

Every pragma command that reads the lock refuses one with a newer format than the binary knows,
and says to update pragma.

### Writing a rule by hand

A rule written into the lock blocks exactly like an extracted one, and `pragma lock` leaves it
alone. `pragma lock` marks the rules it writes with `"origin": "extracted"`, and treats a rule
without that mark as hand-written. This is a complete lock with one rule, the same one the demo
uses:

```json
{
  "format": 1,
  "clauses": [
    {
      "id": "c88859b94d9fa5a6",
      "anchor": {
        "path": "AGENTS.md",
        "heading_path": ["Demo", "Conventions"],
        "para_hash": "2136f31bfe0fb493",
        "lines": [6, 6],
        "quote": "- Never call `console.log` in `src/`; log through `log()` from `src/log.ts`."
      },
      "text": "Never call console.log in src/; log through log() from src/log.ts.",
      "scope": ["src/**/*.ts", "!src/log.ts"],
      "check": {
        "kind": "library",
        "rule": "forbidden-pattern",
        "pattern": "console.log($$$ARGS)",
        "examples": { "bad": "console.log('users', count);", "good": "log('users', count);" }
      },
      "fix": "Import { log } from './log' and call log(...) instead of console.log(...)."
    }
  ]
}
```

`pragma debug anchor AGENTS.md:6` prints the `id` and `anchor` for a paragraph. `scope` is a list
of globs over repo-relative paths. `*` stays in one folder and `**` crosses folders. A glob that
starts with `!` excludes. `pragma lock check` validates the file.

The four kinds of `check`, all with `"kind": "library"`:

- `protected-paths`, with no other fields: blocks an edit that creates or changes a file in scope.
- `import-boundary`, with `forbid`, a list of globs: blocks an edit that adds an import of a
  module matching one of them.
- `forbidden-pattern`, with `pattern` and `examples`: blocks an edit that adds code matching the
  pattern.
- `file-naming`, with `case` and an optional `suffix`: blocks an edit that creates a file whose
  name breaks the case or lacks the suffix.

Details:

- `import-boundary` looks at `import ... from`, `export ... from`, `require()` and `import()`. A
  relative import is also matched as the repo path it points to.
- `pattern` is an [ast-grep](https://ast-grep.github.io/) pattern. `$$$ARGS` matches any
  arguments. Text inside strings and comments never matches. `examples.bad` must match and
  `examples.good` must not.
- `case` is `kebab`, `camel`, `pascal` or `snake`. It applies to the file name before its first
  dot. Names that start with a dot are skipped.
- Patterns and imports are checked in TypeScript and JavaScript files (`.ts`, `.tsx`, `.mts`,
  `.cts`, `.js`, `.jsx`, `.mjs`, `.cjs`). Path rules work on any file.

## The edit check

The edit check is the command `pragma hook pre-tool-use`. `pragma hooks install` registers it with
Claude Code as a `PreToolUse` hook for `Edit`, `Write` and `MultiEdit`. Claude Code runs it before
each of those tool calls and passes the call as JSON.

**The gate.** The hook does nothing unless all of these hold:

- the folder Claude Code is working in is inside a git repo that has a `.pragma/` folder (the root
  checkout's, if it has one);
- the repo is not paused (`pragma pause`);
- `PRAGMA_DISABLE` and `PRAGMA_JUDGE` are not set.

So pragma acts in any Claude Code session in an enabled repo, and nowhere else.

**What it checks.** It rebuilds the file as it would be after the edit, from `Edit`'s old and new
strings, `MultiEdit`'s list of edits, or `Write`'s content. If an `Edit`'s old string is not found,
or matches more than once without `replace_all`, the tool would fail anyway, so the hook lets it
pass. Then it runs the lock's rules whose scope covers the file. It resolves the path first,
against the hook's working folder with `..` and symlinks followed, so a rule cannot be dodged by
spelling a path differently. Edits outside the repo are not checked.

**New violations only.** An edit is blocked only for violations it adds. A violation is identified
by its rule plus the matched text and how many times it occurs, never by line number. So moving
code is not a new violation, and neither is editing a file that already breaks a rule, as long as
the edit adds no further match. `pragma audit` is how you find the violations already there.

**The block.** The hook exits with code 2 and writes this to stderr. Claude Code stops the edit
and gives the message to the agent:

```text
pragma blocked this edit.
Rule (AGENTS.md:6, "Conventions"): Never call console.log in src/; log through log() from src/log.ts.
Found: src/report.ts:2  console.log(users.length);
Fix: Import { log } from './log' and call log(...) instead of console.log(...).
If the user explicitly asked for this change, stop and tell them pragma blocked it; they can
pause pragma for this repo with `pragma pause`.
```

**Speed and failing open.** The check targets under 20 ms and is capped at 50 ms. It starts no
other process and uses no network. Past the cap, the edit goes through and the event is recorded
as `unchecked`. Anything else that goes wrong also lets the edit through: input it cannot read,
a lock it cannot load, a file over 512 KB (content rules are skipped for it), an internal error.
The hook never stops an edit by accident. The exit code is 2 only for a real block.

**Three strikes.** If the same rule blocks the same file three times in one Claude Code session,
the fourth edit goes through. It is recorded as `escalated`, the count starts again, and blocking
resumes. This stops a wrong rule from trapping an agent in a loop. An edit that also breaks
another rule that has not reached three blocks is still stopped.

**What it records.** Each decision is a line in `<git dir>/pragma/events.jsonl`: `blocked`,
`escalated`, `unchecked` (with the reason) or `cleared` (the session's next edit to the file no
longer broke the rule). `pragma events` lists them. The hook never writes `.pragma/`.

**What it does not see.** Edits made through a shell command, like `sed -i` or a redirect in the
Bash tool, are not checked. Notebook edits are not checked. Other agents than Claude Code are not
checked.

## The files and where they live

### In your repo

The folder `.pragma/` in the root checkout. Commit it.

| File            | What it is                                                              |
| --------------- | ----------------------------------------------------------------------- |
| `README.md`     | A note for people who find the folder.                                  |
| `config.md`     | Repo settings in front matter: `docs.include`, `docs.exclude`, `model`. |
| `pragma.lock`   | The rules that block. Generated, plus any you write by hand.            |
| `overrides.md`  | Exceptions you recorded. The text under the front matter is yours.      |
| `specs/**/*.md` | Optional specs written for pragma, in plain markdown.                   |

Deleting `.pragma/` turns pragma off for the repo. Your docs are untouched.

### In `.git`, for the whole repo

`<git common dir>/pragma/` is `.git/pragma/` in a normal clone. Every worktree of the repo shares
it.

| File                            | What it is                                                 |
| ------------------------------- | ---------------------------------------------------------- |
| `paused`                        | Present when you ran `pragma pause`.                       |
| `backtest-cache.json`           | The backtest's findings per commit.                        |
| `feedback.jsonl`                | Findings you marked as not violations, with their details. |
| `writes.json`                   | Hashes of the `.pragma/` files pragma wrote.               |
| `extract.lock`, `extract.guard` | Keep to one extraction run at a time.                      |

### In `.git`, per checkout

`<git dir>/pragma/` is `.git/pragma/` in the main checkout and `.git/worktrees/<name>/pragma/` in
a linked worktree. It goes away with the worktree.

| File                    | What it is                                               |
| ----------------------- | -------------------------------------------------------- |
| `index.json`            | This checkout's rules at every level.                    |
| `extract-progress.json` | The extraction run as it goes, and how it ended.         |
| `drift.json`            | The latest `pragma drift` result.                        |
| `audit.json`            | The latest full `pragma audit` result.                   |
| `audit-history.jsonl`   | The counts per rule from every full audit.               |
| `backtest.json`         | The latest `pragma backtest` report.                     |
| `events.jsonl`          | What the edit check decided.                             |
| `blocks/`               | Each session's block counts, used for the three strikes. |

Everything under `.git/pragma/` is local state that git does not track. If you delete it, you lose
the event log, the audit history and any pause. The other files are made again the next time their
command runs.

### On your machine

`$PRAGMA_HOME`, which is `~/.pragma` unless you set it:

- `bin/pragma`: the binary. The hook runs this path.
- `cache/extract/`: the extraction cache.

Claude Code's own `settings.json` (`~/.claude/settings.json`, or in `$CLAUDE_CONFIG_DIR`) holds
the hook that `pragma hooks install` adds.

## Worktrees

A worktree is a second checkout of the same repo, like a task folder for an agent. The root
checkout is the repo's main working tree.

- **One copy of `.pragma/`.** Every worktree reads the root checkout's `.pragma/`, committed or
  not. A change to the lock or config applies to every running session at once, and becomes
  durable when you commit it. If the root checkout has no `.pragma/`, a worktree uses its own.
- **One writer.** pragma writes `.pragma/` only in the root checkout. `pragma config set`,
  `pragma lock` and `pragma commit` stop in another worktree and tell you to run them in the
  root checkout. `pragma init` and `pragma exception add` write to the root checkout from anywhere.
- **Docs per worktree.** The other docs, like `AGENTS.md`, are read from each worktree, so a
  branch sees its own docs and has its own index.
- **State per checkout.** `drift`, `audit`, `backtest` and `events` keep their results per
  worktree. `pragma pause` covers the whole repo.

## Exceptions

Some findings are not violations. `pragma exception add <id> --reason <reason>` records that, in
the front matter of `.pragma/overrides.md`:

- `deliberate`: the rule is right, and this code is sanctioned anyway;
- `check-is-wrong`: the check fired on code that does not break the rule;
- `intended`: a drift finding that is meant, like an example path.

An audit exception is keyed to the rule's paragraph, the file and the matched text. A drift
exception is keyed to the doc, its paragraph and the reference. The finding then stays hidden
from `pragma audit`, `pragma drift` and `pragma backtest`. Commit `overrides.md` and it holds on
every machine. If the code changes or goes away, the exception no longer matches anything, and
`audit` and `drift` tell you how many have gone stale.

An exception does not change what the edit check blocks. Turning a rule off or changing its level
from `overrides.md` is not yet built.

## The code you already have

These three commands need no agent running.

**Drift** checks the docs against the repo. A doc that names a file, script or code name the repo
no longer has misleads every agent that reads it. `pragma drift` finds those references with no
model call and, for a moved file, suggests the new path. A path is looked for from the doc's folder,
the repo root and each package root, which is a folder with a manifest such as `package.json`,
`Cargo.toml` or `composer.json`. A name written without its extension counts when a file has that
name plus an extension. A path that starts with `/` and matches no file may be a URL of a deployed
site, so drift lists it apart as less certain and does not count it as missing. A code name is
found by a text search of the repo's source and config files, so drift can miss a renamed symbol or
match the wrong one.

**Audit** runs the lock's rules over the whole tree instead of one edit. Its findings are your
baseline. Each full audit appends its counts to `audit-history.jsonl`, a record of the trend. The
edit check stops new violations, so the count should fall as you fix things.

**Backtest** asks what pragma would have stopped in your recent history. It walks the first-parent
history of `HEAD`, 200 commits by default, and compares each commit with its first parent. It
counts the violations that commit introduced, using today's lock, not the lock of the time.
The first commit of the repo is skipped. Results are cached per commit, in `backtest-cache.json`,
under a key made from the lock's checks. When the lock changes, the cache starts over. A rule that
guards paths applies to edits only, so history never runs it. In a partial clone, a commit whose
files are not local cannot be fully checked: its hits are left out, the output says how many such
commits there were, and they are tried again on the next run.

## Known limits

- Edits made through shell commands are not blocked, and notebook edits are not checked.
- Content checks cover TypeScript and JavaScript files. Other files get path rules only.
- The hook works with Claude Code. It does not install on Windows yet.
- A plain folder without git is not supported.
- Judged and context rules can differ a little from machine to machine, because model answers
  vary. Blocking rules cannot, because they come from the lock.
- pragma does not detect rules that contradict each other.
- A doc changed only on a branch adds no blocking rule until the change reaches the root checkout.
- Drift's check of code names is a text search.

## Not yet built

- Checking judged rules when a turn ends, and sending the agent a request to fix what it found.
- Handing context rules to the agent.
- Turning a rule off, or changing its level, from `.pragma/overrides.md`.
- Promoting a generated check to blocking after the backtest.
- A built-in set of general engineering rules.
- Agents other than Claude Code.

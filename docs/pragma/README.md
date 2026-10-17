# pragma

pragma makes coding agents follow the engineering specs your repo already states in markdown:
`AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` and the files under `docs/`. It reads those docs and
splits each one into the specific things it asks for, called rules. Each rule links back to the
paragraph it came from. Your docs stay plain prose.

Rules that code can check exactly go into a lock file that you commit, `.pragma/pragma.lock`. When
Claude Code is about to write an edit that breaks one of them, a pragma hook stops the edit and
hands the agent the paragraph it breaks and how to fix it. pragma also checks the code you already
have: docs that point at files that are gone (`drift`), code that already breaks a rule (`audit`),
and violations your recent commits added (`backtest`).

## Status

pragma is a preview, version 0.1.0.

- It works with Claude Code only.
- The hook installs on macOS and Linux. It does not install on Windows yet.
- A prebuilt binary exists for macOS, on Apple Silicon and Intel. Elsewhere, build from source.
- Only the edit check runs inside a Claude Code session. Checks at the end of a turn, and sending
  violations back to the agent, are not built yet.

## Requirements

- `git`.
- Claude Code (`claude`), logged in. The hook runs inside it, and pragma reads your docs through it.
- To build from source: [rustup](https://rustup.rs) and a C compiler.

## Install

The binary must live at `~/.pragma/bin/pragma`. The hook that pragma adds to Claude Code runs
`$PRAGMA_HOME/bin/pragma`, and `PRAGMA_HOME` defaults to `~/.pragma`. The hook does not look at
your `PATH`, and it does nothing if that file is missing. The `PATH` line below is for you.

### A. The preview zip (macOS)

Unzip `pragma-0.1.0-macos.zip`, open a terminal in the `pragma` folder it creates, and run:

```sh
mkdir -p ~/.pragma/bin
mv pragma ~/.pragma/bin/
xattr -d com.apple.quarantine ~/.pragma/bin/pragma 2>/dev/null   # unsigned preview
echo 'export PATH="$HOME/.pragma/bin:$PATH"' >> ~/.zshrc
export PATH="$HOME/.pragma/bin:$PATH"
pragma version
pragma hooks install
```

### B. From source (macOS or Linux)

```sh
git clone https://github.com/xiaojia630/pragma.git
cd pragma
cargo build --release -p pragma-cli
mkdir -p ~/.pragma/bin
cp target/release/pragma ~/.pragma/bin/pragma
export PATH="$HOME/.pragma/bin:$PATH"   # also add this line to your shell's startup file
pragma version
pragma hooks install
```

The first time cargo runs, rustup installs the Rust toolchain pinned in `rust-toolchain.toml`. The
build compiles C code for the TypeScript and JavaScript grammars, so it needs a C compiler.

With either install, `pragma version` should print `pragma 0.1.0 (lock format 1)`.

### What `pragma hooks install` does

It adds one hook to Claude Code's settings file, `~/.claude/settings.json` (or
`$CLAUDE_CONFIG_DIR/settings.json`): a `PreToolUse` hook for `Edit`, `Write` and `MultiEdit`. The
hook's command is tagged `PRAGMA_HOOK=1;`, so pragma can find and remove its own entry and leaves
your hooks as they are. The hook acts only in a repo that has a `.pragma/` folder, so your other
projects are untouched. `pragma hooks status` tells you whether it is installed.

## Quick start

### Watch it stop an edit

The demo makes a small repo whose `AGENTS.md` states two rules, with both rules already in its
lock. The script is `docs/pragma/demo/try-demo.sh` in this repo, and `try-demo.sh` in the zip.

```sh
sh docs/pragma/demo/try-demo.sh     # in the zip's folder: sh try-demo.sh
cd ~/pragma-demo
claude "Add src/report.ts with a function that prints the number of users using console.log"
pragma events
```

The script creates `~/pragma-demo`. Pass another folder to put it elsewhere. The script needs
`pragma` on your `PATH`, and the `claude` step needs the hook installed. Claude tries to write
`console.log`. pragma blocks that edit and hands Claude the rule (`AGENTS.md:6`), the line that
broke it and the fix, so Claude can write the file again with `log()`. `pragma events` lists what
was stopped.

### Your own repo

```sh
cd your-repo
pragma init                     # creates .pragma/
pragma docs                     # the docs pragma will read
pragma extract --estimate       # how many model calls reading them takes
pragma config set --docs-exclude 'docs/plans/**'    # optional: skip working notes
pragma extract                  # reads the docs with your claude CLI
pragma lock                     # puts the rules that can block into .pragma/pragma.lock
pragma lock show                # what blocks edits now
pragma commit -m "chore: add pragma"                # commits .pragma/ and nothing else
```

A big doc folder costs more model calls, so check the estimate first. `pragma config set` replaces
the exclude list, so pass every glob you want to keep.

### The code you already have

These need no model and no agent. `audit` and `backtest` check the rules in the lock, so run them
after `pragma lock`.

```sh
pragma drift       # docs that name files, scripts or code that no longer exist
pragma audit       # code that already breaks the rules in the lock
pragma backtest    # violations your last 200 commits added that the lock would have stopped
pragma events      # what the edit check blocked, let through or left unchecked
```

When a finding is not a violation, record why. The `<id>` is printed next to the finding:

```sh
pragma exception add <id> --reason deliberate    # or check-is-wrong, or intended for drift
```

Then run `pragma commit -m "chore: record an exception"` so the exception holds on every machine.

## What to expect

- Reading your docs makes a rule blocking only when code can check it exactly: protected paths,
  import boundaries and file naming. Much of what docs say is advice code cannot check. pragma
  keeps those rules as judged or context rules for later checks, and nothing acts on them yet.
  `pragma status` counts the rules by level.
- Rules you write into `.pragma/pragma.lock` by hand block too. `pragma lock check` validates
  them, and `pragma lock` never changes them. The demo's lock shows the format.
- A rule stops an edit only if the edit adds a violation. Existing code that already breaks a rule
  is not stopped. `pragma audit` finds it.
- Edits made through shell commands are not stopped. Only Claude's edit tools are.
- Each edit check has a 50 ms budget. Past it, the edit goes through rather than slowing the agent.

## Turning it off and uninstalling

- **Pause one repo.** `pragma pause` stops the hook for this repo on this machine. `pragma resume`
  starts it again.
- **Skip it for one session.** `PRAGMA_DISABLE=1 claude` makes the hook do nothing in that session.
- **Remove the hook everywhere.** `pragma hooks uninstall` removes pragma's entry from Claude
  Code's settings and leaves your other hooks.
- **Remove pragma from your machine.** After `pragma hooks uninstall`, delete `~/.pragma`. That
  removes the binary and the cache. Delete the `PATH` line from your shell's startup file.
- **Remove pragma from a repo.** Delete the repo's `.pragma/` folder and commit the deletion. Your
  docs stay as they are. You can also delete `.git/pragma/`, where pragma keeps its local state.

If you delete `~/.pragma` first, the hook finds no binary and does nothing, but its entry stays in
Claude Code's settings. Remove it by hand: delete the hook whose command starts with
`PRAGMA_HOOK=1;`.

## Troubleshooting

**`pragma: command not found`.** Your shell does not have `~/.pragma/bin` on its `PATH`. Add the
`export PATH=...` line from the install steps to your shell's startup file, then open a new
terminal. `command -v pragma` shows where the shell finds it.

**macOS refuses to open pragma.** The preview is not notarized by Apple, so Gatekeeper can block
it. Run `xattr -d com.apple.quarantine ~/.pragma/bin/pragma`. If macOS still refuses, allow it
under System Settings > Privacy & Security.

**Reading the docs is paused.** `pragma status` says why. It prints either
`the claude CLI is not logged in; run claude and /login` or
`the claude CLI was not found on PATH; install Claude Code`. Fix that and run `pragma extract`
again. Answers already read are cached, so it carries on.

**An edit was not stopped.** Check, in this order:

1. `pragma hooks status` says the hook is installed, and `~/.pragma/bin/pragma` exists. In
   Claude Code, `/hooks` lists the active hooks. If pragma's is missing in a session that was open
   during the install, start a new session.
2. `pragma status` does not say the repo is paused, and `PRAGMA_DISABLE` is not set in the
   environment Claude Code runs in.
3. The rule is in the lock. `pragma lock show` lists the rules that block. A rule that
   `pragma extract` found but `pragma lock` has not copied does not block.
4. The file is in the rule's scope. `pragma lock show --json` prints each rule's `scope`.
5. The edit adds a violation. An edit that leaves a file's existing violations as they are is
   allowed.
6. The edit went through Claude's `Edit`, `Write` or `MultiEdit` tool, not a shell command.
7. `pragma events` shows `unchecked` for the file. The check ran past its 50 ms budget, or the file
   is over 512 KB, or the lock could not be read; the reason is on the line. `escalated` means the
   rule had already blocked that file three times in the session.

**A rule blocks something it should not.** `pragma pause` stops the hook while you decide. Then
reword the paragraph that states the rule, and run `pragma extract` and `pragma lock`. For a rule
you wrote by hand, edit `.pragma/pragma.lock` and run `pragma lock check`.

## Privacy and cost

pragma runs on your machine. It has no server, no API key of its own and no telemetry. The only
text that leaves your machine is the paragraphs of your docs, which `pragma extract` sends to Claude
through your own `claude` CLI and account. Your source files are never sent. Nothing else that
pragma does calls a model.

Each call runs `claude -p` with tools off, no MCP servers, none of your settings, hooks disabled
and no session saved. pragma sets `PRAGMA_JUDGE=1` on the call so its own hook stays silent. Each
call counts against your Claude usage, takes up to 10 paragraphs, and uses the model `haiku` unless
you set `model:` in `.pragma/config.md`. `pragma extract --estimate` shows the number of calls
first.

Answers are cached per paragraph in `~/.pragma/cache`, shared by every repo and worktree on your
machine. A paragraph you did not change is read again only if its cached answer was removed, which
happens after 30 days unused, or if you switched models.

## More

- [Command reference](commands.md): every command and flag.
- [How it works](how-it-works.md): docs, rules, the lock, the edit check and where files live.

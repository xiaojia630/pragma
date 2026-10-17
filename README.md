# pragma

[![ci](https://github.com/xiaojia630/pragma/actions/workflows/ci.yml/badge.svg)](https://github.com/xiaojia630/pragma/actions/workflows/ci.yml)

pragma makes coding agents follow the engineering specs your repo already writes down in
markdown: AGENTS.md, CLAUDE.md, CONTRIBUTING, ADRs, design docs.

- It reads your docs into rules, each linked to the paragraph it came from.
- Rules that code can check go into a committed lock. When Claude Code tries an edit that
  breaks one, pragma stops the edit and hands the agent the paragraph and the fix.
- It also checks the code you already have: docs that point at code that's gone, code that
  already breaks the rules, and what recent commits added.

![pragma stopping an edit that breaks AGENTS.md](docs/pragma/images/blocked-edit.svg)

pragma is one binary. It runs locally, and reads your docs through your own Claude Code account.

## Install

macOS or Linux, one line:

```sh
curl -fsSL https://raw.githubusercontent.com/xiaojia630/pragma/main/install.sh | sh
pragma hooks install
```

The first line puts `pragma` in `~/.pragma/bin` and adds it to your `PATH`; the second lets it
check Claude Code's edits. It only acts in repos you turn it on in. You need git and
[Claude Code](https://claude.com/claude-code).

**See it work in two minutes**, in a throwaway demo repo:

```sh
curl -fsSL https://raw.githubusercontent.com/xiaojia630/pragma/main/docs/pragma/demo/try-demo.sh | sh
cd ~/pragma-demo
claude "Add src/report.ts with a function that prints the number of users using console.log"
```

Claude tries to write `console.log`, which the demo's AGENTS.md rules out. pragma stops the edit
and hands Claude the rule and the fix.

## Use it

```sh
cd your-repo
pragma init                  # creates .pragma/; commit it
pragma extract --estimate    # how many model calls reading your docs takes
pragma extract               # reads your docs with your claude CLI
pragma lock                  # the rules that can block go into .pragma/pragma.lock
pragma audit                 # what your code already breaks
```

Full docs:
- [docs/pragma/README.md](docs/pragma/README.md): install, a two-minute demo, troubleshooting.
- [docs/pragma/commands.md](docs/pragma/commands.md): every command.
- [docs/pragma/how-it-works.md](docs/pragma/how-it-works.md): how it works.

## Develop

See [AGENTS.md](AGENTS.md) for the layout, commands and rules of this repo.

## License

Apache-2.0, see [LICENSE.md](LICENSE.md).

# .pragma

This folder holds pragma's settings for this repo. pragma makes coding agents follow the
engineering specs in this repo's docs.

- `config.md`: repo settings.
- `overrides.md`: corrections and exceptions, kept by pragma.
- `pragma.lock`: the rules that block edits, compiled from the docs. It is generated; review its
  diffs like a lockfile.
- `specs/`: extra specs written for pragma, in plain markdown.

Deleting this folder turns pragma off for the repo. The docs stay exactly as they are.

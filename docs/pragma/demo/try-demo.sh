#!/bin/sh
# Creates a small repo whose AGENTS.md states two rules, with those rules in its pragma lock,
# so you can watch pragma stop Claude Code from breaking them.
#   sh try-demo.sh [folder]      (default: ~/pragma-demo)
set -e
DIR="${1:-$HOME/pragma-demo}"
command -v pragma >/dev/null || { echo "pragma is not on your PATH yet: see the README, under Install."; exit 1; }
command -v git >/dev/null || { echo "git is needed."; exit 1; }
if [ -e "$DIR" ]; then echo "$DIR already exists. Pass another folder: sh try-demo.sh /path/to/new-folder"; exit 1; fi

mkdir -p "$DIR/src"
cd "$DIR"
git init -q

cat > AGENTS.md <<'EOF'
# Demo

## Conventions

- Use top-level `import` statements; do not use `require()`.
- Never call `console.log` in `src/`; log through `log()` from `src/log.ts`.
EOF

cat > src/log.ts <<'EOF'
export function log(...args: unknown[]): void {
  process.stdout.write(args.join(' ') + '\n');
}
EOF

cat > src/app.ts <<'EOF'
import { log } from './log';

export function main(users: string[]): void {
  log('starting with', users.length, 'users');
}
EOF

pragma init >/dev/null

# The two rules, as `pragma lock` would write them. Rules in the lock block edits; hand-written
# ones like these are kept as they are when the docs are read again.
cat > .pragma/pragma.lock <<'EOF'
{
  "format": 1,
  "clauses": [
    {
      "id": "cb689b5f2615be31",
      "anchor": { "path": "AGENTS.md", "heading_path": ["Demo", "Conventions"], "para_hash": "28ebb56d72546910", "lines": [5, 5], "quote": "- Use top-level `import` statements; do not use `require()`." },
      "text": "Use top-level import statements, not require().",
      "scope": ["**/*.ts", "**/*.tsx"],
      "check": { "kind": "library", "rule": "forbidden-pattern", "pattern": "require($$$ARGS)", "examples": { "bad": "const fs = require('fs');", "good": "import fs from 'fs';" } },
      "fix": "Use a top-level import instead."
    },
    {
      "id": "c88859b94d9fa5a6",
      "anchor": { "path": "AGENTS.md", "heading_path": ["Demo", "Conventions"], "para_hash": "2136f31bfe0fb493", "lines": [6, 6], "quote": "- Never call `console.log` in `src/`; log through `log()` from `src/log.ts`." },
      "text": "Never call console.log in src/; log through log() from src/log.ts.",
      "scope": ["src/**/*.ts", "!src/log.ts"],
      "check": { "kind": "library", "rule": "forbidden-pattern", "pattern": "console.log($$$ARGS)", "examples": { "bad": "console.log('users', count);", "good": "log('users', count);" } },
      "fix": "Import { log } from './log' and call log(...) instead of console.log(...)."
    }
  ]
}
EOF

pragma lock check
git add -A
git -c user.name="pragma demo" -c user.email="demo@localhost" commit -qm "pragma demo"

echo
echo "Demo ready in $DIR. Next:"
echo "  cd $DIR"
echo "  claude \"Add src/report.ts with a function that prints the number of users using console.log\""
echo "pragma stops the console.log edit and hands Claude the rule and the fix. Then run: pragma events"
echo "(If you haven't yet, run 'pragma hooks install' first.)"

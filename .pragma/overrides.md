---
exceptions:
- kind: violation
  clause: cb689b5f2615be31
  para_hash: 28ebb56d72546910
  file: packages/core/src/primitives/sqlite-store/codegen/compile-drizzle-schema.ts
  matched: require('drizzle-kit/api')
  reason: deliberate
  note: drizzle-kit/api is CommonJS-only; createRequire is how this module loads it
---
# pragma overrides

Corrections and exceptions for this repo's rules. pragma keeps the list above; the text here
is yours.

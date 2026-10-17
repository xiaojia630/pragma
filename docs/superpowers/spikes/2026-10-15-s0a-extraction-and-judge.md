# S0a: extraction and judge quality on AGENTS.md

Spike, throwaway code. Claude Code 2.1.301. Judge calls use the flags from S0c ("Decisions for S1 and S4"),
prompt on stdin, `PRAGMA_JUDGE=1`, models `haiku` and `sonnet` (aliases).

## Result in one table

| Question | Answer |
| --- | --- |
| Extraction precision (30-paragraph sample, 50 clauses) | 0.96 (48 right, 2 wrong) |
| Extraction recall (sample) | 0.98 (1 missed) |
| Paragraphs / clauses over the whole file | 168 paragraphs, 229 clauses, all 17 batches parsed |
| Rung split (all 229) | judged 117, context 103, blocking 9 |
| Median seconds per batch of 10 paragraphs | 19.9 s (range 5.7 to 30.4; 4 batches ran at once) |
| Judge, haiku | precision 1.00, recall 1.00, median 5.5 s (12 cases) |
| Judge, sonnet | precision 1.00, recall 1.00, median 5.4 s (12 cases; two 62 s outliers) |
| Default model for `.pragma/config.md` | `haiku` (cheapest at or above 0.8 precision) |
| `--json-schema` | Works: validated object comes back in `structured_output`; `result` still holds the JSON text. 8.0 s on haiku. |

## Extraction

`paragraphs.py` split AGENTS.md (419 lines) into 168 units (prose paragraphs and list items; fenced
blocks dropped). Seventeen batches of up to 10 went through `claude -p` on the account default model
(no `--model` given, so the account default), 4 at a time. Every batch returned parseable JSON in
`result`, with no code fences needed; 168 of 168 paragraph ids came back.

Sample: 30 paragraphs drawn with `random.seed(7)` (not `shuf`, so it can be reproduced), hand-labelled.

- Extracted 50 clauses; 48 correct, 2 wrong. Wrong ones: p39 second clause (the oxlint config line)
  invents a fix "keep the enabled rule groups on"; p12 (`packages/shared/` description) invents
  "reuse ... instead of duplicating". Both are descriptions turned into instructions the doc never gave.
- Missed 1: p8 (`apps/workspace-server/` one-line description) came back empty, while the same shape of
  line (p21, p11, p15) got a context clause. Inconsistent, low harm.
- Good behavior: link-only list items (Further Reading) correctly gave empty lists; compound
  paragraphs were split into single clauses (p107 into 7, p112 into 5), each with a usable fix.
- Rung problems (clause right, rung debatable): "Lint with oxlint" and "Call out high-risk changes in the
  PR description" are labelled judged but cannot be checked from a diff; p112 splits one "do not ... unless
  the user asks" list into 4 context and 1 judged. Process rules ("run the gate") need a fourth bucket
  or should land in context. Directory-layout statements ("X lives under Y") came out as context or
  judged inconsistently (p115 judged, p120 context).
- Rung split over all 229 clauses: 117 judged, 103 context, 9 blocking.

Blocking clauses and the four library rules (all 9 listed):

| Library rule | Count | Clauses | Params sensible? |
| --- | --- | --- | --- |
| forbidden-pattern | 5 | `require()`; `export { } from` re-export; `asProvisioned(...)!`; `asAvailableProject(...)!`; `project.taskManager` | Yes. Patterns are valid ast-grep shapes with bad/good examples. `$PROJECT.taskManager` is loose (matches any `.taskManager`). |
| protected-paths | 4 | `build/`; non-pnpm lockfiles; numbered Drizzle migrations and `drizzle/meta/`; `dist/ release/ out/ build/` | Mostly. `**/build/**` is too wide (matches source dirs named build); the model dropped the "unless the task is about packaging" exception into `fix` only. |
| import-boundary | 0 | none | The model never used it. AGENTS.md has no import-boundary sentence that is explicit enough; "Never re-export" was mapped to forbidden-pattern instead. |
| file-naming | 0 | none | "Components use PascalCase" was judged, not file-naming (it is about identifiers, so that is right). |

Only 9 of 229 clauses (4%) are blocking, and the PTY, state-guard and Result clauses that the golden
set covers are judged. Blocking is a thin slice of a prose AGENTS.md; the value on this file rests on the
judged rung.

## Judge

12 golden cases (`crates/pragma-evals/golden/agents-md/`): six rules, each with one violating and one
clean file, presented as a new-file diff against a single rule `R1`. Positive = flagged a violation.

| Model | TP | FP | FN | Unparseable | Precision | Recall | Median s |
| --- | --- | --- | --- | --- | --- | --- | --- |
| haiku | 6 | 0 | 0 | 0 | 1.00 | 1.00 | 5.5 |
| sonnet | 6 | 0 | 0 | 0 | 1.00 | 1.00 | 5.4 |

Both models classified all 12 correctly and returned bare JSON (code fences stripped by the harness if
present). Sonnet had two 62 s calls (`expected-failure-result`, `state-guard-ready`) while 4 calls ran in
parallel, probably a retry or backoff, which backs the "wrap in a timeout, fail open" decision.

Default model: **haiku**. It meets the 0.8 bar, ties sonnet, and costs less. Per spec section 11, send-back
stays automatable.

Caveat on the numbers: perfect scores on 12 cases do not show 0.8 will hold. The violating files are
blatant, one rule per call, and the clean cases are near-copies of the bad ones. The golden set is a seed
for `pragma eval`; S3 needs harder cases (subtle violations, several rules at once, large diffs, rules
that do not apply to the file) before the 0.8 gate means much.

## Notes for later tasks

- Golden case line numbers: for `expected-failure-*` the cited line 199 is the last line of a paragraph
  that starts at 198 (the paragraph spans two lines), so the quote is inside that paragraph, not at its start.
- The extraction prompt with no `--model` ran on the account default; S3 should pin the model.
- `--json-schema` is usable for the judge and would remove fence-stripping; use `structured_output`.
- Scripts and raw output are not committed. They lived in the session scratchpad.

## Prompts (verbatim)

### Extraction prompt

```text
You turn a team's engineering docs into the specific things they ask for.

For each paragraph below, list its clauses: each single thing the paragraph asks a coding agent to do
or not do. For each clause give:
- "text": the instruction in one sentence, in the doc's words where possible.
- "rung": "blocking" if code can check it on a single file edit, "judged" if a reviewer could check
  it from a diff, "context" if it is background an agent should know.
- "scope": glob patterns for the files it applies to, from paths the paragraph names; [] when it
  applies everywhere.
- "check": for blocking clauses, one of
  {"rule": "forbidden-pattern", "pattern": "<ast-grep pattern>", "bad": "<code that breaks it>", "good": "<code that follows it>"},
  {"rule": "import-boundary", "forbid": ["<module specifier glob>"]},
  {"rule": "protected-paths"},
  {"rule": "file-naming", "case": "kebab" | "camel" | "pascal" | "snake"};
  null for other rungs.
- "fix": how to fix a violation, in one sentence.

A paragraph with no instruction gets an empty list. Answer with JSON only, no prose:
{"paragraphs": [{"id": "<id>", "clauses": [...]}]}

Paragraphs, one JSON object per line:
```

### Judge prompt

```text
You review a code change against a team's engineering rules.

Rules (id: text):
<rules>

Change (unified diff):
<diff>

List every rule the change breaks. List a rule only when the added lines clearly break it.
Answer with JSON only, no prose:
{"violations": [{"rule": "<id>", "file": "<path>", "line": <line in the new file>, "why": "<one sentence>"}]}
When nothing is broken, answer {"violations": []}.
```

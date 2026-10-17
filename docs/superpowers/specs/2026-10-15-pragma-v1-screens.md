# pragma v1 screens

The 41 screens for pragma v1, 2026-10-15, revised the same day for value on existing code. They
serve the v1 architecture spec (`2026-10-15-pragma-v1-architecture-design.md`); every § points
into it, and S1–S10 are its build slices.

## How the 41 were chosen

- Screens go where the user makes a decision or where pragma wins or loses their trust: the
  repo's state on first open, the block, the turn check, the violation on the diff, and the
  Rulebook.
- A screen is one frame at desktop size, drawn inside the existing app shell, in one theme.
- States that only change content are variants listed under their screen, not screens of their
  own. A component sheet (all states of one component) counts as one screen.
- Not screens: the text agents receive (the block and send-back messages, whose copy is in
  §5.3), CLI output, and `.pragma/` file formats.

## First open (4)

**1. Enable offer** · S5 · §5.6
Shown on opening a repo: the docs pragma found with paragraph counts, the pack, roughly how many
model calls extraction will take, and what enabling does (creates `.pragma/` in the main
checkout, installs hooks). The first time, a short strip explains how pragma works: your docs →
the specific things they ask for → the ladder → every turn checked. Enable or Not now.
_Variant:_ repo already enabled elsewhere (a committed `.pragma/` makes enabling instant).

**2. Enabling** · S5 · §5.2, §5.6
Live progress: pack and committed lock active, hooks installed, drift done in seconds, each doc's
extraction with its clauses appearing, then the audit and the backtest.
_Variants:_ model CLI missing or logged out (pack, lock and drift still work, with how to fix
it); extraction paused.

**3. Repo health** · S5, history part in S8 · §5.6, §5.7, §5.10
The first-open report and its home afterwards, in three parts: docs that point at code that's
gone, code that already breaks the docs (the audit baseline with its trend), and what recent
history broke. Each part leads to its detail.
_Variants:_ still running; a clean repo; before S8, without the history part.

**4. No docs yet** · S8 · §5.2, §5.9
A repo with no spec docs: the pack is on, plus a nudge to write an AGENTS.md and what makes a
good one.

## Existing code (2)

**5. Docs drift** · S5 · §5.10
Doc problems grouped by doc: missing paths (with the new path when a file moved), unknown
scripts, and, set apart, code names pragma could not find. Apply the suggested fixes as one doc
diff, send the rest to an agent as a task, or mark one as intended.
_Variant:_ drift caused by a turn, arriving from that turn's report.

**6. Burn-down** · S5 · §5.10
Existing violations by rule and area, with counts and a trend that only goes down. Pick groups to
start fix tasks, with a preview of each task's prompt; see tasks in flight and what merged.
Opening a finding shows the code with its violation popover (20).
_Variant:_ a group expanded to its findings.

## Agents at work (12)

**7. Task list** · S6 · §5.6
A project's parallel tasks, each with its badge; tasks that need the user sort first. The project
sidebar shows each project's pragma status: on, paused, off, extracting, needs attention.
_Variant:_ filtered to "needs attention".

**8. Task view, clean turn** · S6 · §5.6
The baseline layout the rest of this group builds on: agent terminal, the badge in the titlebar
showing clean, the pragma tab.

**9. Badge states** (component sheet) · S6 · §5.6
Checking, clean, blocked (n, live), violations (n), sent back, escalated, unchecked (reason),
paused, off, needs install, each with its hover summary.

**10. Blocked mid-edit** · S6 · §5.3, §5.5
The moment pragma stops an edit: the terminal shows the block message, the badge counts
"blocked 1" live, and an activity entry links to the rule.
_Variant:_ the third block on the same rule lets the edit through as escalated.

**11. Checking at turn end** · S6 · §5.6
The agent stopped and pragma holds the task in "checking": code checks done, the judge running
over n rules, time elapsed. The "finished" notification waits for it.

**12. pragma tab: the task's turns** · S6 · §5.4, §5.5
Every turn with its outcome (clean, sent back then fixed, escalated, unchecked), totals, and the
rules allowed for this task, each with undo.

**13. Turn detail: violations sent back** · S6 · §5.4, §5.5
One turn's violations grouped by file, each citing its paragraph; violations about how the work
was done, with the transcript step as evidence; the exact message sent back; "1 of 2 automatic
send-backs".
_Variants:_ resolved on the next turn; the turn left a doc naming a file it deleted (drift flag,
with the fix offered).

**14. Escalated: needs you** · S6 · §5.5
pragma sent the turn back twice, or blocked the same rule three times, and violations remain.
Shows what's left and the choices: send back again, fix it yourself, allow for this task, not a
violation.
_Variant:_ escalated mid-edit.

**15. Manual send-back** · S6 · §5.5
Manual mode: the violations, a "Send back" button, and an editable preview of the fix request
before it reaches the agent.

**16. Unchecked turn** · S6 · §5.4, §8
What pragma could not check and why (judge timed out, diff too big, model unavailable, an edit
check over time), what it still checked, and Retry.

**17. Agent changed the rules** · S6 · §5.4
The turn edited a spec doc or `.pragma/`: the change, what it does to the rules, and "Looks fine"
to clear the flag.
_Variant:_ the change was made from outside the worktree.

**18. What the agent was told** · S6 · §5.3
The rules pragma handed this session, repo-wide and per area; docs skipped because the agent
loads them itself; the context budget used.

## Reviewing the diff (2)

**19. Diff with inline violations** · S6 · §5.6
The existing diff view with violations on their exact lines, gutter markers, and per-file counts.
_Variants:_ a violation outdated by later edits; judged and blocking styles side by side.

**20. Violation popover** · S5 · §5.5, §5.6, §5.10
The rule's quote; its doc, line and heading; why the code breaks it; the fix. Actions: Not a
violation, Allow for this task, Open in Rulebook.
_Variants:_ "Not a violation" asking why (a deliberate exception, or the check is wrong); after
"Allow for this task"; on existing code, from burn-down, where "Allow for this task" becomes
"Start a fix task".

## Chat (1)

**21. Chat with pragma** · after spike S0b · §13
A chat (ACP) conversation with pragma's events in the transcript: a blocked edit card, the turn
check result, a send-back.

## Rulebook (15)

**22. Rulebook overview** · S7 · §5.6
Docs with clause counts per rung, the pack in its own section, a health strip (needs attention,
contradictions, uncommitted changes), search, rung filters.
_Variants:_ first extraction running; re-extracting after a doc change; model unavailable.

**23. Doc view** · S7 · §3, §5.2
A doc rendered as written, each paragraph's clauses beside it as rung chips. Every citation
lands here.
_Variants:_ arrived from a citation (paragraph highlighted); a paragraph naming a missing file
(drift); superseded doc (no clauses); a paragraph that failed extraction.

**24. Clause detail** · S7 · §3, §5.8, §5.10
The clause and its source paragraph, rung, scope, check, backtest hits, exceptions (deliberate,
and the check is wrong), and whether it is derived or overridden, with revert.

**25. Changing the rung** · S7 · §2, §3
The ladder control (context, judged, blocking, off), why blocking needs a code check, and the
lock change that moving to blocking makes.

**26. Scope editor** · S7 · §5.2
Globs with a live preview of matching files, and the scope inherited from where the doc lives.

**27. Check editor** · S7 · §5.1, §5.8
A library rule's params, or a pattern with its bad and good examples; a test run against current
code and the backtest hits before saving.

**28. Reword a clause** · S7 · §5.8
The user's wording beside the derived one, saved as an override anchored to the paragraph.

**29. Suggest rewording the doc** · S7 · §5.6
A drafted rewrite of a vague or conflicting paragraph as a diff on the doc. Accept writes the
doc; reject dismisses it. Never automatic.

**30. Contradictions** · S7 · §5.2
Two clauses that disagree, side by side with their docs, which one wins today and why, and the
ways out: override, or suggest rewording.

**31. Needs attention** · S7 · §5.8, §5.10, §8
Stale overrides and exceptions (re-anchor or drop), overrides placed by fuzzy match (confirm),
and paragraphs that failed extraction (retry).

**32. Promotion review** · S8 · §5.7
A generated check with its backtest hits as code snippets. Mark each hit real or false, then
promote to blocking or keep it judged.

**33. The pack** · S8 · §5.9
The good-engineering clauses, which of them the repo's docs override, and switches for each
clause and for the whole pack.

**34. Which rules apply here** · S7 · §5.1
Pick a file or folder; see every clause that applies, by rung, with the doc each comes from.

**35. Write a spec** · S7 · §4, §5.2
A new `.pragma/specs` doc in the editor, with frontmatter for scope and rung, and the clauses
pragma extracts appearing live beside it.
_Variant:_ editing an existing doc such as AGENTS.md with the same live preview.

**36. Review and commit rule changes** · S5 · §4, §5.2
What changed in what is enforced since the last commit (blocking rules added, changed or
removed; overrides; exceptions; promotions), the raw `.pragma/` diff behind a toggle, and Commit.
Also shown right after enabling.

## History (2)

**37. Decisions with violations** · S8 · §5.7
The Decisions view with markers on decisions whose commits broke rules, and a filter for them.
_Variant:_ filtered to one rule, arriving from a clause's hits.

**38. Decision detail with violations** · S8 · §5.7
The existing detail panel with a "Broke these rules" section linking to the clauses and diffs.

## Settings and system (3)

**39. Repo settings** · S5 · §4
Send-back mode with the precision bar explained, caps, model, doc include and exclude globs with
a preview, the pack switch.

**40. pragma on this machine** · S5, SSH rows in S9 · §5.3, §10
Pause for this repo, hook status with install and uninstall, binary version, model CLI status,
model calls used, and hosts (local and SSH) with pragma's version on each.
_Variants:_ paused; needs install or update; a host's binary out of date.

**41. Notifications** (component sheet) · S6 · §5.6
In-app toasts and OS notifications: blocked, sent back, escalated, unchecked, rules changed,
extraction finished, audit finished.

## Design order

Design one slice ahead of the build.

1. **First open and existing code, for S5:** 1, 2, 3, 5, 6, 20, 36, 39, 40.
2. **The agent loop, for S6:** 8, 9, 10, 11, 12, 13, 19; then 7, 14, 15, 16, 17, 18, 41.
3. **Rulebook, for S7:** 22–31, 34, 35.
4. **History and pack, for S8:** 4, 32, 33, 37, 38.
5. **After spike S0b:** 21.

## Coverage

Every user-facing behavior in the spec has a screen:

| Spec                                          | Screens                 |
| --------------------------------------------- | ----------------------- |
| Enable and first open (§5.6)                  | 1, 2, 3, 36             |
| Drift, audit, burn-down, exceptions (§5.10)   | 3, 5, 6, 20, 24, 31     |
| Badge and agent status (§5.6)                 | 7, 8, 9, 11             |
| Blocking mid-edit (§5.3)                      | 10, 14                  |
| Turn checks, tamper and drift flags (§5.4)    | 12, 13, 16, 17          |
| Send back and the user's escapes (§5.5)       | 12–15, 20, 40           |
| Context hand-off (§5.3)                       | 18, 34                  |
| Inline violations (§5.6)                      | 19, 20                  |
| Rulebook and overrides (§5.6, §5.8)           | 22–31, 34–36            |
| Backtest and promotion (§5.7)                 | 3, 32, 37, 38           |
| Pack (§5.9)                                   | 4, 33                   |
| Settings and failure modes (§4, §8)           | 2, 16, 22, 39, 40, 41   |
| Chat sessions (§13, S0b)                      | 21                      |

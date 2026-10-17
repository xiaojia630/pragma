# S0c: what Claude Code's hooks can do

Spike run on 2026-10-16 against Claude Code **2.1.301**, in a throwaway repo at `/tmp/pragma-s0c/repo`
with project-local `.claude/settings.local.json` (the user's `~/.claude/settings.json` was not touched).

## Flags that exist (`claude --help`)

Present: `--print/-p`, `--output-format`, `--json-schema`, `--model`, `--settings`, `--setting-sources`,
`--tools`, `--allowedTools`, `--disallowedTools`, `--strict-mcp-config`, `--permission-mode`,
`--bare`, `--no-session-persistence`, `--disable-slash-commands`, `--input-format`.
Missing: **`--max-turns`** (not in this version; bound runs with `timeout` instead).

## Findings

| # | Question | Answer | Evidence |
|---|----------|--------|----------|
| 1 | Tool names for file edits | `Write` (create) and `Edit` (modify). `MultiEdit` never appeared; the agent said Edit takes one replacement per call and made separate calls. | Exp A: PreToolUse log had `3 "tool_name":"Edit"`, `1 "tool_name":"Write"`. Matcher `Edit\|Write\|MultiEdit` still works (unused names are harmless). |
| 2 | Events that accept `additionalContext` | All four tried: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, via `{"hookSpecificOutput":{"hookEventName":"<Event>","additionalContext":"..."}}`. | Exp A: the agent listed APPLE (SessionStart), BANANA (UserPromptSubmit), CHERRY (PreToolUse), DATE (PostToolUse) as reaching it. |
| 3 | PreToolUse exit 2 reaches the agent | Yes. The edit was refused (file stayed `one`) and the agent quoted the stderr message verbatim: "blocked the edit, citing a rule at `NOTES.md:1`: notes say three, never two. Fix: write three." The agent did not write `three` on its own because that contradicted the user's request; it offered the choice. A real fix request must be consistent with the task, or the agent will push back. | Exp B: final `notes.txt` = `one`; the Edit was attempted once (Write then Edit in the log). |
| 4 | Stop `{"decision":"block","reason":...}` sends the agent back | Yes, once. The agent read the reason, appended `bye`, and finished. First Stop input has `"stop_hook_active":false`, the second `"stop_hook_active":true`. Stop input also carries `last_assistant_message`. | Exp C: `notes.txt` = `one\nbye`; `grep -c stop_hook_active` = 2; values `false` then `true`. |
| 5 | Isolated judge call (no hooks, no tools, JSON out) | Flag set below. `result` held `{"ok": true}`, `is_error:false`, `num_turns:1`, 0 hook log lines added, 4.4 to 5.9 s wall time. Control run without the flags added 7 hook log lines. | Exp D. |
| 6 | App's own Stop handling; parallel Stop hooks | App installs `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop` hooks (`packages/plugins/src/agents/impl/claude/hooks.ts`). Stop becomes canonical `status`/`stop` with `lastAssistantMessage` (`parse-hook-event.ts`), applied by `hook-pipeline.ts`, so the app marks the agent idle. Claude Code runs same-event hooks in parallel: with hook A sleeping 3 s and hook B instant, B started and ended while A was still running (starts 0.002 s apart; A ended 3.0 s later). A pragma Stop block therefore does not delay or suppress the app's idle signal. | Exp E: `/tmp/pragma-s0c/par.log`. |

## Observations worth keeping

- Hook stdin is one JSON object with `session_id`, `transcript_path`, `cwd`, `prompt_id`, `permission_mode`,
  `effort`, `hook_event_name`, `tool_name`, `tool_input` (Write: `file_path`, `content`), `tool_use_id`.
  `cwd` is the resolved path (`/private/tmp/...` on macOS, not `/tmp/...`).
- `--bare` is NOT usable for the judge: it never reads OAuth/keychain, so a logged-in user gets
  `Not logged in` (`is_error:true`). Do not use it.
- `--tools` is variadic: place the prompt before it, or send the prompt on stdin, or `claude -p --tools ""`
  swallows the prompt ("Input must be provided..."). Stdin was used for the final run.
- Without `</dev/null` or stdin, `claude -p` waits 3 s and warns about no stdin data.
- `--setting-sources ""` alone was enough to get 0 hook lines; adding `--settings '{"disableAllHooks":true}'`
  is a belt-and-braces guard and was the cheapest run (US$0.0008 vs 0.015 to 0.018; cost varies with caching,
  and the unrestricted control cost US$0.115).
- `PRAGMA_JUDGE=1` in the env is the pragma-side guard (hooks of ours exit silently when it is set).

## Decisions for S1 and S4

- **PreToolUse matcher:** `Edit|Write|MultiEdit` (only `Edit` and `Write` observed in 2.1.301; keep `MultiEdit` for other versions).
- **Events that accept added context:** SessionStart, UserPromptSubmit, PreToolUse, PostToolUse.
  Output shape: `{"hookSpecificOutput":{"hookEventName":"<Event>","additionalContext":"<text>"}}`.
- **Block mid-edit:** PreToolUse, print the message to stderr, exit 2. The agent sees the message verbatim.
- **Send back after a turn:** Stop hook prints `{"decision":"block","reason":"..."}`; the next Stop has
  `stop_hook_active:true` and the hook must not block again then.
- **Judge call flags** (exact):

  ```
  echo "$PROMPT" | PRAGMA_JUDGE=1 claude -p --output-format json --tools "" --no-session-persistence \
    --strict-mcp-config --disable-slash-commands --setting-sources "" --settings '{"disableAllHooks":true}'
  ```

  Read the `result` field of the JSON. Add `--model <m>` to pick a model and `--json-schema` for structured
  output (flag exists; not exercised here). Expect about 4 to 6 s per call, so run beside the agent and fail open.
  No `--max-turns`: wrap in a timeout.

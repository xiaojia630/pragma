# S0b: how to catch edits in chat (ACP) sessions

Spike run on 2026-10-16 against Claude Code **2.1.301**, `@agentclientprotocol/claude-agent-acp` **0.81.2**
(Agent SDK 0.3.280), in a throwaway repo at `/tmp/pragma-s0b/repo` with project-local
`.claude/settings.local.json`. No product code was changed.

## How this was tested (and what was not)

The dev app needs its Electron UI, which cannot run headless. Instead a small ACP client
(`/tmp/pragma-s0b/client/run.mjs`, not committed) launched the **same adapter the app launches**
(`node .../claude-agent-acp/dist/index.js`, with `CLAUDE_CODE_EXECUTABLE` pointed at the installed `claude`
exactly as `packages/plugins/src/agents/impl/claude/index.ts` `buildSpawn` does), opened a session in the scratch
repo, optionally switched mode with `setSessionMode`, sent one prompt, and logged every ACP update and
`requestPermission` call. The app's Claude plugin passes no `claudeCode` options and no `settingSources`/hook
overrides (grep of the app's TypeScript sources; the only `disableAllHooks` is the usage probe),
so the adapter runs with its own defaults.

**Verified by running:** hooks fire in ACP sessions, a PreToolUse block reaches the agent and the ACP client,
behavior in default and bypass modes, and whether `requestPermission` is called per mode.
**From reading code only (not run):** that `cell.ts` `requestPermission` writes the `permission_request` raw-log
entry, and the session-manager wiring. Not exercised: the app's own UI/transcript rendering of the failed tool
call, Electron-as-Node (`ELECTRON_RUN_AS_NODE`) instead of plain Node, SSH-hosted sessions.

## Findings

| # | Question | Answer | Evidence |
|---|----------|--------|----------|
| 1 | Do project PreToolUse hooks fire in chat sessions? | **Yes.** | Step 2: asked for `hello.txt`; the `ChatPreToolUse` log held `1 "tool_name":"Write"` and the file was created. Code: `acp-agent.js` line 6377 `settingSources: ["user","project","local"]`. |
| 2 | Does a block (exit 2) reach the chat agent and client? | **Yes.** The Edit tool call came back to the ACP client as `tool_call_update` with `status:"failed"` and text `PreToolUse:Edit hook error: [/tmp/pragma-s0c/block-edit.sh]: pragma blocked this edit. Rule (NOTES.md:1): ... Fix: write three.` The agent quoted it verbatim and `notes.txt` stayed `one`. | Step 3, default mode: log showed 1 Write, 1 Edit; `permission_mode":"default"`. |
| 3 | Does the agent write `three` on its own? | **No.** As in S0c, it declined because that contradicted the user's request (my prompt also said not to retry). A real fix request must be consistent with the task. | Same run. |
| 4 | Do hooks fire in bypass-permissions mode? | **Yes**, identically. The hook payload carried `"permission_mode":"bypassPermissions"`, the Edit was blocked, the file stayed `one`, the refusal reached the client the same way. | Step 4: `setSessionMode` returned `{}`; log showed 1 Write, 1 Edit, both `bypassPermissions`. |
| 5 | Is `requestPermission` called for edits? | **Default mode: yes** (a `permission_request` of kind `edit` for the Write; for the Edit the hook denied before any prompt). **Bypass mode: no** permission request was sent at all for either tool. So the permission path cannot see edits in bypass mode, and hooks can. | Step 2 and 3 event logs (permission_request present in default, absent in bypass). Adapter comment at `acp-agent.js` ~5601: Claude Code applies bypass before `canUseTool`, so only bypass-immune checks reach the client. |
| 6 | Mode list offered by the adapter | `default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions` (bypass is clamped off for root outside a sandbox). | `newSession` response. |

## Recommendation

PreToolUse hooks fire in chat sessions, in every permission mode including bypass, and their stderr refusal
reaches both the agent and the ACP client, so **chat support needs no new interception path: the S1 Claude Code
hooks cover it.** The slice that adds chat support should only (a) make sure the hook entries are installed in the
project or worktree `.claude/settings.local.json` before an ACP session starts (the adapter reads
`user`, `project` and `local` settings, so the same install used for TUI sessions works), and (b) show the
failed tool call text in the chat transcript with the violation visible (it arrives as a failed tool call whose
content is the hook stderr). Do **not** use `requestPermission` in `cell.ts` as the interception point: it is not
called for edits in bypass mode, it fires only when the user would be prompted anyway, and it would double-handle
edits already covered by hooks. It can still serve as a secondary signal in default mode. Remaining to confirm in
the real app: rendering of the failed call in the chat UI and the Electron-as-Node launch path.

# Vector OS agent environment

Lets AI coding agents (Claude Code, Codex, Antigravity) do real desktop work
on your Linux machine (open apps, click through them, browse logged-in
websites, run test instances) while you keep working. Nothing they do
appears on your screen, touches your clipboard or notifications, or deletes
your files. When several agents run at once, they can see what each other is
doing and don't trample each other's work.

Built for Hyprland and Claude Code; parts work with any agent that runs shell
commands.

## What's in it

| Piece | What it does |
|---|---|
| **AI workspace** (`aiws`) | Workspace 0 lives on an invisible headless output. Agents open, drive and screenshot windows there without ever taking your focus. Press SUPER+0 to watch it on your screen and SUPER+0 again to leave. Every agent runs with `AI_WORKSPACE=1`, so a daemon moves any window an agent opens to workspace 0, even when the agent forgets. |
| **Nested compositor** (`aiws nest`) | A second Hyprland in a window on workspace 0 with its own pointer, keyboard, focus and clipboard: real clicks, drags, typing and copy/paste, fully isolated. Starts on use, stops itself when idle, costs nothing while idle. |
| **AI browser** (`aiws browser`) | One Chromium on workspace 0 with its own persistent profile. You log in to the sites agents may use once, and the logins stay. Websites see a normal browser rather than an automated one, so sites that block bots work. Driven over CDP by a dedicated Playwright MCP server (`ai-browser`), so it works for every Claude account and any agent. Closes itself when only empty tabs are left. |
| **Sandboxes and test files** | `aiws env` gives a test instance its own XDG dirs and D-Bus session. `aiws make jpg:12 clip.mp4 report.pdf tree` generates synthetic images, video, audio, office documents, archives and folder trees, so tests never use your own files. |
| **crew** | Shows what every agent is *actually* doing, from Claude Code's session state and the tool calls in each transcript: working or idle, last actions, files edited (also through the shell), sub-agents, Codex and Antigravity processes. Hooks stop an agent once before it edits a file another agent is working on, or before a git command would sweep up or discard someone else's uncommitted work. Repeating the same action goes through. Includes messages, locks, groups and `crew watch` to follow an agent live. |
| **aiws-guard** | A Claude Code hook that blocks reading or writing your real clipboard, reading your notification history, popping things up on your screen, reading browser cookies and logins, deleting or overwriting your personal folders and keys, and a few classic foot-guns (`pkill -f` killing its own shell). Every block says what to use instead. When you really asked for exactly that, the agent adds `# owner-asked`. |
| **aisudo** | The root commands agents may run without a password: services, logs, official package installs, reading non-secret system files, diagnostics. The allowlist is a root-owned script that agents can't change. Anything else still needs you. |
| **Skills** | `ai-workspace`, `ai-browser` and `crew` teach agents when and how to use all of the above, so they get it right without being told. |

## Checking on your agents from anywhere

Everything works from any shell, including over SSH from your phone, with no
extra Claude session and without interrupting the agent:

```sh
crew                      # every agent: working or idle, task, last actions, files, sub-agents
crew show NAME            # status report: what you last asked, its plan, what it said, what it did
crew watch NAME           # follow it live
crew ask NAME "how far are you with the deploy?"   # a real answer from the model, waits for it
crew send NAME "also update the README"            # an instruction, no waiting
```

`show` and `watch` read the agent's transcript, so they're instant and cost
nothing. `ask` and `send` reach the agent at its next step (or wake it when
idle). They arrive marked as coming from you, the owner, so it treats them as
your words and answers with `crew reply`.

## Install

```sh
git clone https://github.com/Not-Bad-Company/vector
cd vector/agents && ./install.sh
sudo ~/.local/bin/aisudo-install      # optional: allowlisted root commands
```

Then paste [`AGENTS.md`](AGENTS.md) into `~/.claude/CLAUDE.md` (and
`~/.codex/AGENTS.md`, `~/.gemini/GEMINI.md`) and restart your agent sessions.
`./install.sh --remove` takes the hooks, service and skills out again.

**Needs:**
- Hyprland, Python 3, grim, wtype, Chromium, dbus-daemon, and npx (for the browser MCP).
- For `aiws make`: ImageMagick, ffmpeg, pandoc, LibreOffice, zip, 7z.
- `SUPER+0` is rebound to show the AI workspace (Hyprland workspace 10). See `config/hypr/ai-workspace.conf`.

## When something is blocked

- Each block message says why and what to use instead. Usually the agent just
  switches to the right tool.
- If you really want the blocked action, tell the agent so ("yes, copy it to my
  clipboard"). It adds `# owner-asked` to that command, and that command goes through.
- crew's clash checks never block for good: the same action a second time goes through.
- To switch the guard off entirely, remove the `aiws-guard` entry from
  `~/.claude*/settings.json` (or run `./install.sh --remove`).

## How it works

- `bin/aiws`: one Python file. It talks to Hyprland's socket, creates the
  headless output `AI`, keeps workspace 10 there, keeps focus off it, and claims
  windows from processes with `AI_WORKSPACE=1` in their environment.
- `libexec/aiws-pointer`: a dependency-free `zwlr_virtual_pointer_v1` client,
  used for mouse input into the nested compositor.
- `bin/crew`: reads `~/.claude*/sessions/*.json` and transcripts. It stores
  messages and locks in SQLite and wires Claude Code hooks (SessionStart,
  PreToolUse, PostToolUse, Stop).
- `libexec/aiws-guard`: a PreToolUse hook that reads the tool call as JSON and
  exits 2 with a reason to block.

Part of [Vector OS](../README.md). MIT licensed, like the rest of the repo.

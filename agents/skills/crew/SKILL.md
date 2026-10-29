---
name: crew
description: See and coordinate with the other AI agents on this laptop (other Claude sessions and accounts, sub-agents, Codex, Antigravity). Use when a crew hook warns you about a clash, when you need something from another agent, before claiming what another agent is doing, when you must share an exclusive resource (GPU, a port, the editor), or to watch what a sub-agent is doing.
---

# crew

Several agents often work on this laptop at once. crew shows what each one is
*actually* doing, read from Claude Code's own session state and the tool calls
in each transcript, and stops clashes automatically. You rarely need to call it.

## What happens by itself (hooks)

- **Clashing edit:** you edit a file another live agent edited in the last 30
  minutes, by Edit/Write or through Bash. The edit is stopped once, with who and
  what. Then either work on something else, ask them, or repeat the edit if
  you're sure (their change is done, or you agreed). The repeat goes through.
- **Clashing git:** in a repo where another agent has uncommitted work, these
  are stopped once: `git add -A` / `add .` / `commit -a`, `stash`,
  `reset --hard`, `checkout -- .`, `restore`, `clean -f`, branch switches,
  rebase and force-push. Stage your own files by path instead.
- **Messages:** messages from agents arrive in your context by themselves, mid-turn or when idle.
- **Same repo:** you're told once when another agent starts editing in your repo.

## When to use the commands

| Situation | Do |
|---|---|
| Need to know what others do (before saying anything about them) | `crew who` (`-v` for more), `crew show NAME` |
| Need an answer from another agent | `crew ask NAME "question"` (waits for the reply) |
| Must tell an agent something that changes its work | `crew send NAME "..."`, add `--urgent` only if it must stop now |
| Exclusive resource (GPU, port 3000, a long build, the AI browser) | `crew lock NAME -r why [--wait 3600]`, then `crew unlock NAME` |
| Following your sub-agent live | `crew watch me --subagents-only` (or `crew watch NAME`) |
| A message arrived and they asked something | `crew reply ID "answer"` |

## Rules

- Never message to announce, report progress, say "done", or acknowledge. A
  message costs the other agent attention. Send one only when you need
  something or your action affects their work.
- Never say another agent is working on X unless `crew who` shows it right now.
  Session titles can be old; the listed actions, edited files and working/idle state are facts.
- Prefer avoiding a clash (another file, another time) over messaging about it.
- Sub-agents: `crew send parent --as LABEL "..."` reaches the agent that started
  you; `crew inbox --as LABEL` reads instructions sent to you.

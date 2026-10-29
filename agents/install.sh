#!/bin/sh
# Install the Vector OS agent environment for this user:
#   aiws (AI workspace, AI browser, nested compositor, test files), crew (agents
#   see each other, clash checks), aiws-guard (protects the owner's screen,
#   clipboard, notifications, private data and personal files), aisudo
#   (allowlisted root commands), and the skills that teach agents to use them.
#
#   ./install.sh            install / update
#   ./install.sh --remove   take the hooks, service and skills out again
#
# Needs: Hyprland, Python 3, grim, wtype, chromium, dbus-daemon, npx (for the
# ai-browser MCP server); for `aiws make`: ImageMagick, ffmpeg, pandoc,
# LibreOffice, zip, 7z. Claude Code is what the hooks and skills are for.
set -eu
cd "$(dirname "$0")"
B=$HOME/.local/bin L=$HOME/.local/libexec S=$HOME/.local/share/aiws C=$HOME/.config

if [ "${1:-}" = "--remove" ]; then
    "$B/crew" install-hooks --remove || true
    python3 - <<'PY'
import glob, json, os
for p in glob.glob(os.path.expanduser("~/.claude*/settings.json")):
    d = json.load(open(p))
    hooks = d.get("hooks", {})
    pre = [g for g in hooks.get("PreToolUse", [])
           if not any("aiws-guard" in h.get("command", "") for h in g.get("hooks", []))]
    if pre:
        hooks["PreToolUse"] = pre
    else:
        hooks.pop("PreToolUse", None)
    json.dump(d, open(p, "w"), indent=2)
PY
    systemctl --user disable --now aiws.service || true
    for d in "$HOME"/.claude* "$HOME"/.codex*; do
        rm -f "$d/skills/ai-workspace" "$d/skills/ai-browser" "$d/skills/crew"
    done
    echo "removed hooks, service and skills (the programs stay in ~/.local/bin)"
    exit 0
fi

install -Dm755 bin/aiws "$B/aiws"
install -Dm755 bin/crew "$B/crew"
install -Dm755 bin/aisudo "$B/aisudo"
install -Dm755 bin/aisudo-install "$B/aisudo-install"
install -Dm755 libexec/aiws-guard "$L/aiws-guard"
install -Dm755 libexec/aiws-pointer "$L/aiws-pointer"
install -Dm644 share/aisudo.py "$S/aisudo.py"
install -Dm644 config/aiws/nest.conf "$C/aiws/nest.conf"
install -Dm644 config/hypr/ai-workspace.conf "$C/hypr/ai-workspace.conf"
install -Dm644 systemd/aiws.service "$C/systemd/user/aiws.service"
for s in ai-workspace ai-browser crew; do
    install -Dm644 "skills/$s/SKILL.md" "$S/skills/$s/SKILL.md"
done

# Hyprland: the AI output, workspace 0 and SUPER+0
grep -q "ai-workspace.conf" "$C/hypr/hyprland.conf" 2>/dev/null ||
    printf '\n# Vector OS agent environment (aiws)\nsource = ~/.config/hypr/ai-workspace.conf\n' >> "$C/hypr/hyprland.conf"
systemctl --user daemon-reload
systemctl --user enable --now aiws.service

# Claude Code: hooks, the ai-browser MCP server, AI_WORKSPACE=1, skills
"$B/crew" install-hooks
python3 - <<'PY'
import glob, json, os
guard = os.path.expanduser("~/.local/libexec/aiws-guard")
for p in glob.glob(os.path.expanduser("~/.claude*/settings.json")):
    d = json.load(open(p))
    pre = d.setdefault("hooks", {}).setdefault("PreToolUse", [])
    if not any("aiws-guard" in h.get("command", "") for g in pre for h in g.get("hooks", [])):
        pre.insert(0, {"matcher": "Bash|Read|Grep|Glob|Edit|Write|NotebookEdit|mcp__claude-in-chrome__.*",
                       "hooks": [{"type": "command", "command": guard, "timeout": 10}]})
    d.setdefault("env", {})["AI_WORKSPACE"] = "1"
    json.dump(d, open(p, "w"), indent=2)
    print("hooks and env in", p)
for p in glob.glob(os.path.expanduser("~/.claude*.json")) + glob.glob(os.path.expanduser("~/.claude*/.claude.json")):
    try:
        d = json.load(open(p))
    except ValueError:
        continue
    d.setdefault("mcpServers", {})["ai-browser"] = {
        "type": "stdio", "command": "npx",
        "args": ["-y", "@playwright/mcp@latest", "--cdp-endpoint", "http://127.0.0.1:9333"], "env": {}}
    json.dump(d, open(p, "w"), indent=2)
    print("ai-browser MCP server in", p)
PY
for d in "$HOME"/.claude* "$HOME"/.codex*; do
    [ -d "$d" ] || continue
    mkdir -p "$d/skills"
    for s in ai-workspace ai-browser crew; do ln -sfn "$S/skills/$s" "$d/skills/$s"; done
done

cat <<MSG

Installed. Left for you:
1. Paste the rules in $(pwd)/AGENTS.md into ~/.claude/CLAUDE.md (and
   ~/.codex/AGENTS.md, ~/.gemini/GEMINI.md) so every agent follows them.
2. Optional, for allowlisted root commands: sudo ~/.local/bin/aisudo-install
3. Restart your Claude Code sessions. SUPER+0 shows the AI workspace; log into
   the sites agents may use in the AI browser there (aiws browser) once.
MSG

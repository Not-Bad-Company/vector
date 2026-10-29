import json
import os
import subprocess
import sys

GUARD = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "libexec", "aiws-guard")
H = os.path.expanduser("~")
cases = [
    # (expected exit, tool, key, value, label)
    # still blocked
    (2, "Bash", "command", "wl-paste", "read clipboard"),
    (2, "Bash", "command", "echo hi | wl-copy", "write clipboard"),
    (2, "Bash", "command", "makoctl history", "notification history"),
    (2, "Bash", "command", "notify-send hi", "notify-send"),
    (2, "Bash", "command", "sleep 1 && notify-send done", "notify-send after &&"),
    (2, "Bash", "command", "xdg-open x.pdf", "xdg-open"),
    (2, "Bash", "command", "hyprctl dispatch exec kitty", "exec"),
    (2, "Bash", "command", "hyprctl dispatch workspace 3", "ws switch"),
    (2, "Bash", "command", "hyprctl --batch 'dispatch focusmonitor eDP-1'", "batch focus"),
    (2, "Bash", "command", "cat ~/.zen/abc/cookies.sqlite", "cookies"),
    (2, "Bash", "command", "sqlite3 ~/.config/chromium/Default/Cookies 'select 1'", "chromium cookies"),
    (2, "Bash", "command", "pkill -f gsc_one.js", "pkill self"),
    (2, "Bash", "command", f"sed -i s/a/b/ {H}/.local/share/aiws/aisudo.py", "sed allowlist"),
    (2, "Bash", "command", f"echo x > {H}/.local/bin/aisudo", "redirect allowlist"),
    (2, "Bash", "command", f"cp evil.py {H}/.local/share/aiws/aisudo.py", "cp allowlist"),
    (2, "Bash", "command", "rm ~/Pictures/x.jpg", "rm personal"),
    (2, "Bash", "command", "rm -rf ~/.config", "rm top-level"),
    (2, "Bash", "command", "busctl --user call org.freedesktop.Notifications /org/freedesktop/Notifications x", "busctl notes"),
    (2, "Edit", "file_path", f"{H}/.local/bin/aisudo-install", "edit allowlist"),
    (2, "Read", "file_path", f"{H}/.local/state/vector-bar/notifications.json", "read notes file"),
    # no longer false positives
    (0, "Bash", "command", f"grep -n x {H}/.local/bin/aisudo {H}/.local/bin/aisudo-install", "grep allowlist files"),
    (0, "Bash", "command", "grep -n -E 'notified|notify-send' crates/vector-bar/src/modules/ai.rs", "grep notify-send"),
    (0, "Bash", "command", "rg 'wl-copy' src/", "rg wl-copy"),
    (0, "Bash", "command", "git commit -m 'stop xdg-open and notify-send popping up'", "commit message"),
    (0, "Bash", "command", "python3 - <<'EOF'\nprint('notify-send')\nos.system('true')\nEOF", "heredoc text"),
    (0, "Bash", "command", "cat > x.sh <<'EOF'\nnotify-send hi\nEOF\nchmod +x x.sh", "heredoc script body"),
    (0, "Bash", "command", "echo 'run hyprctl dispatch workspace 2 to switch'", "echo text"),
    (0, "Bash", "command", "crew send foo 'please stop using wl-paste'", "crew message"),
    # normal work
    (0, "Bash", "command", "aisudo systemctl status x", "aisudo use"),
    (0, "Bash", "command", f"cat {H}/.local/share/aiws/aisudo.py", "read allowlist"),
    (0, "Bash", "command", "cargo build --release", "build"),
    (0, "Bash", "command", "hyprctl dispatch exec '[workspace 10 silent] kitty'", "exec to ws10"),
    (0, "Bash", "command", "eval \"$(aiws env)\"; notify-send test", "private bus"),
    (0, "Bash", "command", "aiws nest run wl-copy hi", "nested clipboard"),
    (0, "Bash", "command", "pkill -f '[g]sc_one.js'", "pkill bracket"),
    (0, "Bash", "command", "ls ~/Pictures", "list personal"),
    (0, "Bash", "command", "rm -rf ~/.local/src/vector/target", "rm build dir"),
    (0, "Bash", "command", "cat ~/.zen/abc/chrome/userChrome.css", "userChrome"),
    (0, "Read", "file_path", f"{H}/.local/share/aiws/aisudo.py", "read tool allowlist"),
]
bad = 0
for want, tool, key, val, label in cases:
    ev = {"tool_name": tool, "cwd": H, "tool_input": {key: val}}
    r = subprocess.run([GUARD], input=json.dumps(ev), capture_output=True, text=True)
    if r.returncode != want:
        bad += 1
        print(f"WRONG {label}: want {want} got {r.returncode} {r.stderr[:120]}")
print(f"{len(cases) - bad}/{len(cases)} right")

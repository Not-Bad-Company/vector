#!/usr/bin/python3 -I
"""aisudo: the root commands AI agents may run without a password.

Installed root-owned as /usr/local/libexec/aisudo with a sudoers rule that lets
the owner's user run only this file (aisudo-install). Agents call `aisudo CMD
ARGS...` (~/.local/bin/aisudo runs `sudo -n` on this file). Every argument is
checked against the allowlist below; anything else is refused. The command
then runs directly (no shell). Each run is logged to the journal (tag aisudo).

Allowed:
  systemctl  status/show/cat/list-*/is-*, start/stop/restart/reload/enable/disable,
             daemon-reload, reset-failed (not for login, display manager, D-Bus,
             journald, udev, polkit or user@ units)
  journalctl reading only (no vacuum, rotate, flush)
  dmesg      reading only
  pacman     -S/-Su/-Syu (--needed, --noconfirm) from the configured repos;
             -Q*, -Si, -Ss, -Sl, -F*: never -R, -U, -D or options that change paths
  read       cat, head, tail, ls, stat, wc, file, grep, find (no -exec/-delete)
             under /etc, /var/log, /var/lib/pacman, /usr, /opt, /boot, /proc, /sys,
             except secrets: shadow, sudoers, SSH host keys, Wi-Fi and VPN
             passwords, crypttab
  diagnose   ss, lsof, smartctl -a/-H/-i/-x, btrfs filesystem/subvolume/device
             reading, du, nft list, resolvectl status/query/statistics/flush-caches
"""
import os
import re
import subprocess
import sys

UNIT = re.compile(r"^[A-Za-z0-9@._:-]+$")
PROTECTED_UNITS = re.compile(r"^(systemd-logind|dbus(-broker)?|sddm|gdm|greetd|ly|lightdm|systemd-journald|"
                             r"systemd-udevd|polkit|user@.*|getty@.*|sudo.*)(\.service)?$")
READ_ROOTS = ("/etc", "/var/log", "/var/lib/pacman", "/usr", "/opt", "/boot", "/proc", "/sys")
SECRETS = re.compile(r"^/etc/(shadow|gshadow|sudoers|crypttab|security/opasswd)|^/etc/ssh/.*key|"
                     r"^/etc/NetworkManager/system-connections|^/var/lib/iwd|^/etc/wireguard|"
                     r"^/var/lib/bluetooth|^/etc/(ppp|ipsec)|^/proc/(kcore|kmsg|\d+/(environ|mem))")


def refuse(why):
    sys.exit(f"aisudo: refused: {why}. Run `aisudo --help` for what is allowed; anything else needs the owner.")


def path_ok(p):
    rp = os.path.realpath(p)
    if not rp.startswith(READ_ROOTS) or SECRETS.search(rp):
        refuse(f"{p} is outside the readable system dirs or holds secrets")


def check(argv):
    if not argv:
        refuse("no command")
    prog, args = argv[0], argv[1:]
    opts = [a for a in args if a.startswith("-")]
    plain = [a for a in args if not a.startswith("-")]
    if prog == "systemctl":
        read = {"status", "show", "cat", "list-units", "list-unit-files", "list-timers", "list-dependencies",
                "is-active", "is-enabled", "is-failed"}
        write = {"start", "stop", "restart", "reload", "try-restart", "reload-or-restart", "enable", "disable",
                 "daemon-reload", "reset-failed"}
        if not plain or plain[0] not in read | write:
            refuse("systemctl action not allowed")
        for o in opts:
            if o not in ("--now", "--no-pager", "-l", "--full", "--all", "-a", "--failed") \
                    and not o.startswith(("--type=", "--state=", "-n", "--lines=")):
                refuse(f"systemctl option {o}")
        for u in plain[1:]:
            if not UNIT.match(u):
                refuse(f"unit name {u!r}")
            if plain[0] in write and PROTECTED_UNITS.match(u):
                refuse(f"{u} keeps the session or the system running")
    elif prog == "journalctl":
        bad = ("--vacuum", "--rotate", "--flush", "--relinquish", "--sync", "--setup-keys", "--update-catalog",
               "--smart-relinquish")
        if any(a.startswith(bad) for a in args):
            refuse("journalctl may only read")
    elif prog == "dmesg":
        if any(a in ("-C", "--clear", "-c", "--read-clear", "-D", "--console-off", "-E", "--console-on", "-n",
                     "--console-level") or a.startswith("--console-level") for a in args):
            refuse("dmesg may only read")
    elif prog == "pacman":
        if not opts:
            refuse("pacman needs an operation")
        op = opts[0]
        if re.fullmatch(r"-S(y{0,2}u|u)?", op) or op == "-S":
            if op.startswith("-Sy") and "u" not in op:
                refuse("-Sy without -u (partial upgrades break the system)")
            for o in opts[1:]:
                if o not in ("--needed", "--noconfirm", "--noprogressbar"):
                    refuse(f"pacman option {o}")
            for pkg in plain:
                if not re.fullmatch(r"[a-z0-9@._+\-]+(/[a-z0-9@._+\-]+)?", pkg):
                    refuse(f"package name {pkg!r}")
        elif re.fullmatch(r"-(Q[a-z]*|Si|Ss|Sl|Sii|F[a-z]*)", op):
            for o in opts[1:]:
                if o.startswith(("--dbpath", "--root", "--config", "--cachedir", "--gpgdir", "--hookdir")):
                    refuse(f"pacman option {o}")
        else:
            refuse(f"pacman {op} (only installs, upgrades and queries)")
    elif prog in ("cat", "head", "tail", "ls", "stat", "wc", "file", "grep", "find"):
        if prog == "find" and any(a in ("-exec", "-execdir", "-delete", "-ok", "-okdir", "-fprint", "-fprintf",
                                        "-fls", "-fprint0") for a in args):
            refuse("find may only list")
        if prog == "grep":
            rest, i = [], 0
            while i < len(args):  # skip the pattern
                a = args[i]
                if a in ("-e", "-f", "--regexp", "--file"):
                    if a in ("-f", "--file"):
                        path_ok(args[i + 1])
                    i += 2
                    continue
                rest.append(a)
                i += 1
            paths = [a for a in rest if not a.startswith("-")][0 if "-e" in args or "--regexp" in args else 1:]
        elif prog in ("head", "tail"):
            paths = [a for a, prev in zip(args, [""] + args) if not a.startswith("-") and prev not in ("-n", "-c")]
        elif prog == "find":
            paths = [a for a in args if not a.startswith("-")][:1] or ["."]
        else:
            paths = plain
        if not paths:
            refuse(f"{prog} needs explicit paths")
        for p in paths:
            path_ok(p)
    elif prog == "ss" or prog == "lsof":
        pass  # read-only listings
    elif prog == "smartctl":
        if not any(o in ("-a", "-H", "-i", "-x", "--all", "--health", "--info", "--xall") for o in opts) or \
                any(o.startswith(("-t", "--test", "-s", "--smart", "-o", "-S", "--set")) for o in opts):
            refuse("smartctl may only read")
    elif prog == "btrfs":
        if not (len(plain) >= 2 and plain[0] in ("filesystem", "fi", "subvolume", "sub", "device", "dev") and
                plain[1] in ("usage", "df", "show", "list", "stats", "du")):
            refuse("btrfs may only report")
    elif prog == "du":
        pass
    elif prog == "nft":
        if plain[:1] != ["list"]:
            refuse("nft may only list")
    elif prog == "resolvectl":
        if plain[:1] and plain[0] not in ("status", "query", "statistics", "flush-caches", "dns", "domain"):
            refuse("resolvectl action not allowed")
        if plain[:1] in (["dns"], ["domain"]) and len(plain) > 2:
            refuse("resolvectl may not change DNS settings")
    else:
        refuse(f"{prog} is not on the list")


def main():
    argv = sys.argv[1:]
    if argv in ([], ["-h"], ["--help"]):
        print(__doc__.strip())
        return
    check(argv)
    who = os.environ.get("SUDO_USER", "?")
    subprocess.run(["logger", "-t", "aisudo", f"{who}: {' '.join(argv)}"])
    env = {"PATH": "/usr/bin:/usr/sbin", "LANG": os.environ.get("LANG", "C.UTF-8"), "TERM": "dumb"}
    os.execve(f"/usr/bin/{argv[0]}", argv, env)


if __name__ == "__main__":
    main()

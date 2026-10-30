# vectorOS system commands

`vector` (also installed as `vector-update`, `vector-install`, `vector-remove`,
`vector-setup`, `vector-about`, `vector-version`, `vector-lock`,
`vector-logout`, `vector-restart`, `vector-shutdown`): the system actions
behind the Start menu. Menus open in Vector Space's picker (`vector-space
dmenu`); anything that needs a password or shows progress runs in a floating
terminal (`org.vectoros.terminal`).

`share/` holds the brand: `logo.txt` (the two-tone `vectorOS` wordmark for
fastfetch, `$1` accent and `$2` light) and `wordmark.txt` (one colour, for the
screensaver). Made with figlet's `mono12` font.

Install: `os/install.sh` (also writes `~/.local/share/vectoros/version`).

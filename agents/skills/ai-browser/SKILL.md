---
name: ai-browser
description: Use for ANY web browsing or browser automation - opening a website, reading a page, logging-in-required sites (Instagram, Outlook, Canvas, GitHub web...), filling forms, clicking through a web app, screenshots of a site, testing a local dev server in a browser. Use the AI browser (`aiws browser` + the ai-browser MCP tools), not Claude in Chrome and not a browser on the owner's screen.
---

# AI browser

One Chromium for all agents and accounts, on the AI workspace (workspace 0),
with its own persistent profile at `~/.local/share/aiws/browser`. The owner
logs into sites there once (SUPER+0 shows it) and the logins stay. To websites
it looks like a normal browser (`navigator.webdriver` false, normal user
agent), so sites that block automation, like Instagram, work. It replaces
Claude in Chrome. Don't use the claude-in-chrome tools, and don't open URLs in
the owner's own browsers.

## Start it first

```sh
aiws browser                 # starts it if needed (idempotent); prints the CDP URL
aiws browser https://x.y     # also opens that URL in a new tab
```

## Drive it

- **The `ai-browser` MCP tools** (`mcp__ai-browser__browser_navigate`,
  `browser_snapshot`, `browser_click`, `browser_type`, `browser_take_screenshot`,
  `browser_tabs`, ...) are Playwright MCP pinned to it (`--cdp-endpoint
  http://127.0.0.1:9333`). They act in its default context, so they have the
  owner's AI-browser logins. (The generic Playwright plugin is off on purpose.)
  Clicks go over CDP, so the owner's mouse never moves. If they error with
  "connect ECONNREFUSED", run `aiws browser` and retry.
- **Scripts:** `chromium.connectOverCDP("http://127.0.0.1:9333")`, then
  `browser.contexts()[0]`. Close only the pages you opened, never `browser.close()` on
  a launched-by-you browser here (connectOverCDP's close just disconnects).
- **Screenshots:** the MCP/Playwright screenshot, or `aiws shot` for the whole workspace.

## Rules

- Only visit what the task needs. The logins are the owner's accounts: never
  post, buy, send or change account settings unless the owner asked for exactly that.
- Several Google accounts can be signed in at once. Google pages pick one with
  `/u/N/` in the path or `?authuser=N` (N = 0, 1, 2...). Before concluding "no
  access" anywhere on Google (Search Console, Analytics, Drive...), try every
  signed-in account. Search Console properties can be domain properties
  (`resource_id=sc-domain:example.com`) or URL-prefix ones (`https://example.com/`).
- If a site needs a login that isn't there, ask the owner to log in via
  SUPER+0. Never type their password and never copy cookies from their own browsers.
- Plain checks without logins (a public page, local dev server screenshots,
  performance) may also use a throwaway headless Playwright browser
  (`headless: true`). That keeps the AI browser's tabs tidy.
- Several agents share this browser: open your own tab and close it when done.
  Don't close or navigate other tabs. When no other agent uses it (`crew who`),
  close the browser itself when you finish: `aiws close <its address>` (logins
  stay in the profile). As a fallback it closes by itself after two minutes
  with only empty tabs.

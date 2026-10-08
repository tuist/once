---
title: Terminal status, live links, and smoother progress
date: 2026-10-08
---

Once now reports whether a build, test, lint, or run command is working,
finished, or failed to supporting terminals. Terminals can display that state
without interpreting spinner characters or command output, and status reporting
works without a reporting account or network connection.

Live dashboard links are now clickable in interactive terminals and appear as
soon as the reporting service returns the run's URL, so you can open the dashboard
while work is still running. Links and log messages stay coordinated with the
progress panel, and synchronized redraws reduce flicker on supporting terminals.

Terminal integration uses portable protocols and is enabled automatically for
interactive human output. Machine-output formats, redirected stderr, dumb
terminals, and CI do not receive these controls. Disable them explicitly with
`--terminal-controls=never`, independently of your color settings.

See [Terminal integration](/docs/guide/local-execution/terminal-controls) for
quiet-mode behavior, cancellation, and output compatibility details.

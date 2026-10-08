# Terminal integration

Once reports build, test, lint, and run status to supporting terminals using the
[Program Status Protocol (OSC 7501)](https://www.superlogical.com/rex/docs/build/program-status).
A terminal can show that work is running, finished, failed, or cancelled without
trying to interpret progress spinners or command output. The terminal decides
whether to display a tab indicator, a notification, or another status view.

Status reporting works without a reporting account or network connection. Once
reports preparation as working, success as done, and failure as error. When the
existing connected-run cancellation handler processes a signal, Once reports
idle before exiting by that signal. Unconnected runs retain the default signal
behavior; terminals clear transient working status when they observe process
exit or the next shell prompt.

Once does not guess progress percentages or report scheduler and memory waits
as requests for user input. Literal commands executed through `once exec` do not
receive Once's own program-status reports, so captured child status sequences
cannot be confused with Once's execution state.

## Open a live run

When a reporting service creates a build, test, or script run and returns its
dashboard URL, Once prints the link immediately:

```text
  ↗ Once live: https://dashboard.example/runs/123
```

On an interactive terminal, the URL is an
[OSC 8 hyperlink](https://ghostty.org/docs/vt/osc/8). In redirected logs it remains
plain text. Once uses the canonical URL supplied by the service, rather than
constructing a provider-specific route, and prints it only once across reconnects.
The link remains above the progress panel instead of being overwritten by redraws.
Captured output shown by the build and test progress reporter is rendered without
terminal escapes so child output cannot replace Once's status or leave a hyperlink
or synchronized frame open. Original captured output remains unchanged in the cache.
Application output from `once run` and literal output from `once exec` retain their
existing pass-through behavior.

No extra wait is added for the link. For a fast cached run, it can arrive after
the completion summary while reporting drains. `--quiet` suppresses the dashboard
announcement. Machine-output modes retain the existing plain-text link on stderr;
they never receive hyperlink escape sequences from Once.

## Synchronized redraws

Once uses DEC private mode 2026 to present each progress-panel redraw as one
frame on supporting terminals. Synchronization covers only the redraw, never
execution or a wait for work. Frames use a bounded buffer, with ordinary output
as the fallback when a frame exceeds that bound.

## Disable terminal controls

Terminal controls are automatic for human output on an interactive stderr
terminal. They are disabled for redirected output, machine-output formats,
`TERM=dumb`, and detected CI environments. No terminal-brand environment variable
or interactive capability query is required. Unsupported terminals ignore the
protocol sequences. On Windows, controls also require successful virtual-terminal
initialization.

To disable status reports, hyperlinks, and synchronized redraws:

```sh
once --terminal-controls=never build App
```

The default is `--terminal-controls=auto`. This setting is independent of color:
`--color=never` and `NO_COLOR` disable colors, not terminal integration.
`--quiet` suppresses the progress panel and dashboard announcement but retains
invisible program-status reports.

# oriel-mac

macOS implementation of `oriel_core::Platform`. Accessibility (AX) moves and raises real app
windows. NSWorkspace launches, hides and quits apps.

## Modules (`src/`)

- `lib.rs`: `MacPlatform`, the public entry point. `send` only queues.
- `worker.rs`: the worker thread, commands, trust polling, `Present` (frame first, then show)
  and its gate (hides and minimizes wait until the presented window is in place).
- `mover.rs`: the frame mover thread: only `SetFrame`, newest first, position-only when the
  size is unchanged, one read-back when a burst settles.
- `shared.rs`: what the worker, the mover and the observer thread share (windows, echo filter,
  targets, bursts), and the moved/resized handler that runs on the observer thread.
- `apps.rs`: app lifetime, window tracking, AX notification handling.
- `observer.rs`: `AXObserver` wrapper and the CFRunLoop thread.
- `ax.rs` / `ffi.rs`: safe `AXUIElement` wrapper and raw bindings.
- `appctl.rs`: NSRunningApplication / NSWorkspace calls.
- `screen.rs`: main-thread notifications (screen, launch, quit, activate).
- `scan.rs`: installed app scan. `frame.rs`: coordinate math, echo suppression, coalescing.
- `msg.rs`: worker messages.

## Probe tool

`cargo run -p oriel-mac --example probe -- <mode>`; every event prints as one line.

- `trust`: print `trusted` / `not trusted`, exit.
- `request-trust`: show the system prompt, print events until Ctrl+C.
- `apps`: print `bundle id<TAB>name<TAB>path` of the installed apps, exit.
- `watch <bundle>...`: print events until Ctrl+C.
- `repl <bundle>...`: watch, and read commands from stdin:
  `launch <path>`, `present <bundle> <window_id> <x> <y> <w> <h>`, `frame <window_id> <x> <y> <w> <h>`,
  `hide <bundle>`, `show <bundle>`, `min <window_id>`, `close <window_id>`, `quit <bundle>`,
  `reopen <bundle>`, plus `watch|unwatch <bundle>`, `scan`, `sleep <seconds>`, `flush` (wait for the queued commands), `exit`.
  Lines starting with `#` are comments. At end of input the probe waits 1 s and exits.

Set `ORIEL_MAC_DEBUG=1` to log every window the worker sees (role, subrole, buttons), and
`ORIEL_MAC_DEBUG_MARKS=1` to print timestamped `oriel-mark` lines for each `Present` gate check
and each message handled while one waits.

Example (needs Accessibility trust for the terminal):

    printf 'launch /System/Applications/TextEdit.app\nsleep 4\n' | \
      cargo run -p oriel-mac --example probe -- repl com.apple.TextEdit

# FerroSeek launcher (`ferroseek-launcher`)

An optional, keyboard-first launcher for applications and FerroSeek file
search on Wayland. It draws a layer-shell overlay, asks the running
`ferroseekd` for file results and launches applications from their
freedesktop desktop entries. It is a separate process: it never opens or
writes the index and adds no background service.

**Status:** experimental. Built and tested in a nested, virtual KWin
(Plasma 6). Other compositors that implement `wlr-layer-shell` should work
but have not been tested. The installer does not manage it yet.

## Build and run

```sh
cargo build --release -p ferroseek-launcher
install -Dm755 target/release/ferroseek-launcher ~/.local/bin/ferroseek-launcher
ferroseek-launcher            # opens; run it again to close it
```

Bind the command to a key yourself, e.g. in KDE System Settings → Keyboard →
Shortcuts → *Add New* → *Command or Script*. The launcher never registers
global shortcuts or changes desktop settings. Running it while it is open
closes the open instance (single instance via the session-bus name
`io.github.ferroseek.Launcher`).

| Option | Effect |
|---|---|
| `--query TEXT` | start with `TEXT` already typed |
| `--private` | do not record anything opened in this session |
| `--forget-history` | delete the launch history and exit |

## Keys

| Key | Action |
|---|---|
| Up/Down, Tab/Shift+Tab, Ctrl+N/Ctrl+P | move the selection |
| Page Up/Page Down, Ctrl+Home/Ctrl+End | move by a page, to the first/last row |
| Enter | launch the application, open the file, copy a calculator result |
| Ctrl+Enter | show the file in its folder |
| Ctrl+K | action panel for the selected row |
| Ctrl+Shift+C | copy the file's path |
| Esc | close the action panel, else clear the query, else close |

## Results

- **Applications** come from `.desktop` files in `$XDG_DATA_HOME` and
  `$XDG_DATA_DIRS`, parsed by the launcher itself (Desktop Entry
  Specification: ID masking, localized names and keywords, `Hidden`,
  `NoDisplay`, `OnlyShowIn`/`NotShowIn`, `TryExec`). Strong name or keyword
  matches are shown above files, weak ones (description only, loose
  subsequence) below them.
- **Files and folders** come from the daemon's `org.kde.krunner1` `Match`
  method; opening and revealing go through its `Run` method, which only
  opens indexed paths. Calling the daemon's bus name D-Bus-activates it.
- **Calculator:** an arithmetic expression (`(2+3)*4`, `sqrt(2)`, `2^10`)
  shows its value; Enter copies it. A fixed grammar is evaluated in process;
  nothing is passed to a shell or interpreter.
- **Empty query:** the most often opened applications and files that still
  exist.

Applications start without a shell, from the parsed `Exec` argument vector,
in their own transient systemd user scope when a user manager is available.
`Terminal=true` entries run in `xdg-terminal-exec` or a known terminal.

## Launch history

`$XDG_STATE_HOME/ferroseek/launcher-usage` (mode `0600`, directory `0700`)
records what was opened and what had been typed, with weights that halve
every 14 days. It is used only to order results:

- Overall use and use from a related query (one is a prefix of the other)
  add a capped boost.
- The boost only reorders matches of the same class: it never lifts a match
  above an exact name, or a weak match above files.
- Updates are a locked read-modify-write, so several instances and
  `--forget-history` do not overwrite each other.

## Design

```text
ferroseek-launcher
 ├─ UI thread: iced (tiny-skia) on a wlr-layer-shell overlay
 │    model.rs (rows, selection, actions, keys) · usage.rs · calc.rs
 └─ bus thread: zbus::block_on for the process lifetime
      ├─ serves io.github.ferroseek.Launcher1.Toggle
      └─ forwards Match/Run to io.github.ferroseek.FerroSeek /runner
```

- zbus's default runtime drives a connection only from inside
  `zbus::block_on`, so one thread stays in it; the UI talks to it through
  channels. Consecutive pending searches are coalesced; every reply carries
  the query generation and outdated ones are dropped. A new query clears the
  previous file rows, and late results keep the selected row.
- The whole stack is Rust (iced, iced_layershell, zbus, freedesktop-icons);
  it links only the system Wayland client libraries. The `.desktop` parser
  is local because the common crate builds C code (gettext).
- `iced_exdevtools` 0.19.1 does not build with `winit-core` beta.4, so
  `winit-core` is pinned to `=0.31.0-beta.2` in the crate manifest.

## Limitations

- No XDG activation token is passed to launched applications:
  `iced_layershell` 0.19 does not expose the layer surface needed to request
  one. Focus of the new window is left to the compositor's policy.
- The daemon's runner shares one query-generation counter between clients:
  if KRunner and the launcher search at the same instant, one of them may
  get an empty reply.
- The window size and position are fixed (720 px wide, near the top of the
  active output); there is no light theme or configuration file yet.

## Tests

```sh
cargo test -p ferroseek-launcher
tests/launcher-smoke.sh target/debug target/launcher-smoke
```

The smoke test runs the launcher on `kwin_wayland --virtual` against a test
`ferroseekd`, with a temporary HOME/XDG tree, a synthetic file corpus and a
private session bus whose activation directory is empty (so neither the
real daemon nor desktop portals can be started). It saves one screenshot per
query and checks that a second invocation closes the first. It skips when
`kwin_wayland` or `spectacle` is missing.

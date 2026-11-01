# Installing FerroSeek

FerroSeek is a Linux launcher with a reusable file search engine. Install as
your normal user, without sudo. The main installation is `--launcher`, which
includes the indexing daemon and CLI. The UI requires Wayland with
`wlr-layer-shell` and session D-Bus; see [tested scope and limits](LAUNCHER.md).
Launcher source builds also need libxkbcommon development files. The engine
and CLI can be installed independently for headless use.
Python 3.11+ is required by the installer; source builds also need Rust 1.89+,
a C compiler/linker and Cargo. SQLite is bundled. `/usr/bin/env` is used for
setting activation environment variables. A fixed `/bin/sh` exec command
receives the executable path as a separate argument, never as shell code. This
also supports `=` in paths, which `env` alone treats as an assignment.
No downloads or remote scripts are executed by the installer.

## Select components

```sh
./scripts/install.sh --launcher     # main product: launcher + daemon + CLI
./scripts/install.sh                # guided setup; launcher recommended on Wayland
./scripts/install.sh --dry-run      # inspect recommended changes; no writes/build
./scripts/install.sh --core         # daemon + CLI, no graphical integration
./scripts/install.sh --krunner      # core + KDE Plasma KRunner registration
./scripts/install.sh --noctalia     # core + copied Noctalia v5 launcher plugin
./scripts/install.sh --mcp          # core + optional MCP executable
./scripts/install.sh --full         # all compatible components, including Noctalia
./scripts/install.sh --krunner --mcp
./scripts/install.sh --core --dry-run
```

Explicit component flags are non-interactive, except optional agent registration.
No flags with redirected input is rejected; scripts must choose components.
Guided setup recommends launcher + core in a Wayland session and core alone
otherwise. Wayland detection does not establish compositor layer-shell support.
Explicit `--launcher` also permits installation for a later graphical session.
KRunner, Noctalia and MCP are optional selections. `--full` includes the launcher
when Wayland is detected, plus compatible integrations. Noctalia supplies its
own optional launcher integration, including on Niri. CLI indexing/search works without a graphical
session or session D-Bus. Core includes optional user systemd/D-Bus activation
files for its control interface; it does not depend on KDE packages or MCP.

The installer reports distribution, architecture, desktop, session D-Bus,
systemd user manager, agent CLI availability and existing configuration.
Noctalia is included in `--full` when stable Noctalia v5+ and GLib
`gio` are available. Explicit `--noctalia` validates those requirements before
installing files; dependencies are not installed automatically. Its managed
plugin copy lives at `$XDG_DATA_HOME/noctalia/plugins/ferroseek` and defaults to
the absolute installed CLI path. See [Noctalia setup and updates](NOCTALIA.md).

Recommendations do not install unimplemented integrations. `--no-build` uses
already built `target/release` executables. A matching binary archive uses its
`bin/` payload automatically and needs no Rust toolchain.

## Configuration and starting

The default engine configuration indexes your home with its normal exclusions.
Choose roots with `ferroseek config` before starting if you want a smaller scope.
`--config /absolute/path/config.toml` installs initial engine configuration only
when no configuration exists; subsequent installs never replace it or an index.

Installation does not start or restart the daemon. The standalone launcher
and KRunner D-Bus-activate it on their first file query. Bind
`ferroseek-launcher` to a shortcut in your desktop settings; the installer does
not register shortcuts or launch the UI. To opt into background indexing now:

```sh
./scripts/install.sh --core --enable
./scripts/install.sh --noctalia --enable  # also activate the installed plugin
```

`--enable` requires an available user systemd manager and session bus. It is an
explicit opt-in to `systemctl --user enable --now ferroseek.service`; an already
running service is not restarted. Without a session bus, `ferroseek index` runs
foreground indexing and `ferroseek search` reads the index locally.
With `--noctalia`, `--enable` also requires a running Noctalia instance. Plugin
activation uses native IPC and preserves other enabled entries. A plain
`--noctalia` install reloads reachable Noctalia and prints the activation command;
it preserves the saved enabled list. A failed post-install reload/enable leaves
the complete installed file set and reports how to retry.

Add `$HOME/.local/bin` to PATH if your distribution does not already do so.
KRunner discovers a new runner at its next restart/login. The installer does
not restart KRunner, change KDE configuration or alter Baloo.

## Optional agent registration

Installing MCP does not edit agent configuration or connect the server automatically.
When launched without a policy, the MCP executable now authorizes already-indexed
paths by default; review existing manual client registrations during upgrades.
To register it explicitly:

```sh
./scripts/install.sh --mcp --register-claude --allow "$HOME/Projects/example"
./scripts/install.sh --mcp --register-codex --allow "$HOME/Projects/example"
# Multiple explicit directories and both clients are supported:
./scripts/install.sh --mcp --register-claude --register-codex \
  --allow "$HOME/Projects/one" --allow "$HOME/Projects/two"
```

For cross-project discovery, the recommended setup is:

```sh
./scripts/install.sh --mcp --register-codex --mcp-access all_indexed
./scripts/install.sh --mcp --register-claude --mcp-access all_indexed
```

You must type `all_indexed` to confirm the authorization and then `yes` for
client configuration edits. Redirected input/EOF never enables it implicitly.
New agent registrations default to `all_indexed` with typed confirmation;
`--allow DIR` without an explicit mode and existing policy files without
`access_mode` preserve the previous restricted behavior. `--mcp-access home` uses indexed HOME
paths without listing directories. `--allow` is for restricted setup only.
Interactive setup explains the global recommendation and pressing Enter
selects `all_indexed`, followed by a mandatory typed confirmation. The engine configuration and running daemon are untouched.
Global authorization includes any future roots that you separately add to the
index; it does not itself add roots. Existing different agent entries are still
refused rather than silently upgraded to broader access.

Each proposed edit displays the file and complete server command and asks you
to type `yes`. Declining/EOF leaves that agent untouched. No filesystem roots are added
by selecting global authorization. Interactive installation can offer detected clients and ask for
a directory in restricted mode. `--dry-run` previews the registration without asking or writing.

Registration creates only the `ferroseek` stdio entry in the user's Claude Code
JSON or Codex TOML configuration. Other settings/servers are retained; Codex
comments are retained. Identical entries are no-ops; differing existing entries
are refused, not overwritten. `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are honored.
MCP exclusions stay in effect. Registration explicitly sets the selected mode
in the client command; an existing allowlist sets scope only in restricted
mode. No policy file is rewritten. MCP
still only reads the existing index and stays off when no client runs it.
See [MCP security and tools](MCP.md).

## Upgrade, ownership and rollback

Binaries go in `$HOME/.local/bin`; registrations use absolute XDG data/config
paths, with the same fallback rules as the application. Installer state is
private (0700) under `$XDG_STATE_HOME/ferroseek/installer`, falling back to
`$HOME/.local/state/ferroseek/installer`. Spaces, Unicode, quotes, backslashes,
dollar/percent signs and other printable path characters are supported.
Control characters and symlink destinations/ancestors are refused before writes.
D-Bus/systemd service search paths cannot reliably represent colons: choose
XDG_DATA_HOME and XDG_CONFIG_HOME without `:`. A colon in the binary/HOME path
is supported when those registration roots are explicitly set elsewhere.
Unsupported registration paths are rejected before writes, not silently installed
with broken activation.
Do not change HOME/XDG roots between installing and uninstalling.

An ownership manifest records installed file hashes. Upgrades replace only
unchanged owned files or byte-identical files. Unrelated files and locally
modified installed files are refused; resolve those conflicts yourself before
retrying. Pre-manifest installations can be adopted only where bytes match;
older differing binaries/registrations need manual backup/removal first. No
heuristic guesses that a conflicting file is safe to overwrite.

Component flags are additive: `--core` updates core while retaining previously
installed MCP/KRunner components. No active daemon is automatically restarted;
it continues using its original executable until the user restarts it later.
Configuration and index data are never replaced during an upgrade.

Files are atomically replaced under a per-installation lock. A durable undo
journal stores originals before mutation. Errors roll back changed files;
after abrupt termination the next modifying invocation recovers first.
Externally changed destinations block rollback instead of overwriting another
writer. Retry with the original HOME/XDG/agent environment; preserve the private
journal when investigating a reported conflict. Empty parent directories may
remain after rollback. No filesystem-wide atomic switch is promised: another
process may see an intermediate file set while installation runs.

Service/bus reloads occur after the file transaction. A failed refresh prints
its retry command; an enable failure leaves a complete installed file set and
reports failure. External service state is not part of file rollback. Agent
configuration edits share the file transaction, but clients concurrently
rewriting their configuration are not synchronized by the installer lock;
close their configuration editors during registration.

## Uninstall

```sh
./scripts/uninstall.sh --dry-run
./scripts/uninstall.sh --launcher    # remove UI only, retain engine/integrations
./scripts/uninstall.sh --mcp         # remove only the optional executable
./scripts/uninstall.sh --krunner     # remove only KDE registration
./scripts/uninstall.sh --noctalia    # disable/remove only the managed plugin
./scripts/uninstall.sh              # remove all managed components
./scripts/uninstall.sh --stop        # explicitly stop/disable an active service too
```

Core cannot be removed while retained integrations require it. Active/enabled
systemd installations require `--stop`; the installer never guesses permission
to stop them. A daemon owning the current session bus outside this systemd unit
must be stopped manually; uninstall refuses removal even with `--stop` in that
case. The ownership check does not activate a daemon. Missing files are harmless and repeat removal is safe. Modified
managed files cause a refusal, and unowned files are never deleted.
Noctalia removal disables the plugin via IPC before deleting its files when
the shell is reachable; a disable failure leaves the files in place. Offline
removal preserves Noctalia's saved settings and enabled list. Other plugin
sources, Noctalia settings, core and index are retained by partial removal.
Configuration, indexes and agent registrations are retained. Remove agent
entries separately with `claude mcp remove --scope user ferroseek` and
`codex mcp remove ferroseek` if desired. The old `--purge` switch is intentionally
unsupported: user data removal is separate from software uninstallation.

## Local distribution archives (no release published)

```sh
python3 scripts/package.py                    # default: launcher + core
python3 scripts/package.py --components launcher
python3 scripts/package.py --components core
python3 scripts/package.py --components krunner
python3 scripts/package.py --components mcp
python3 scripts/package.py --components noctalia
python3 scripts/package.py --components full
# After a workspace release build, add --no-build to reuse its binaries.
```

Archives are native Linux `tar.gz` files under `dist/`, with a SHA-256 sidecar,
`bin/`, component/platform metadata, per-file integrity hashes, installation
scripts/templates, license and installation/MCP documentation.
Launcher/full archives include the standalone UI. All archives include the
launcher guide; Noctalia/full archives also include the plugin payload and Noctalia guide.
Metadata is normalized for reproducibility (`SOURCE_DATE_EPOCH`, default zero). Output files
are never silently overwritten. No upload, GitHub release or AUR dependency.

Before extracting a downloaded archive, authenticate its checksum through the
trusted publication channel, then run `sha256sum -c ARCHIVE.tar.gz.sha256`.
Extract in a fresh directory and run its `scripts/install.sh` with the matching
component flag. A core-only archive cannot satisfy `--mcp` or `--full`; the
installer reports the missing component. Per-file hashes detect corruption,
not an attacker who can replace the archive and checksums together.

Remaining packaging work: a signed release workflow, chosen minimum glibc
baseline, tested distro/architecture matrix, clean-build reproducibility across
machines, and binary provenance/SBOM. Native archives are scaffolding for those
checks, not a claim of universal Linux binary compatibility. The source build
remains available when a matching archive is unavailable.

Implementation references: [systemd command syntax](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html),
[D-Bus activation specification](https://dbus.freedesktop.org/doc/dbus-specification.html),
[Claude Code MCP](https://code.claude.com/docs/en/mcp).

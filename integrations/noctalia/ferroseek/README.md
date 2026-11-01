# FerroSeek for Noctalia v5

Type `/ferro report` in Noctalia's launcher to search the existing FerroSeek
filename/path index, then press Enter to open a result. Uses the CLI's
versioned JSON output, with no second indexer or MCP server.

Requires Noctalia plugin API 24+, the updated `ferroseek` CLI and GLib's `gio`.
Install with `./scripts/install.sh --noctalia`; `--enable` also activates the
plugin and live indexer. The managed plugin copy is independent of the checkout.
For development, add `integrations/noctalia` as a Noctalia **path source**, then
enable `lucenx9/ferroseek`. Point that source at the parent of this plugin folder.

See [installation, configuration and validation](../../../docs/NOCTALIA.md).

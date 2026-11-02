#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Synthetic single-instance application, used only by the isolated KWin test."""
import json
import os
from pathlib import Path
import sys


def main():
    if os.environ.get("FERROSEEK_SMOKE_ISOLATED") != "1":
        raise SystemExit("Only run inside the isolated launcher test")
    log = Path(os.environ["FERROSEEK_ACTIVATION_LOG"])

    def record(event, **fields):
        with log.open("a") as stream:
            stream.write(json.dumps({"event": event, "pid": os.getpid(), **fields}) + "\n")

    # Record presence before the toolkit consumes/removes the startup token.
    record("started", token_present=bool(os.environ.get("XDG_ACTIVATION_TOKEN")))
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gio, Gtk

    class Application(Gtk.Application):
        def __init__(self):
            super().__init__(application_id="org.ferroseek.ActivationProbe")
            self.window = None
            self.activation_count = 0

        def do_startup(self):
            Gtk.Application.do_startup(self)
            quit_action = Gio.SimpleAction.new("quit", None)
            quit_action.connect("activate", lambda *_: self.quit())
            self.add_action(quit_action)

        def do_activate(self):
            self.activation_count += 1
            if self.window is None:
                self.window = Gtk.ApplicationWindow(application=self, title="FerroSeek activation target")
                entry = Gtk.Entry()
                entry.connect("changed", lambda entry: record("input", text=entry.get_text()))
                self.window.set_child(entry)
                self.window.set_default_size(400, 100)
                self.window.connect("map", lambda _: entry.grab_focus())
                self.window.connect("notify::is-active", lambda window, _: record(
                    "focus", active=window.is_active(), activation=self.activation_count))
            record("activated", activation=self.activation_count)
            self.window.present()

    return Application().run([sys.argv[0]])


if __name__ == "__main__":
    raise SystemExit(main())

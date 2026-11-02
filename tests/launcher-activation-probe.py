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
    record("started", token_present=bool(os.environ.get("XDG_ACTIVATION_TOKEN")), arguments=sys.argv[1:],
           action=os.environ.get("FERROSEEK_TEST_ACTION"))
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gio, GLib, Gtk

    class Application(Gtk.Application):
        def __init__(self):
            super().__init__(application_id="org.ferroseek.ActivationProbe", flags=Gio.ApplicationFlags.HANDLES_OPEN)
            self.window = None
            self.activation_count = 0

        def do_startup(self):
            Gtk.Application.do_startup(self)
            quit_action = Gio.SimpleAction.new("quit", None)
            quit_action.connect("activate", lambda *_: self.quit())
            self.add_action(quit_action)
            self.fm_info = Gio.DBusNodeInfo.new_for_xml("""<node>
              <interface name="org.freedesktop.FileManager1"><method name="ShowItems">
                <arg type="as" direction="in"/><arg type="s" direction="in"/>
              </method></interface></node>""")
            self.fm_connection = self.get_dbus_connection()
            self.fm_registration = self.fm_connection.register_object(
                "/org/freedesktop/FileManager1", self.fm_info.interfaces[0], self.on_show_items, None, None)
            self.fm_owner = Gio.bus_own_name_on_connection(
                self.fm_connection, "org.freedesktop.FileManager1", Gio.BusNameOwnerFlags.NONE,
                lambda *_: record("file_manager_ready"), lambda *_: record("file_manager_name_lost"))

        def do_shutdown(self):
            Gio.bus_unown_name(self.fm_owner)
            self.fm_connection.unregister_object(self.fm_registration)
            Gtk.Application.do_shutdown(self)

        def do_open(self, files, n_files, hint):
            record("opened", uris=[file.get_uri() for file in files])
            self.do_activate()

        def on_show_items(self, connection, sender, object_path, interface_name, method_name, parameters, invocation):
            uris, token = parameters.unpack()
            assert self.window is not None
            self.activation_count += 1
            record("revealed", uris=uris, token_present=bool(token), activation=self.activation_count)
            if token:
                self.window.set_startup_id(token)
            self.window.present()
            invocation.return_value(GLib.Variant("()", ()))

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

    return Application().run(sys.argv)


if __name__ == "__main__":
    raise SystemExit(main())

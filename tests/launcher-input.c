/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Test-only input on the disposable KWin socket, never the desktop. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-client.h>
#include "fake-input-client-protocol.h"

static struct org_kde_kwin_fake_input *input;

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version)
{
    (void)data;
    if (!strcmp(interface, "org_kde_kwin_fake_input") && version >= 4) {
        input = wl_registry_bind(registry, name, &org_kde_kwin_fake_input_interface, 4);
    }
}

static void removed(void *data, struct wl_registry *registry, uint32_t name)
{
    (void)data;
    (void)registry;
    (void)name;
}

int main(int argc, char **argv)
{
    const char *isolated = getenv("FERROSEEK_SMOKE_ISOLATED");
    const char *socket = getenv("WAYLAND_DISPLAY");
    if (!isolated || strcmp(isolated, "1") || !socket ||
        strncmp(socket, "wayland-ferroseek-smoke-", 24)) {
        fputs("Refusing input outside the isolated launcher test\n", stderr);
        return 1;
    }
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    static const struct wl_registry_listener listener = {global, removed};
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !input) {
        fputs("KWin fake input v4 is unavailable\n", stderr);
        return 1;
    }
    org_kde_kwin_fake_input_authenticate(input, "FerroSeek isolated test", "Keyboard focus regression");
    if (wl_display_roundtrip(display) < 0) return 1;
    for (int i = 1; i < argc; i++) {
        char *end;
        if (!strcmp(argv[i], "--double-click")) {
            if (i + 2 >= argc) return 1;
            errno = 0;
            long x = strtol(argv[++i], &end, 10);
            if (errno || *end || x < 0 || x > 4096) return 1;
            long y = strtol(argv[++i], &end, 10);
            if (errno || *end || y < 0 || y > 4096) return 1;
            org_kde_kwin_fake_input_pointer_motion_absolute(input, wl_fixed_from_int((int)x), wl_fixed_from_int((int)y));
            if (wl_display_roundtrip(display) < 0) return 1;
            for (int click = 0; click < 2; click++) {
                org_kde_kwin_fake_input_button(input, 0x110, 1); /* BTN_LEFT */
                if (wl_display_roundtrip(display) < 0) return 1;
                org_kde_kwin_fake_input_button(input, 0x110, 0);
                if (wl_display_roundtrip(display) < 0) return 1;
            }
            continue;
        }
        errno = 0;
        long key = strtol(argv[i], &end, 10);
        if (errno || *end || key == 0 || key < -255 || key > 255) return 1;
        org_kde_kwin_fake_input_keyboard_key(input, (uint32_t)labs(key), key > 0 ? 1 : 0);
        if (wl_display_roundtrip(display) < 0) return 1;
    }
    wl_display_disconnect(display);
    return 0;
}

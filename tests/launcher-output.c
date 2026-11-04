/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Test-only output configuration on the wrapper's private virtual KWin. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-client.h>
#include "output-device-client-protocol.h"
#include "output-management-client-protocol.h"

struct output {
    struct kde_output_device_v2 *proxy;
    char name[64];
    int32_t x, y, width, height, enabled;
    double scale;
};
static struct output outputs[8];
static unsigned count;
static struct kde_output_management_v2 *manager;
static int completed;

static void mode_size(void *data, struct kde_output_device_mode_v2 *mode, int32_t width, int32_t height)
{ (void)mode; struct output *o = data; o->width = width; o->height = height; }
static void mode_refresh(void *data, struct kde_output_device_mode_v2 *mode, int32_t value)
{ (void)data; (void)mode; (void)value; }
static void mode_preferred(void *data, struct kde_output_device_mode_v2 *mode)
{ (void)data; (void)mode; }
static void mode_removed(void *data, struct kde_output_device_mode_v2 *mode)
{ (void)data; kde_output_device_mode_v2_destroy(mode); }
static const struct kde_output_device_mode_v2_listener mode_listener = {
    .size = mode_size, .refresh = mode_refresh, .preferred = mode_preferred, .removed = mode_removed
};
static void geometry(void *data, struct kde_output_device_v2 *proxy, int32_t x, int32_t y,
                     int32_t width, int32_t height, int32_t subpixel, const char *make,
                     const char *model, int32_t transform)
{
    (void)proxy; (void)width; (void)height; (void)subpixel; (void)make; (void)model; (void)transform;
    struct output *o = data; o->x = x; o->y = y;
}
static void current_mode(void *data, struct kde_output_device_v2 *proxy, struct kde_output_device_mode_v2 *mode)
{ (void)data; (void)proxy; (void)mode; }
static void mode(void *data, struct kde_output_device_v2 *proxy, struct kde_output_device_mode_v2 *mode)
{ (void)proxy; kde_output_device_mode_v2_add_listener(mode, &mode_listener, data); }
static void done(void *data, struct kde_output_device_v2 *proxy)
{ (void)data; (void)proxy; }
static void scale(void *data, struct kde_output_device_v2 *proxy, wl_fixed_t value)
{ (void)proxy; ((struct output *)data)->scale = wl_fixed_to_double(value); }
static void enabled(void *data, struct kde_output_device_v2 *proxy, int32_t value)
{ (void)proxy; ((struct output *)data)->enabled = value; }
static void name(void *data, struct kde_output_device_v2 *proxy, const char *value)
{ (void)proxy; snprintf(((struct output *)data)->name, sizeof(outputs[0].name), "%s", value); }
static void string_event(void *data, struct kde_output_device_v2 *proxy, const char *value)
{ (void)data; (void)proxy; (void)value; }
static void uint_event(void *data, struct kde_output_device_v2 *proxy, uint32_t value)
{ (void)data; (void)proxy; (void)value; }
static const struct kde_output_device_v2_listener output_listener = {
    .geometry = geometry, .current_mode = current_mode, .mode = mode, .done = done,
    .scale = scale, .enabled = enabled, .name = name,
    .edid = string_event, .uuid = string_event, .serial_number = string_event, .eisa_id = string_event,
    .capabilities = uint_event, .overscan = uint_event, .vrr_policy = uint_event, .rgb_range = uint_event
};
static void global(void *data, struct wl_registry *registry, uint32_t id, const char *interface, uint32_t version)
{
    (void)data;
    if (!strcmp(interface, "kde_output_management_v2")) {
        manager = wl_registry_bind(registry, id, &kde_output_management_v2_interface, 1);
    } else if (!strcmp(interface, "kde_output_device_v2") && version >= 2 && count < 8) {
        struct output *o = &outputs[count++];
        o->scale = 1.0;
        o->proxy = wl_registry_bind(registry, id, &kde_output_device_v2_interface, 2);
        kde_output_device_v2_add_listener(o->proxy, &output_listener, o);
    }
}
static void removed(void *data, struct wl_registry *registry, uint32_t id)
{ (void)data; (void)registry; (void)id; }
static const struct wl_registry_listener registry_listener = { global, removed };
static void applied(void *data, struct kde_output_configuration_v2 *config)
{ (void)data; (void)config; completed = 1; }
static void failed(void *data, struct kde_output_configuration_v2 *config)
{ (void)data; (void)config; completed = -1; }
static const struct kde_output_configuration_v2_listener configuration_listener = { applied, failed };
static int integer(const char *text, int32_t *value)
{
    char *end; errno = 0; long v = strtol(text, &end, 10);
    if (errno || !*text || *end || v < -4096 || v > 4096) return 0;
    *value = (int32_t)v; return 1;
}
int main(int argc, char **argv)
{
    const char *isolated = getenv("FERROSEEK_SMOKE_ISOLATED"), *socket = getenv("WAYLAND_DISPLAY");
    if (!isolated || strcmp(isolated, "1") || !socket || strncmp(socket, "wayland-ferroseek-smoke-", 24)) {
        fputs("Refusing output changes outside the isolated launcher test\n", stderr); return 1;
    }
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0 || wl_display_roundtrip(display) < 0 || !manager || count != 2) return 1;
    for (unsigned i = 0; i < count; ++i) {
        if (strncmp(outputs[i].name, "Virtual-", 8) || !outputs[i].name[8] ||
            strspn(outputs[i].name + 8, "0123456789") != strlen(outputs[i].name + 8)) return 1;
    }
    if (argc == 2 && !strcmp(argv[1], "--list")) {
        fputs("{\"outputs\":[", stdout);
        for (unsigned i = 0; i < count; ++i) {
            struct output *o = &outputs[i];
            printf("%s{\"id\":%u,\"name\":\"%s\",\"enabled\":%s,\"pos\":{\"x\":%d,\"y\":%d},\"scale\":%.3f,\"width\":%d,\"height\":%d}",
                   i ? "," : "", i, o->name, o->enabled ? "true" : "false", o->x, o->y, o->scale, o->width, o->height);
        }
        puts("]}"); wl_display_disconnect(display); return 0;
    }
    if (argc < 6 || (argc - 1) % 5) return 1;
    struct kde_output_configuration_v2 *config = kde_output_management_v2_create_configuration(manager);
    kde_output_configuration_v2_add_listener(config, &configuration_listener, NULL);
    for (int arg = 1; arg < argc; arg += 5) {
        struct output *o = NULL;
        for (unsigned i = 0; i < count; ++i) if (!strcmp(outputs[i].name, argv[arg])) o = &outputs[i];
        int32_t x, y, active; char *end; errno = 0;
        double factor = strtod(argv[arg+3], &end);
        if (!o || errno || !*argv[arg+3] || *end || !(factor >= 0.5 && factor <= 3) ||
            !integer(argv[arg+1], &x) || !integer(argv[arg+2], &y) || !integer(argv[arg+4], &active) || (active != 0 && active != 1)) return 1;
        kde_output_configuration_v2_position(config, o->proxy, x, y);
        kde_output_configuration_v2_scale(config, o->proxy, wl_fixed_from_double(factor));
        kde_output_configuration_v2_enable(config, o->proxy, active);
    }
    kde_output_configuration_v2_apply(config);
    while (!completed) if (wl_display_dispatch(display) < 0) return 1;
    kde_output_configuration_v2_destroy(config);
    wl_display_disconnect(display);
    if (completed != 1) { fputs("Virtual output configuration rejected\n", stderr); return 1; }
    puts("applied"); return 0;
}

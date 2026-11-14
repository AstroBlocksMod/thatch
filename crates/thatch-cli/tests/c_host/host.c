/* A host written in C against thatch.h, the smallest one that does what the headless host does:
 * it prints every bubble as "Name says: text" and runs frames until no script is left or
 * --frames N frames have run. The tests build every sample with `thatch build --object --no-main`,
 * link it with this file and libthatch_runtime.a, and compare the output with scratch-vm's. It is
 * also the template for a real host. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "thatch.h"

static void say(void *data, ThatchRuntime *rt, int64_t target, const char *text, size_t len, int8_t think) {
    size_t name_len = 0;
    const char *name;
    (void)data;
    if (len == 0) {
        return; /* the bubble was cleared */
    }
    name = thatch_runtime_target_name(rt, target, &name_len);
    printf("%.*s %s: %.*s\n", (int)name_len, name ? name : "", think ? "thinks" : "says", (int)len, text);
}

int main(int argc, char **argv) {
    uint64_t max_frames = UINT64_MAX;
    ThatchHost host;
    ThatchRuntime *rt;
    int32_t loaded;

    if (argc == 3 && strcmp(argv[1], "--frames") == 0) {
        max_frames = strtoull(argv[2], NULL, 10);
    } else if (argc != 1) {
        fprintf(stderr, "usage: %s [--frames N]\n", argv[0]);
        return 2;
    }

    host.data = NULL;
    host.say = say;
    host.now_ms = NULL; /* the virtual clock, so the output is repeatable */

    rt = thatch_runtime_create(THATCH_ABI_VERSION, &host, 1);
    if (rt == NULL) {
        fprintf(stderr, "the runtime library is for another ABI version than thatch.h\n");
        return 1;
    }
    loaded = thatch_runtime_load(rt, thatch_program_abi_version(), thatch_program_init);
    if (loaded != THATCH_LOAD_OK) {
        fprintf(stderr, "the program did not load: %d\n", loaded);
        thatch_runtime_destroy(rt);
        return 1;
    }
    thatch_runtime_green_flag(rt);
    while (thatch_runtime_has_threads(rt) && thatch_runtime_frame_count(rt) < max_frames) {
        if (thatch_runtime_frame(rt) != 0) {
            size_t len = 0;
            const char *message = thatch_runtime_last_error(rt, &len);
            fprintf(stderr, "error: %.*s\n", (int)len, message ? message : "");
            thatch_runtime_destroy(rt);
            return 1;
        }
    }
    thatch_runtime_destroy(rt);
    return 0;
}

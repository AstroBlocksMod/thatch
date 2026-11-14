/* The Thatch host API. Generated from crates/thatch-abi/src/lib.rs; do not edit. */
#ifndef THATCH_H
#define THATCH_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The ABI version this header describes. A program reports the version it was compiled for through
 * thatch_program_abi_version, and thatch_runtime_load refuses any other. A host passes it to
 * thatch_runtime_create, which refuses any other too. */
#define THATCH_ABI_VERSION 3

/* The runtime. Opaque; create one with thatch_runtime_create. */
typedef struct ThatchRuntime ThatchRuntime;

/* A compiled program exports these two functions. */
typedef void (*ThatchInitFn)(ThatchRuntime *rt);
void thatch_program_init(ThatchRuntime *rt);
int32_t thatch_program_abi_version(void);

/* Hat kinds for thatch_runtime_start_hats. */
#define THATCH_HAT_FLAG 0
#define THATCH_HAT_BROADCAST 1

/* Results of thatch_runtime_load. */
#define THATCH_LOAD_OK 0
#define THATCH_LOAD_WRONG_ABI_VERSION 1
#define THATCH_LOAD_NO_STAGE 2
#define THATCH_LOAD_ALREADY_LOADED 3

/* Called when a target says or thinks text: UTF-8, len bytes, not NUL-terminated, already cut and
 * formatted as scratch-vm shows it. Empty text clears the bubble. */
typedef void (*ThatchSayFn)(void *data, ThatchRuntime *rt, int64_t target, const char *text, size_t len, int8_t think);

/* The host's clock in milliseconds, read by wait and timer. Only differences matter. */
typedef double (*ThatchNowFn)(void *data);

/* What a host gives the runtime: data is passed back to every callback. A null callback means the
 * host does not care, except now_ms, where null selects the virtual clock of one frame per thirtieth
 * of a second that makes headless runs repeatable. A callback may ask the runtime about its state
 * (thatch_runtime_target_name and the like), but must not run a frame, load, start or stop
 * anything, or destroy the runtime. */
typedef struct ThatchHost {
    void *data;
    ThatchSayFn say;
    ThatchNowFn now_ms;
} ThatchHost;

/* A host creates a runtime, loads a program, starts the green flag and then runs a frame at a time,
 * drawing after each one that requested a redraw. Nothing here is thread-safe; one runtime belongs to
 * one thread. */

/* Creates a runtime that reports to `host` (copied, or null for no callbacks) and seeds `pick
 * random` with `seed`. `abi_version` is the THATCH_ABI_VERSION the host was compiled with,
 * which says how big its ThatchHost is; for any other version this returns null and reads
 * nothing from `host`. */
ThatchRuntime *thatch_runtime_create(int32_t abi_version, const ThatchHost *host, uint64_t seed);

/* Checks the program's ABI version (from thatch_program_abi_version) against the runtime's,
 * then lets the program register itself. Returns THATCH_LOAD_OK or a THATCH_LOAD_* error, in
 * which case the runtime is left empty, as it was created. A runtime holds one program:
 * loading another into it gives THATCH_LOAD_ALREADY_LOADED and changes nothing. */
int32_t thatch_runtime_load(ThatchRuntime *rt, int32_t abi_version, ThatchInitFn init);

/* Stops every script and clears every bubble, as the stop sign does, then resets the timer and
 * starts the green flag scripts, as clicking the green flag does. */
void thatch_runtime_green_flag(ThatchRuntime *rt);

/* Starts every script with the given hat kind (a THATCH_HAT_* value) and argument, such as a
 * broadcast name, as a broadcast block would. */
void thatch_runtime_start_hats(ThatchRuntime *rt, int64_t hat, const char *arg, size_t len);

/* Runs one frame: steps the threads until they all wait for the next frame, a redraw was
 * requested or 75 percent of a frame has passed. Returns 0, or nonzero after an error that
 * thatch_runtime_last_error describes, after which the runtime may only be destroyed. */
int32_t thatch_runtime_frame(ThatchRuntime *rt);

/* Whether the host should draw: something changed what the stage shows (a bubble, for one) or
 * a wait began, during the last frame or since it ended, or the program was just loaded. */
int8_t thatch_runtime_redraw_requested(const ThatchRuntime *rt);

/* Whether any script is still running or waiting. Stopped scripts do not count, even before
 * the next frame lets them finish. */
int8_t thatch_runtime_has_threads(const ThatchRuntime *rt);

/* How many frames have run. */
uint64_t thatch_runtime_frame_count(const ThatchRuntime *rt);

/* Stops every script and clears every bubble, as the stop sign does. */
void thatch_runtime_stop_all(ThatchRuntime *rt);

/* How many targets the program registered. The stage is one of them. */
int64_t thatch_runtime_target_count(const ThatchRuntime *rt);

/* The name of a target as UTF-8, `*len` bytes long, not NUL-terminated. Valid until the
 * runtime is destroyed. Null for an index out of range. */
const char *thatch_runtime_target_name(const ThatchRuntime *rt, int64_t target, size_t *len);

/* Whether a target is the stage. */
int8_t thatch_runtime_target_is_stage(const ThatchRuntime *rt, int64_t target);

/* The message of the last error, UTF-8 and `*len` bytes long, or null when there was none. */
const char *thatch_runtime_last_error(const ThatchRuntime *rt, size_t *len);

/* Stops every script, lets the suspended ones finish and frees the runtime. */
void thatch_runtime_destroy(ThatchRuntime *rt);

#ifdef __cplusplus
}
#endif

#endif

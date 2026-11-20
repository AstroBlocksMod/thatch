// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// The Thatch host on Scratch Everywhere!.
//
// A game built for this host is three things linked together: the program Thatch compiled, the
// Thatch runtime, and this adapter, which owns `main`. The Scratch Everywhere! library is loaded
// next to them at run time. The division of work:
//
//   - The Thatch runtime runs every script and owns all Scratch state: variables, the scheduler,
//     and where each sprite is and how it looks.
//   - Scratch Everywhere! loads the project file for its costumes, opens the window, draws the
//     sprites and their speech bubbles, and reads the keyboard and the mouse. It runs no scripts:
//     its two startThread functions are patched to hand every hat to the hook set below.
//   - This adapter implements Thatch's host table (thatch.h) on top of the library and runs the
//     frame loop. After each frame it copies the state of the sprites that changed into the
//     library's own sprites, which is all the library needs to draw them.
//
// The adapter only calls the library, so it is licensed like the rest of Thatch. The library and
// the patch to it are LGPL-3.0-or-later; see ../LICENSE.

#include <algorithm>
#include <chrono>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

#if defined(__APPLE__)
#include <mach-o/dyld.h>
#elif defined(__linux__)
#include <unistd.h>
#endif

#include <audio.hpp>
#include <collision.hpp>
#include <host_hooks.hpp>
#include <input.hpp>
#include <os.hpp>
#include <renderer.hpp>
#include <runtime.hpp>
#include <speech_manager.hpp>
#include <unzip.hpp>

#include "thatch.h"

namespace {

using Clock = std::chrono::steady_clock;

/// One line of an input script: something the player does before a frame.
struct ScriptedInput {
    uint64_t frame = 0;
    enum { Key, MouseMove, MouseButton } kind = Key;
    std::string key;
    /// The target a mouse button event is for; empty for the one under the pointer.
    std::string target;
    bool down = false;
    double x = 0;
    double y = 0;
};

struct Options {
    /// The .sb3 the costumes come from.
    std::string project;
    /// Stop after this many frames; forever by default.
    uint64_t frames = UINT64_MAX;
    /// The seed of `pick random`; the time by default.
    uint64_t seed = 0;
    bool hasSeed = false;
    /// Also print every bubble as `Name says: text`, as the headless host does.
    bool printBubbles = false;
    /// Input to play on top of the real keyboard and mouse, in frame order.
    std::vector<ScriptedInput> input;
};

/// What the adapter keeps between callbacks.
struct Host {
    ThatchRuntime *rt = nullptr;
    /// The library's sprite of each Thatch target, by target index.
    std::vector<Sprite *> sprites;
    std::unordered_map<Sprite *, int64_t> targets;
    int64_t stage = -1;
    /// Targets whose state changed since it was last copied into the library.
    std::vector<bool> changed;
    bool anyChanged = false;
    /// The keys that were down in the last frame, as the library names them, and for how many
    /// frames each has been held.
    std::vector<std::string> keysDown;
    std::unordered_map<std::string, uint64_t> keyHeld;
    bool mouseDown = false;
    /// The input script: what is left of it, and its mouse, which stands in for the real one from
    /// its first mouse event on.
    std::vector<ScriptedInput> script;
    size_t nextScripted = 0;
    bool scriptMouse = false;
    double scriptMouseX = 0;
    double scriptMouseY = 0;
    bool printBubbles = false;
    Clock::time_point started = Clock::now();
};

Sprite::RotationStyle rotationStyle(int32_t style) {
    switch (style) {
    case THATCH_ROTATE_LEFT_RIGHT:
        return Sprite::LEFT_RIGHT;
    case THATCH_ROTATE_DONT_ROTATE:
        return Sprite::NONE;
    default:
        return Sprite::ALL_AROUND;
    }
}

/// Copies how a target looks from the Thatch runtime into the library's sprite.
void sync(Host &host, int64_t target) {
    ThatchSprite state;
    if (target < 0 || static_cast<size_t>(target) >= host.sprites.size()) return;
    if (!thatch_runtime_sprite(host.rt, target, &state)) return;
    Sprite *sprite = host.sprites[target];

    if (!sprite->isStage) {
        sprite->xPosition = state.x;
        sprite->yPosition = state.y;
        sprite->rotation = static_cast<float>(state.direction);
        sprite->rotationStyle = rotationStyle(state.rotation_style);
        sprite->visible = state.visible != 0;
        const float size = static_cast<float>(state.size);
        if (sprite->size != size) {
            sprite->size = size;
            // Vector costumes are drawn at the size they are shown at.
            Render::resizeSVGs(sprite);
        }
    }
    if (state.costume >= 0 && static_cast<size_t>(state.costume) < sprite->costumes.size() &&
        sprite->currentCostume != static_cast<int>(state.costume)) {
        // Loads the costume's image when it is first shown.
        Scratch::switchCostume(sprite, static_cast<double>(state.costume));
    }
    sprite->colorEffect = static_cast<float>(state.effects[THATCH_EFFECT_COLOR]);
    sprite->fisheyeEffect = static_cast<float>(state.effects[THATCH_EFFECT_FISHEYE]);
    sprite->whirlEffect = static_cast<float>(state.effects[THATCH_EFFECT_WHIRL]);
    sprite->pixelateEffect = static_cast<float>(state.effects[THATCH_EFFECT_PIXELATE]);
    sprite->mosaicEffect = static_cast<float>(state.effects[THATCH_EFFECT_MOSAIC]);
    sprite->brightnessEffect = static_cast<float>(state.effects[THATCH_EFFECT_BRIGHTNESS]);
    sprite->ghostEffect = static_cast<float>(state.effects[THATCH_EFFECT_GHOST]);
    host.changed[target] = false;
}

/// Gives the library's sprites the drawing order the runtime keeps. The library draws its sprite
/// list back to front and sorts it by layer, the highest in front.
void syncOrder(Host &host) {
    std::vector<int64_t> order(host.sprites.size());
    const size_t count = thatch_runtime_draw_order(host.rt, order.data(), order.size());
    for (size_t layer = 0; layer < count && layer < order.size(); layer++) {
        const int64_t target = order[layer];
        if (target >= 0 && static_cast<size_t>(target) < host.sprites.size()) {
            host.sprites[target]->layer = static_cast<int>(layer);
        }
    }
    Scratch::sortSprites();
}

/// Copies every change of the last frame into the library, so it draws what the runtime has.
void syncChanged(Host &host) {
    if (!host.anyChanged) return;
    for (size_t target = 0; target < host.changed.size(); target++) {
        if (host.changed[target]) sync(host, static_cast<int64_t>(target));
    }
    syncOrder(host);
    host.anyChanged = false;
}

void markChanged(Host &host, int64_t target) {
    if (target < 0 || static_cast<size_t>(target) >= host.changed.size()) return;
    host.changed[target] = true;
    host.anyChanged = true;
}

extern "C" {

/// A target says or thinks something: the library draws the bubble.
void hostSay(void *data, ThatchRuntime *rt, int64_t target, const char *text, size_t len, int8_t think) {
    Host &host = *static_cast<Host *>(data);
    if (target < 0 || static_cast<size_t>(target) >= host.sprites.size()) return;
    Sprite *sprite = host.sprites[target];
    const std::string message(text, len);

    if (host.printBubbles && len != 0) {
        size_t nameLen = 0;
        const char *name = thatch_runtime_target_name(rt, target, &nameLen);
        if (name != nullptr) std::fwrite(name, 1, nameLen, stdout);
        std::printf(" %s: ", think ? "thinks" : "says");
        std::fwrite(text, 1, len, stdout);
        std::putchar('\n');
        std::fflush(stdout);
    }

    // A renderer without bubbles (the headless one) has no speech manager.
    if (!Render::createSpeechManager()) return;
    SpeechManager *speech = Render::getSpeechManager();
    if (speech == nullptr) return;
    if (len == 0) {
        speech->clearSpeech(sprite);
    } else {
        // The runtime times `say for seconds` itself and clears the bubble when the time is up.
        speech->showSpeech(sprite, message, -1, think ? "think" : "say");
    }
}

/// The clock `wait` and `timer` read.
double hostNow(void *data) {
    const Host &host = *static_cast<Host *>(data);
    return std::chrono::duration<double, std::milli>(Clock::now() - host.started).count();
}

/// A target looks different: it is copied into the library before the next drawing.
void hostSpriteChanged(void *data, ThatchRuntime *, int64_t target) {
    markChanged(*static_cast<Host *>(data), target);
}

/// How big a sprite is drawn, which the runtime needs for fencing, bouncing and size limits. The
/// library knows the costume's box, which it also uses for its own fencing; it does not rotate
/// it, so the tight bounds and the bounding box are the same here.
int8_t hostMeasure(void *data, ThatchRuntime *, int64_t target, int32_t what, double *out) {
    Host &host = *static_cast<Host *>(data);
    if (target < 0 || static_cast<size_t>(target) >= host.sprites.size()) return 0;
    Sprite *sprite = host.sprites[target];
    if (sprite->isStage || sprite->costumes.empty()) return 0;
    // The runtime asks in the middle of a block, so the library's copy may be behind.
    sync(host, target);
    if (sprite->spriteWidth < 1 || sprite->spriteHeight < 1) Scratch::loadCurrentCostumeImage(sprite);
    if (sprite->spriteWidth < 1 || sprite->spriteHeight < 1) return 0;

    if (what == THATCH_MEASURE_COSTUME_SIZE) {
        // The library keeps bitmap costumes at their stored resolution.
        const int resolution = std::max(1, sprite->costumes[sprite->currentCostume].bitmapResolution);
        out[0] = static_cast<double>(sprite->spriteWidth) / resolution;
        out[1] = static_cast<double>(sprite->spriteHeight) / resolution;
        out[2] = 0;
        out[3] = 0;
        return 1;
    }
    const collision::AABB box = collision::getSpriteBounds(sprite);
    out[0] = box.left;
    out[1] = box.right;
    out[2] = box.bottom;
    out[3] = box.top;
    return 1;
}

/// The library found a hat to start (a key, a click) and hands it over instead of running it.
/// Nothing to do with it: the adapter reports the keys and clicks themselves to the Thatch
/// runtime (see `reportInput`), which starts those hats by scratch-vm's rules. The hook is what
/// keeps the library from starting script threads of its own.
void hostStartHat(void *, Sprite *, Block *, bool) {
}

} // extern "C"

/// The frontmost target under a point in Scratch coordinates, as scratch-render picks it: sprites
/// that are hidden or fully ghosted do not count, the stage is under everything, and a point
/// outside the stage hits nothing (-1).
int64_t targetAt(Host &host, double x, double y) {
    // A point that is not a number is on no stage; the comparisons below would let it through.
    if (!std::isfinite(x) || !std::isfinite(y) || std::abs(x) > thatch_runtime_stage_width(host.rt) / 2 ||
        std::abs(y) > thatch_runtime_stage_height(host.rt) / 2) {
        return -1;
    }
    // The library keeps its sprites front to back, so every sprite in front of this one was
    // tested already. Its click mode also tests the sprite in front, but counts hidden ones,
    // which would let a hidden sprite block a click, so it is not used.
    for (Sprite *sprite : Scratch::sprites) {
        if (sprite->isStage || !sprite->visible || sprite->ghostEffect >= 100.0f) continue;
        const bool hit = Scratch::accurateCollision ? collision::pointInSprite(sprite, x, y)
                                                    : collision::pointInSpriteFast(sprite, x, y);
        if (!hit) continue;
        const auto found = host.targets.find(sprite);
        if (found != host.targets.end()) return found->second;
    }
    return host.stage;
}

/// The target called `name`, or -1. Scratch keeps sprite names unique, and the stage's name is
/// never a sprite's.
int64_t targetNamed(Host &host, const std::string &name) {
    const int64_t count = thatch_runtime_target_count(host.rt);
    for (int64_t target = 0; target < count; target++) {
        size_t len = 0;
        const char *found = thatch_runtime_target_name(host.rt, target, &len);
        if (found != nullptr && std::string(found, len) == name) return target;
    }
    return -1;
}

/// Plays the events of the input script that are due before `frame`, each one in turn, as the
/// headless host does: two events for the same key in one frame are two events, and the runtime
/// names the keys, so "left arrow" releases what "ArrowLeft" pressed. A key held by the script
/// does not repeat, as it does not in the headless host.
void playScript(Host &host, uint64_t frame) {
    while (host.nextScripted < host.script.size() && host.script[host.nextScripted].frame <= frame) {
        const ScriptedInput &event = host.script[host.nextScripted++];
        switch (event.kind) {
        case ScriptedInput::Key:
            thatch_runtime_key(host.rt, event.key.data(), event.key.size(), event.down ? 1 : 0);
            break;
        case ScriptedInput::MouseMove:
            // From the script's first mouse event on, it is the mouse.
            host.scriptMouse = true;
            host.scriptMouseX = event.x;
            host.scriptMouseY = event.y;
            thatch_runtime_mouse_move(host.rt, event.x, event.y);
            break;
        case ScriptedInput::MouseButton: {
            host.scriptMouse = true;
            host.mouseDown = event.down;
            // A target the script names gets the event wherever the pointer is, as in the
            // headless host, so a script plays the same on both.
            const int64_t target = event.target.empty() ? targetAt(host, host.scriptMouseX, host.scriptMouseY)
                                                        : targetNamed(host, event.target);
            thatch_runtime_mouse_button(host.rt, event.down ? 1 : 0, target, 0);
            break;
        }
        }
    }
}

/// Tells the Thatch runtime what the player did since the last frame: the events of the input
/// script, then the changes in the state the library polled.
void reportInput(Host &host, uint64_t frame) {
    playScript(host, frame);

    // The library names keys as Scratch does ("a", "space", "up arrow") and adds "any".
    std::vector<std::string> keys;
    for (const std::string &key : Input::inputKeys) {
        if (key != "any") keys.push_back(key);
    }
    for (const std::string &key : host.keysDown) {
        if (std::find(keys.begin(), keys.end(), key) == keys.end()) {
            thatch_runtime_key(host.rt, key.data(), key.size(), 0);
            host.keyHeld.erase(key);
        }
    }
    // A key that stays down repeats, as a keyboard does, which is what starts `when key pressed`
    // scripts again while a key is held in Scratch. The library polls the keyboard instead of
    // taking its events, so the repeat is made here, by the rule the library uses for its own
    // scripts: after half a second, once a frame.
    const double repeatAfter = 15.0 * thatch_runtime_frame_rate(host.rt) / 30.0;
    for (const std::string &key : keys) {
        const uint64_t held = ++host.keyHeld[key];
        if (held == 1 || static_cast<double>(held) > repeatAfter) thatch_runtime_key(host.rt, key.data(), key.size(), 1);
    }
    host.keysDown = std::move(keys);

    // The real mouse, unless the script has taken it over.
    if (!host.scriptMouse) {
        const double x = Input::mousePointer.x;
        const double y = Input::mousePointer.y;
        thatch_runtime_mouse_move(host.rt, x, y);
        const bool down = Input::mousePointer.isPressed;
        if (down != host.mouseDown) {
            host.mouseDown = down;
            thatch_runtime_mouse_button(host.rt, down ? 1 : 0, targetAt(host, x, y), 0);
        }
    }

    // The library starts dragging a draggable sprite by itself and moves its own copy. Dragging is
    // not in the host table yet, so the drag is undone before it shows. Since no drag ever happens,
    // every release is reported as one that ends no drag, and a draggable sprite's release counts
    // as its click, as in Scratch when the mouse did not move it.
    if (Input::draggingSprite != nullptr) {
        const auto found = host.targets.find(Input::draggingSprite);
        if (found != host.targets.end()) markChanged(host, found->second);
        Input::draggingSprite = nullptr;
    }
}

/// Pairs every Thatch target with the library's sprite of the same name. False when the project
/// file is not the one the program was compiled from.
bool pairTargets(Host &host) {
    const int64_t count = thatch_runtime_target_count(host.rt);
    host.sprites.assign(static_cast<size_t>(count), nullptr);
    host.changed.assign(static_cast<size_t>(count), false);
    for (int64_t target = 0; target < count; target++) {
        size_t len = 0;
        const char *name = thatch_runtime_target_name(host.rt, target, &len);
        const std::string wanted(name != nullptr ? name : "", len);
        const bool isStage = thatch_runtime_target_is_stage(host.rt, target) != 0;
        for (Sprite *sprite : Scratch::sprites) {
            if (sprite->isClone || sprite->isStage != isStage || host.targets.count(sprite) != 0) continue;
            if (!isStage && sprite->name != wanted) continue;
            host.sprites[target] = sprite;
            host.targets[sprite] = target;
            break;
        }
        Sprite *sprite = host.sprites[target];
        if (sprite == nullptr) {
            std::fprintf(stderr, "error: the project file has no %s named \"%s\"\n", isStage ? "stage" : "sprite", wanted.c_str());
            return false;
        }
        // Costumes are shown by index, so the project file has to have the program's, by name and
        // in the same order.
        bool sameCostumes = static_cast<int64_t>(sprite->costumes.size()) == thatch_runtime_costume_count(host.rt, target);
        for (size_t costume = 0; sameCostumes && costume < sprite->costumes.size(); costume++) {
            size_t nameLen = 0;
            const char *costumeName = thatch_runtime_costume_name(host.rt, target, static_cast<int64_t>(costume), &nameLen);
            sameCostumes = costumeName != nullptr && sprite->costumes[costume].name == std::string(costumeName, nameLen);
        }
        if (!sameCostumes) {
            std::fprintf(stderr, "error: \"%s\" has other costumes in the project file than in the program\n", wanted.c_str());
            return false;
        }
        if (isStage) host.stage = target;
    }
    return true;
}

/// A whole number from 0 up, in decimal digits and nothing else. `strtoull` alone would take "-1"
/// (as the largest number there is) and quietly cap one that is too big.
bool parseNumber(const char *text, uint64_t &out) {
    if (text[0] < '0' || text[0] > '9') return false;
    char *end = nullptr;
    errno = 0;
    out = std::strtoull(text, &end, 10);
    return errno == 0 && *end == '\0';
}

/// The length in bytes of the whitespace character at `i` in the UTF-8 text `s`, or 0 when
/// there is none there. Whitespace is what Rust's `char::is_whitespace` takes for it, which is
/// what the headless host splits an input script on, so that a script reads the same on both.
size_t whitespaceAt(const std::string &s, size_t i) {
    const auto byte = [&](size_t k) -> uint32_t { return k < s.size() ? static_cast<unsigned char>(s[k]) : 0; };
    const uint32_t b = byte(i);
    if (b == ' ' || (b >= 0x09 && b <= 0x0D)) return 1;
    uint32_t c = 0;
    size_t len = 0;
    if ((b & 0xE0) == 0xC0 && (byte(i + 1) & 0xC0) == 0x80) {
        c = ((b & 0x1F) << 6) | (byte(i + 1) & 0x3F);
        len = 2;
    } else if ((b & 0xF0) == 0xE0 && (byte(i + 1) & 0xC0) == 0x80 && (byte(i + 2) & 0xC0) == 0x80) {
        c = ((b & 0x0F) << 12) | ((byte(i + 1) & 0x3F) << 6) | (byte(i + 2) & 0x3F);
        len = 3;
    } else {
        return 0;
    }
    const bool space = c == 0x85 || c == 0xA0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200A) || c == 0x2028 ||
                       c == 0x2029 || c == 0x202F || c == 0x205F || c == 0x3000;
    return space ? len : 0;
}

/// Moves `pos` past the whitespace at it in `line`.
void skipWhitespace(const std::string &line, size_t &pos) {
    while (pos < line.size()) {
        const size_t space = whitespaceAt(line, pos);
        if (space == 0) break;
        pos += space;
    }
}

/// The first word of `line` from `pos` on, which is moved past it and the whitespace after it.
std::string nextWord(const std::string &line, size_t &pos) {
    skipWhitespace(line, pos);
    const size_t start = pos;
    while (pos < line.size() && whitespaceAt(line, pos) == 0) pos++;
    const size_t end = pos;
    skipWhitespace(line, pos);
    return line.substr(start, end - start);
}

/// The rest of `line` from `pos` on, without the whitespace at its end, so that a name such as
/// "left arrow" or "Sprite 1" stays one name.
std::string restOfLine(const std::string &line, size_t pos) {
    size_t end = pos;
    for (size_t i = pos; i < line.size();) {
        const size_t space = whitespaceAt(line, i);
        if (space != 0) {
            i += space;
        } else {
            end = ++i;
        }
    }
    return line.substr(pos, end - pos);
}

/// Reads an input script in the format of `thatch run --input`: one event per line, starting
/// with the frame it happens before, with blank lines and `#` comments allowed.
///
///     5 key down space
///     8 key up space
///     20 mouse 100 -50
///     21 mouse down
///     25 mouse up Cat
///
/// `mouse down` and `mouse up` go to the target named after them, the rest of the line, and
/// without a name to the one under the pointer. Returns false after printing what is wrong.
bool readInputScript(const std::string &path, std::vector<ScriptedInput> &events) {
    std::ifstream file(path);
    if (!file) {
        std::fprintf(stderr, "error: could not read %s\n", path.c_str());
        return false;
    }
    std::string line;
    for (int number = 1; std::getline(file, line); number++) {
        size_t pos = 0;
        const std::string frame = nextWord(line, pos);
        if (frame.empty() || frame[0] == '#') continue;
        ScriptedInput event;
        const std::string kind = nextWord(line, pos);
        bool ok = parseNumber(frame.c_str(), event.frame);
        if (kind == "key") {
            const std::string state = nextWord(line, pos);
            event.key = restOfLine(line, pos);
            event.down = state == "down";
            ok = ok && (state == "down" || state == "up") && !event.key.empty();
        } else if (kind == "mouse") {
            const std::string first = nextWord(line, pos);
            if (first == "down" || first == "up") {
                event.kind = ScriptedInput::MouseButton;
                event.down = first == "down";
                event.target = restOfLine(line, pos);
            } else {
                event.kind = ScriptedInput::MouseMove;
                const std::string second = nextWord(line, pos);
                char *endX = nullptr;
                char *endY = nullptr;
                event.x = std::strtod(first.c_str(), &endX);
                event.y = std::strtod(second.c_str(), &endY);
                // strtod also reads "nan" and "inf", which are no place on the stage.
                ok = ok && !first.empty() && !second.empty() && *endX == '\0' && *endY == '\0' && pos == line.size() &&
                     std::isfinite(event.x) && std::isfinite(event.y);
            }
        } else {
            ok = false;
        }
        if (!ok) {
            std::fprintf(stderr, "error: %s line %d: expected FRAME key down|up NAME, FRAME mouse X Y or FRAME mouse down|up\n",
                         path.c_str(), number);
            return false;
        }
        events.push_back(event);
    }
    std::stable_sort(events.begin(), events.end(), [](const ScriptedInput &a, const ScriptedInput &b) { return a.frame < b.frame; });
    return true;
}

/// Where this executable is, however it was started: by path, or by name through PATH, in which
/// case `argv[0]` alone would not say. Falls back to `argv[0]` where the system cannot tell.
std::string executablePath(const char *argv0) {
#if defined(__APPLE__)
    char buffer[4096];
    uint32_t size = sizeof buffer;
    if (_NSGetExecutablePath(buffer, &size) == 0) return buffer;
#elif defined(__linux__)
    char buffer[4096];
    const ssize_t length = readlink("/proc/self/exe", buffer, sizeof buffer - 1);
    if (length > 0) return std::string(buffer, static_cast<size_t>(length));
#endif
    return argv0;
}

/// Reads the command line. Returns false after printing what is wrong.
bool parseArgs(int argc, char **argv, Options &options) {
    for (int i = 1; i < argc; i++) {
        const std::string arg = argv[i];
        if (arg == "--print-bubbles") {
            options.printBubbles = true;
        } else if (arg == "--input") {
            if (i + 1 >= argc) {
                std::fprintf(stderr, "error: --input needs a file\n");
                return false;
            }
            if (!readInputScript(argv[++i], options.input)) return false;
        } else if (arg == "--frames" || arg == "--seed") {
            uint64_t value = 0;
            if (i + 1 >= argc || !parseNumber(argv[i + 1], value)) {
                std::fprintf(stderr, "error: %s needs a number\n", arg.c_str());
                return false;
            }
            i++;
            if (arg == "--frames") {
                options.frames = value;
            } else {
                options.seed = value;
                options.hasSeed = true;
            }
        } else if (!arg.empty() && arg[0] != '-' && options.project.empty()) {
            options.project = arg;
        } else {
            std::fprintf(stderr, "usage: %s [PROJECT.sb3] [--frames N] [--seed N] [--input FILE] [--print-bubbles]\n", argv[0]);
            return false;
        }
    }
    // The project file lies next to the executable, under its name with .sb3 added.
    if (options.project.empty()) options.project = executablePath(argv[0]) + ".sb3";
    return true;
}

void shutDown() {
#ifdef ENABLE_AUDIO
    SoundPlayer::deinit();
#endif
    Render::deInit();
    OS::deinit();
}

/// Loads the program into a new runtime and gets the library ready to draw it. Returns false
/// after printing what went wrong.
bool start(Host &host, ThatchHost &table, const Options &options) {
    const uint64_t seed = options.hasSeed ? options.seed : static_cast<uint64_t>(Clock::now().time_since_epoch().count());
    host.rt = thatch_runtime_create(THATCH_ABI_VERSION, &table, seed);
    if (host.rt == nullptr) {
        std::fprintf(stderr, "error: the Thatch runtime is for another ABI version than this host\n");
        return false;
    }
    const int32_t loaded = thatch_runtime_load(host.rt, thatch_program_abi_version(), thatch_program_init);
    if (loaded == THATCH_LOAD_WRONG_ABI_VERSION) {
        std::fprintf(stderr, "error: the program was compiled for ABI version %d, but this runtime has version %d\n",
                     thatch_program_abi_version(), THATCH_ABI_VERSION);
        return false;
    }
    if (loaded != THATCH_LOAD_OK) {
        std::fprintf(stderr, "error: the program did not load (code %d)\n", loaded);
        return false;
    }
    if (!pairTargets(host)) return false;
    // A target the project lacks is a mistake in the script, not a click outside the stage.
    for (const ScriptedInput &event : host.script) {
        if (event.kind == ScriptedInput::MouseButton && !event.target.empty() && targetNamed(host, event.target) < 0) {
            std::fprintf(stderr, "error: the input script clicks \"%s\", which the project has no target named\n",
                         event.target.c_str());
            return false;
        }
    }

    // From here on the library hands over every hat it would have started.
    HostHooks::setStartHat(hostStartHat, &host);
    // The library shows variable monitors from its own copies of the variables, which nothing
    // updates, so they are left out until the runtime's variables are synced to them.
    Render::monitors.clear();

    // The runtime says how big the stage is and how fast it runs; the library follows.
    const int width = static_cast<int>(std::lround(thatch_runtime_stage_width(host.rt)));
    const int height = static_cast<int>(std::lround(thatch_runtime_stage_height(host.rt)));
    Renderer *renderer = Render::getGlobalRenderer();
    if ((width != Scratch::projectWidth || height != Scratch::projectHeight) && renderer != nullptr && renderer->window != nullptr) {
        renderer->window->resize(width, height);
    }
    Scratch::projectWidth = width;
    Scratch::projectHeight = height;
    Scratch::FPS = std::max(1, static_cast<int>(std::lround(thatch_runtime_frame_rate(host.rt))));
    Render::setRenderScale();

    for (size_t target = 0; target < host.sprites.size(); target++) {
        sync(host, static_cast<int64_t>(target));
    }
    syncOrder(host);
    return true;
}

/// Runs frames until the window closes or the frame limit is reached. Returns the exit status.
int run(Host &host, const Options &options) {
    const auto frameLength = std::chrono::duration_cast<Clock::duration>(
        std::chrono::duration<double>(1.0 / thatch_runtime_frame_rate(host.rt)));
    auto next = Clock::now();
    thatch_runtime_green_flag(host.rt);
    for (uint64_t frame = 0; frame < options.frames && Render::appShouldRun(); frame++) {
        Input::getInput();
        reportInput(host, frame);
        if (thatch_runtime_frame(host.rt) != 0) {
            size_t len = 0;
            const char *message = thatch_runtime_last_error(host.rt, &len);
            std::fprintf(stderr, "error: %.*s\n", static_cast<int>(len), message != nullptr ? message : "");
            return 1;
        }
        syncChanged(host);
        if (SpeechManager *speech = Render::getSpeechManager()) speech->update();
        Render::renderSprites();
        Scratch::flushCostumeImages();

        // One frame every frame length. After a stall the schedule starts over instead of
        // running the frames that were missed in a burst.
        next += frameLength;
        const auto now = Clock::now();
        if (next < now) {
            next = now;
        } else {
            std::this_thread::sleep_until(next);
        }
    }
    return 0;
}

} // namespace

int main(int argc, char **argv) {
    Options options;
    if (!parseArgs(argc, argv, options)) return 2;

    Unzip::filePath = options.project;
    if (!Scratch::initializeRuntime()) {
        std::fprintf(stderr, "error: Scratch Everywhere! could not start\n");
        shutDown();
        return 1;
    }
    if (!Unzip::load()) {
        std::fprintf(stderr, "error: could not load %s\n", options.project.c_str());
        shutDown();
        return 1;
    }

    Host host;
    host.printBubbles = options.printBubbles;
    host.script = options.input;
    ThatchHost table;
    std::memset(&table, 0, sizeof table);
    table.data = &host;
    table.say = hostSay;
    table.now_ms = hostNow;
    table.sprite_changed = hostSpriteChanged;
    table.measure = hostMeasure;

    int status = 1;
    if (start(host, table, options)) status = run(host, options);

    if (host.rt != nullptr) thatch_runtime_destroy(host.rt);
    HostHooks::setStartHat(nullptr, nullptr);
    Scratch::cleanupScratchProject();
    shutDown();
    return status;
}

//! The host side of sprite state: what a host that draws sees through the C API, and what it can
//! change by measuring sprites for the runtime. The headless samples cannot cover this, since
//! scratch-vm without a renderer neither fences nor clamps, so the expectations here come from
//! reading scratch-vm and scratch-render with a renderer present.

use std::ffi::c_void;
use std::ptr;

use thatch_abi::{Host, InitFn, MEASURE_COSTUME_SIZE, OpaqueRuntime, Sprite};
use thatch_runtime::api;

/// A stage and two sprites. The Cat runs off the stage, grows past the limit, shrinks below it,
/// turns, comes to the front, sets an effect past its limit and hides.
const PROJECT: &str = r#"{"targets":[
  {"isStage":true,"name":"Stage","costumes":[{"name":"backdrop1"}]},
  {"isStage":false,"name":"Cat","layerOrder":1,"x":0,"y":0,"costumes":[{"name":"a"},{"name":"b"}],"blocks":{
    "h":{"opcode":"event_whenflagclicked","next":"g","parent":null,"inputs":{},"fields":{},"topLevel":true},
    "g":{"opcode":"motion_gotoxy","next":"s","parent":"h","inputs":{"X":[1,[4,"1000"]],"Y":[1,[4,"0"]]},"fields":{},"topLevel":false},
    "s":{"opcode":"looks_setsizeto","next":"s2","parent":"g","inputs":{"SIZE":[1,[4,"1000"]]},"fields":{},"topLevel":false},
    "s2":{"opcode":"looks_changesizeby","next":"t","parent":"s","inputs":{"CHANGE":[1,[4,"-2000"]]},"fields":{},"topLevel":false},
    "t":{"opcode":"motion_turnright","next":"f","parent":"s2","inputs":{"DEGREES":[1,[4,"45"]]},"fields":{},"topLevel":false},
    "f":{"opcode":"looks_gotofrontback","next":"e","parent":"t","inputs":{},"fields":{"FRONT_BACK":["front",null]},"topLevel":false},
    "e":{"opcode":"looks_seteffectto","next":"n","parent":"f","inputs":{"VALUE":[1,[4,"150"]]},"fields":{"EFFECT":["GHOST",null]},"topLevel":false},
    "n":{"opcode":"looks_nextcostume","next":"hide","parent":"e","inputs":{},"fields":{},"topLevel":false},
    "hide":{"opcode":"looks_hide","next":null,"parent":"n","inputs":{},"fields":{},"topLevel":false}}},
  {"isStage":false,"name":"Dog","layerOrder":2,"x":10,"y":20,"direction":45,"size":50,"costumes":[{"name":"d"}]}
]}"#;

struct Recorder {
    changes: Vec<i64>,
}

unsafe extern "C" fn sprite_changed(data: *mut c_void, _rt: *mut OpaqueRuntime, target: i64) {
    // SAFETY: `data` is the `Recorder` the test registered.
    unsafe { (*data.cast::<Recorder>()).changes.push(target) };
}

/// Every sprite is drawn as a 100 by 100 square around its position, whatever its size.
unsafe extern "C" fn measure(_data: *mut c_void, rt: *mut OpaqueRuntime, target: i64, what: i32, out: *mut f64) -> i8 {
    // SAFETY: the runtime passes a live runtime and four doubles to fill.
    unsafe {
        let mut sprite = Sprite { ..std::mem::zeroed() };
        if api::thatch_runtime_sprite(rt, target, &raw mut sprite) == 0 {
            return 0;
        }
        let values = if what == MEASURE_COSTUME_SIZE {
            [100.0, 100.0, 0.0, 0.0]
        } else {
            [sprite.x - 50.0, sprite.x + 50.0, sprite.y - 50.0, sprite.y + 50.0]
        };
        for (i, v) in values.iter().enumerate() {
            *out.add(i) = *v;
        }
        1
    }
}

fn compile() -> thatch_codegen::JitProgram {
    let project = thatch_sb3::Project::from_json(PROJECT).expect("the project parses");
    let lowered = thatch_scratch::lower(&project).expect("the project lowers");
    assert!(lowered.warnings.is_empty(), "{:?}", lowered.warnings);
    thatch_codegen::compile_jit(&lowered.program, &thatch_runtime::symbols()).expect("the project compiles")
}

/// Runs the project for one frame under `host` and returns the Cat, the draw order and whether a
/// redraw was requested.
fn run_frame(host: &Host, abi_version: u32, init: InitFn) -> (Sprite, Vec<i64>, bool, Sprite) {
    // SAFETY: the host table outlives the runtime, the init function matches the ABI version,
    // and the runtime is only used between frames.
    unsafe {
        let rt = api::thatch_runtime_create(thatch_abi::ABI_VERSION as i32, host, 1);
        assert_eq!(api::thatch_runtime_load(rt, abi_version as i32, init), thatch_abi::LOAD_OK);
        // The first frame draws what was loaded; the green flag comes after it.
        assert_eq!(api::thatch_runtime_frame(rt), 0);
        assert_eq!(api::thatch_runtime_redraw_requested(rt), 1, "a loaded program is drawn once");
        assert_eq!(api::thatch_runtime_target_count(rt), 3);
        assert_eq!(api::thatch_runtime_costume_count(rt, 1), 2);
        let mut len = 0;
        let name = api::thatch_runtime_costume_name(rt, 1, 1, &raw mut len);
        assert_eq!(std::slice::from_raw_parts(name.cast::<u8>(), len), b"b");
        assert!(api::thatch_runtime_costume_name(rt, 1, 2, &raw mut len).is_null());

        let mut dog = Sprite { ..std::mem::zeroed() };
        assert_eq!(api::thatch_runtime_sprite(rt, 2, &raw mut dog), 1);
        assert_eq!((dog.x, dog.y, dog.direction, dog.size), (10.0, 20.0, 45.0, 50.0));
        assert_eq!(dog.visible, 1);

        api::thatch_runtime_green_flag(rt);
        assert_eq!(api::thatch_runtime_frame(rt), 0);
        let redraw = api::thatch_runtime_redraw_requested(rt) != 0;
        let mut cat = Sprite { ..std::mem::zeroed() };
        assert_eq!(api::thatch_runtime_sprite(rt, 1, &raw mut cat), 1);
        let mut order = [0i64; 3];
        assert_eq!(api::thatch_runtime_draw_order(rt, order.as_mut_ptr(), 3), 3);
        assert_eq!(api::thatch_runtime_sprite(rt, 3, &raw mut cat), 0, "out of range");
        api::thatch_runtime_destroy(rt);
        (cat, order.to_vec(), redraw, dog)
    }
}

#[test]
fn a_host_that_draws_gets_fenced_and_clamped_sprites() {
    let compiled = compile();
    // SAFETY: the compiler produced this init function for this ABI.
    let init: InitFn = unsafe { std::mem::transmute::<*const u8, InitFn>(compiled.init_address()) };
    let mut recorder = Recorder { changes: Vec::new() };
    let host = Host {
        data: ptr::addr_of_mut!(recorder).cast::<c_void>(),
        say: None,
        now_ms: None,
        sprite_changed: Some(sprite_changed),
        measure: Some(measure),
    };
    let (cat, order, redraw, _) = run_frame(&host, compiled.abi_version(), init);
    // Fenced: a 100 pixel square may leave at most 15 pixels of itself on the stage.
    assert_eq!((cat.x, cat.y), (275.0, 0.0));
    // 1000 percent clamps to one and a half stage heights of a 100 pixel costume, then shrinking
    // by 2000 clamps to 5 pixels.
    assert_eq!(cat.size, 5.0);
    assert_eq!(cat.direction, 135.0);
    assert_eq!(cat.costume, 1);
    assert_eq!(cat.effects[6], 100.0, "ghost is clamped to 100");
    assert_eq!(cat.visible, 0);
    assert_eq!(order, [0, 2, 1], "the Cat moved in front of the Dog");
    assert!(redraw, "moving a visible sprite asks for a redraw when the host draws");
    // The green flag clears every target's effects first, as scratch-vm's stopAll does.
    assert_eq!(recorder.changes[..3], [0, 1, 2]);
    assert!(recorder.changes[3..].iter().all(|t| *t == 1), "{:?}", recorder.changes);
    assert_eq!(
        recorder.changes.len() - 3,
        8,
        "one callback per change: move, two sizes, turn, layer, effect, costume, hide"
    );
}

/// The Cat bounces off the right edge and says something; the Dog moves for as long as a
/// broadcast keeps it going.
const BOUNCE: &str = r#"{"targets":[
  {"isStage":true,"name":"Stage","costumes":[{"name":"backdrop1"}]},
  {"isStage":false,"name":"Cat","layerOrder":1,"costumes":[{"name":"a"}],"blocks":{
    "h":{"opcode":"event_whenflagclicked","next":"g","parent":null,"inputs":{},"fields":{},"topLevel":true},
    "g":{"opcode":"motion_gotoxy","next":"b","parent":"h","inputs":{"X":[1,[4,"300"]],"Y":[1,[4,"0"]]},"fields":{},"topLevel":false},
    "b":{"opcode":"motion_ifonedgebounce","next":"s","parent":"g","inputs":{},"fields":{},"topLevel":false},
    "s":{"opcode":"looks_say","next":null,"parent":"b","inputs":{"MESSAGE":[1,[10,"hi"]]},"fields":{},"topLevel":false}}},
  {"isStage":false,"name":"Dog","layerOrder":2,"costumes":[{"name":"d"}],"blocks":{
    "r":{"opcode":"event_whenbroadcastreceived","next":"f","parent":null,"inputs":{},"fields":{"BROADCAST_OPTION":["go","b1"]},"topLevel":true},
    "f":{"opcode":"control_forever","next":null,"parent":"r","inputs":{"SUBSTACK":[2,"c"]},"fields":{},"topLevel":false},
    "c":{"opcode":"motion_changeyby","next":null,"parent":"f","inputs":{"DY":[1,[4,"1"]]},"fields":{},"topLevel":false}}}
]}"#;

fn sprite_of(rt: *mut OpaqueRuntime, target: i64) -> Sprite {
    // SAFETY: the tests pass a live runtime between frames.
    unsafe {
        let mut sprite = Sprite { ..std::mem::zeroed() };
        assert_eq!(api::thatch_runtime_sprite(rt, target, &raw mut sprite), 1);
        sprite
    }
}

/// A stage with a click hat, a sprite that is clicked as the button goes down, a draggable sprite
/// that is clicked as it comes up, and a key hat.
const CLICKS: &str = r#"{"targets":[
  {"isStage":true,"name":"Stage","costumes":[{"name":"backdrop1"}],"blocks":{
    "h":{"opcode":"event_whenstageclicked","next":"s","parent":null,"inputs":{},"fields":{},"topLevel":true},
    "s":{"opcode":"looks_say","next":null,"parent":"h","inputs":{"MESSAGE":[1,[10,"stage"]]},"fields":{},"topLevel":false},
    "a":{"opcode":"event_whenkeypressed","next":"as","parent":null,"inputs":{},"fields":{"KEY_OPTION":["any",null]},"topLevel":true},
    "as":{"opcode":"looks_say","next":null,"parent":"a","inputs":{"MESSAGE":[1,[10,"any"]]},"fields":{},"topLevel":false}}},
  {"isStage":false,"name":"Cat","layerOrder":1,"costumes":[{"name":"a"}],"blocks":{
    "h":{"opcode":"event_whenthisspriteclicked","next":"s","parent":null,"inputs":{},"fields":{},"topLevel":true},
    "s":{"opcode":"looks_say","next":null,"parent":"h","inputs":{"MESSAGE":[1,[10,"cat"]]},"fields":{},"topLevel":false},
    "k":{"opcode":"event_whenkeypressed","next":"ks","parent":null,"inputs":{},"fields":{"KEY_OPTION":["left arrow",null]},"topLevel":true},
    "ks":{"opcode":"looks_say","next":null,"parent":"k","inputs":{"MESSAGE":[1,[10,"left"]]},"fields":{},"topLevel":false},
    "r":{"opcode":"event_whenkeypressed","next":"rs","parent":null,"inputs":{},"fields":{"KEY_OPTION":["right arrow",null]},"topLevel":true},
    "rs":{"opcode":"looks_say","next":"rf","parent":"r","inputs":{"MESSAGE":[1,[10,"right"]]},"fields":{},"topLevel":false},
    "rf":{"opcode":"control_forever","next":null,"parent":"rs","inputs":{},"fields":{},"topLevel":false},
    "x":{"opcode":"event_whenkeypressed","next":"xs","parent":null,"inputs":{},"fields":{"KEY_OPTION":["s",null]},"topLevel":true},
    "xs":{"opcode":"control_stop","next":null,"parent":"x","inputs":{},"fields":{"STOP_OPTION":["other scripts in sprite",null]},"topLevel":false,
      "mutation":{"tagName":"mutation","children":[],"hasnext":"true"}}}},
  {"isStage":false,"name":"Dog","layerOrder":2,"draggable":true,"costumes":[{"name":"d"}],"blocks":{
    "h":{"opcode":"event_whenthisspriteclicked","next":"s","parent":null,"inputs":{},"fields":{},"topLevel":true},
    "s":{"opcode":"looks_say","next":null,"parent":"h","inputs":{"MESSAGE":[1,[10,"dog"]]},"fields":{},"topLevel":false}}}
]}"#;

unsafe extern "C" fn record_say(
    data: *mut c_void,
    _rt: *mut OpaqueRuntime,
    _target: i64,
    text: *const u8,
    len: usize,
    _think: i8,
) {
    // A cleared bubble, as the headless host ignores it.
    if len == 0 {
        return;
    }
    // SAFETY: `data` is the `Vec<String>` the test registered; the runtime passes `len` bytes of UTF-8.
    unsafe {
        let said = &mut *data.cast::<Vec<String>>();
        said.push(String::from_utf8_lossy(std::slice::from_raw_parts(text, len)).into_owned());
    }
}

#[test]
fn clicks_and_keys_start_the_right_hats() {
    let project = thatch_sb3::Project::from_json(CLICKS).expect("the project parses");
    let lowered = thatch_scratch::lower(&project).expect("the project lowers");
    assert!(lowered.warnings.is_empty(), "{:?}", lowered.warnings);
    let compiled =
        thatch_codegen::compile_jit(&lowered.program, &thatch_runtime::symbols()).expect("the project compiles");
    // SAFETY: as above.
    let init: InitFn = unsafe { std::mem::transmute::<*const u8, InitFn>(compiled.init_address()) };
    let mut said: Vec<String> = Vec::new();
    let host = Host {
        data: ptr::addr_of_mut!(said).cast::<c_void>(),
        say: Some(record_say),
        now_ms: None,
        sprite_changed: None,
        measure: None,
    };
    // SAFETY: the host table outlives the runtime, and the runtime is used between frames only.
    unsafe {
        let rt = api::thatch_runtime_create(thatch_abi::ABI_VERSION as i32, &raw const host, 1);
        assert_eq!(api::thatch_runtime_load(rt, compiled.abi_version() as i32, init), thatch_abi::LOAD_OK);
        assert_eq!(api::thatch_runtime_target_draggable(rt, 1), 0);
        assert_eq!(api::thatch_runtime_target_draggable(rt, 2), 1);
        let frame = |rt| assert_eq!(api::thatch_runtime_frame(rt), 0);

        // The Cat is clicked as the button goes down, and only once per press: a second report of
        // the button down, after the first click ran, clicks nothing.
        api::thatch_runtime_mouse_button(rt, 1, 1, 0);
        frame(rt);
        api::thatch_runtime_mouse_button(rt, 1, 1, 0);
        frame(rt);
        api::thatch_runtime_mouse_button(rt, 0, 1, 0);
        frame(rt);
        assert_eq!(said, ["cat"]);
        // The draggable Dog is clicked as the button comes up, but not at the end of a drag.
        api::thatch_runtime_mouse_button(rt, 1, 2, 0);
        frame(rt);
        assert_eq!(said, ["cat"]);
        api::thatch_runtime_mouse_button(rt, 0, 2, 0);
        frame(rt);
        assert_eq!(said, ["cat", "dog"]);
        api::thatch_runtime_mouse_button(rt, 1, 2, 0);
        api::thatch_runtime_mouse_button(rt, 0, 2, 1);
        frame(rt);
        assert_eq!(said, ["cat", "dog"]);
        // A press on the stage starts its hat; one outside the stage starts nothing.
        api::thatch_runtime_mouse_button(rt, 1, 0, 0);
        frame(rt);
        api::thatch_runtime_mouse_button(rt, 0, -1, 0);
        api::thatch_runtime_mouse_button(rt, 1, -1, 0);
        frame(rt);
        assert_eq!(said, ["cat", "dog", "stage"]);
        // Keys by their browser names. A modifier is no key at all, so it starts not even the
        // `any` hat; a real key starts its own hat and then the stage's `any` hat.
        api::thatch_runtime_mouse_button(rt, 0, -1, 0);
        api::thatch_runtime_key(rt, c"Shift".as_ptr(), 5, 1);
        frame(rt);
        assert_eq!(said, ["cat", "dog", "stage"]);
        api::thatch_runtime_key(rt, c"ArrowLeft".as_ptr(), 9, 1);
        frame(rt);
        assert_eq!(said, ["cat", "dog", "stage", "left", "any"]);
        said.clear();
        // A key hat leaves its running script alone, but one the host stopped starts again even
        // before a frame has let the stopped one finish.
        api::thatch_runtime_key(rt, c"ArrowRight".as_ptr(), 10, 1);
        frame(rt);
        api::thatch_runtime_key(rt, c"ArrowRight".as_ptr(), 10, 1);
        frame(rt);
        assert_eq!(said, ["right", "any", "any"]);
        api::thatch_runtime_stop_all(rt);
        api::thatch_runtime_key(rt, c"ArrowRight".as_ptr(), 10, 1);
        frame(rt);
        assert_eq!(said, ["right", "any", "any", "right", "any"]);
        // `stop other scripts in sprite` kills the running right-arrow script after it has stepped
        // in this frame, so it waits for the next one to finish. Two presses before then start one
        // new copy, not one per press.
        said.clear();
        api::thatch_runtime_key(rt, c"s".as_ptr(), 1, 1);
        frame(rt);
        api::thatch_runtime_key(rt, c"ArrowRight".as_ptr(), 10, 1);
        api::thatch_runtime_key(rt, c"ArrowRight".as_ptr(), 10, 1);
        frame(rt);
        assert_eq!(said, ["any", "right", "any"]);
        api::thatch_runtime_mouse_move(rt, 1000.0, -33.5);
        api::thatch_runtime_destroy(rt);
    }
}

#[test]
fn bouncing_the_green_flag_and_stopping_from_the_host() {
    let project = thatch_sb3::Project::from_json(BOUNCE).expect("the project parses");
    let lowered = thatch_scratch::lower(&project).expect("the project lowers");
    assert!(lowered.warnings.is_empty(), "{:?}", lowered.warnings);
    let compiled =
        thatch_codegen::compile_jit(&lowered.program, &thatch_runtime::symbols()).expect("the project compiles");
    // SAFETY: as above.
    let init: InitFn = unsafe { std::mem::transmute::<*const u8, InitFn>(compiled.init_address()) };
    let mut recorder = Recorder { changes: Vec::new() };
    let host = Host {
        data: ptr::addr_of_mut!(recorder).cast::<c_void>(),
        say: None,
        now_ms: None,
        sprite_changed: Some(sprite_changed),
        measure: Some(measure),
    };
    // SAFETY: the host table outlives the runtime, and the runtime is used between frames only.
    unsafe {
        let rt = api::thatch_runtime_create(thatch_abi::ABI_VERSION as i32, &raw const host, 1);
        assert_eq!(api::thatch_runtime_load(rt, compiled.abi_version() as i32, init), thatch_abi::LOAD_OK);
        let frame = |rt| assert_eq!(api::thatch_runtime_frame(rt), 0);
        let go = "go";
        api::thatch_runtime_start_hats(rt, thatch_abi::HAT_BROADCAST, go.as_ptr().cast(), go.len());
        frame(rt);
        assert!(sprite_of(rt, 2).y >= 1.0);
        assert_eq!(api::thatch_runtime_has_threads(rt), 1);

        // The green flag stops the Dog's loop before it starts the Cat.
        api::thatch_runtime_green_flag(rt);
        let dog_y = sprite_of(rt, 2).y;
        frame(rt);
        assert_eq!(sprite_of(rt, 2).y, dog_y, "the Dog stopped");
        assert_eq!(api::thatch_runtime_has_threads(rt), 0);
        // Fenced at 275, then bounced off the right edge: turned around and pushed back onto the
        // stage, as scratch-vm's ifOnEdgeBounce does with a renderer.
        let cat = sprite_of(rt, 1);
        assert_eq!((cat.x, cat.y, cat.direction), (190.0, 0.0, -90.0));

        // A frame in which nothing happens asks for no drawing; a stop from the host between
        // frames, which clears the Cat's bubble and everyone's effects, is drawn after the next.
        frame(rt);
        assert_eq!(api::thatch_runtime_redraw_requested(rt), 0);
        api::thatch_runtime_stop_all(rt);
        assert_eq!(api::thatch_runtime_redraw_requested(rt), 1);
        frame(rt);
        assert_eq!(api::thatch_runtime_redraw_requested(rt), 1);
        frame(rt);
        assert_eq!(api::thatch_runtime_redraw_requested(rt), 0);

        // Stopped threads stop counting at once.
        api::thatch_runtime_start_hats(rt, thatch_abi::HAT_BROADCAST, go.as_ptr().cast(), go.len());
        frame(rt);
        assert_eq!(api::thatch_runtime_has_threads(rt), 1);
        api::thatch_runtime_stop_all(rt);
        assert_eq!(api::thatch_runtime_has_threads(rt), 0);
        api::thatch_runtime_destroy(rt);
    }
}

#[test]
fn a_host_that_does_not_draw_gets_what_headless_scratch_does() {
    let compiled = compile();
    // SAFETY: as above.
    let init: InitFn = unsafe { std::mem::transmute::<*const u8, InitFn>(compiled.init_address()) };
    let host = Host { data: ptr::null_mut(), say: None, now_ms: None, sprite_changed: None, measure: None };
    let (cat, order, redraw, _) = run_frame(&host, compiled.abi_version(), init);
    assert_eq!((cat.x, cat.y), (1000.0, 0.0), "nothing fences a sprite nobody draws");
    assert_eq!(cat.size, -1000.0, "nothing clamps it either");
    assert_eq!(order, [0, 2, 1]);
    assert!(!redraw, "motion requests no redraw without a renderer, as in scratch-vm");
}

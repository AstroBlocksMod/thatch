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
    assert!(recorder.changes.iter().all(|t| *t == 1));
    assert_eq!(
        recorder.changes.len(),
        8,
        "one callback per change: move, two sizes, turn, layer, effect, costume, hide"
    );
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

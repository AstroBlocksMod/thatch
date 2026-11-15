//! Keyboard and mouse input, following scratch-vm's `io/keyboard.js` and `io/mouse.js`.
//!
//! The host reports what happened (a key went down, the mouse moved, the button went down over a
//! target) through the C API; the runtime keeps the state the sensing blocks read and starts the
//! hats scratch-vm would start. Which target is under the mouse is the host's business, since only
//! it knows what it drew.

use thatch_abi::{HAT_KEY, HAT_SPRITE_CLICKED, HAT_STAGE_CLICKED, STAGE_HEIGHT, STAGE_WIDTH};
use thatch_values::{ValueRef, clamp, round, to_string};

use crate::scheduler::Runtime;

/// The keys Scratch names rather than spells.
const KEY_NAMES: [&str; 6] = ["space", "left arrow", "up arrow", "right arrow", "down arrow", "enter"];

/// `_keyStringToScratchKey`: the Scratch name of a key a host reports by its `KeyboardEvent.key`
/// name. Scratch's own names are accepted too. None for keys Scratch ignores: anything else longer
/// than one UTF-16 unit, such as modifier keys, and the empty string.
pub fn key_string_to_scratch_key(key: &str) -> Option<String> {
    let named = match key {
        " " => "space",
        "ArrowLeft" | "Left" => "left arrow",
        "ArrowUp" | "Up" => "up arrow",
        "ArrowRight" | "Right" => "right arrow",
        "ArrowDown" | "Down" => "down arrow",
        "Enter" => "enter",
        k if KEY_NAMES.contains(&k) => k,
        _ => {
            if key.encode_utf16().count() != 1 {
                return None;
            }
            return Some(key.to_uppercase());
        }
    };
    Some(named.to_owned())
}

/// `_keyArgToScratchKey`: the Scratch name of the key a `key pressed?` argument means. A number
/// from 48 to 90 is a key code of a digit, some punctuation or a letter; 32 and 37 through 40 are
/// space and the arrows. Anything else is text, of which only the first character counts.
pub fn key_arg_to_scratch_key(key: ValueRef<'_>) -> String {
    if let ValueRef::Num(n) = key {
        if (48.0..=90.0).contains(&n) {
            // `String.fromCharCode` takes the code modulo 2^16, so a fraction is dropped.
            return char::from_u32(n as u32).map(String::from).unwrap_or_default();
        }
        const CODES: [(f64, &str); 5] =
            [(32.0, "space"), (37.0, "left arrow"), (38.0, "up arrow"), (39.0, "right arrow"), (40.0, "down arrow")];
        if let Some((_, name)) = CODES.iter().find(|(code, _)| *code == n) {
            return (*name).to_owned();
        }
    }
    let mut text = to_string(key);
    if KEY_NAMES.contains(&text.as_str()) {
        return text;
    }
    if text.encode_utf16().count() > 1 {
        // `keyArg[0]`: the first UTF-16 unit. Half of a character outside the Basic Multilingual
        // Plane would be a lone surrogate, which matches no key; the whole character does not
        // match one either, since such keys are ignored when pressed.
        text = text.chars().next().map(String::from).unwrap_or_default();
    }
    if text == " " {
        return "space".to_owned();
    }
    text.to_uppercase()
}

impl Runtime {
    /// A key went down or up. Going down starts the hats for the key and for "any" before the key
    /// counts as pressed, as scratch-vm does; the threads run in the next frame either way.
    pub fn key_event(&mut self, key: &str, down: bool) {
        let Some(scratch_key) = key_string_to_scratch_key(key) else { return };
        let index = self.keys_pressed.iter().position(|k| *k == scratch_key);
        if down {
            self.start_hats(HAT_KEY, &scratch_key);
            self.start_hats(HAT_KEY, "any");
            if index.is_none() {
                self.keys_pressed.push(scratch_key);
            }
        } else if let Some(i) = index {
            self.keys_pressed.remove(i);
        }
    }

    /// The mouse moved to a point in Scratch coordinates, which scratch-vm rounds and clamps to the
    /// stage. A NaN coordinate is ignored and the old one kept, as scratch-vm's mouse skips a
    /// coordinate that is not truthy, so the mouse position is never NaN.
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        if !x.is_nan() {
            self.mouse_x = round(clamp(x, -STAGE_WIDTH / 2.0, STAGE_WIDTH / 2.0));
        }
        if !y.is_nan() {
            self.mouse_y = round(clamp(y, -STAGE_HEIGHT / 2.0, STAGE_HEIGHT / 2.0));
        }
    }

    /// The mouse button went down or up over `target` (None outside the stage). A target that is
    /// not draggable is clicked as the button goes down, a draggable one as it comes up; both click
    /// hats on that target start then. A repeated report of the same state does nothing, and
    /// neither does a release that ends a drag (`dragged`), as in scratch-vm's mouse.
    pub fn mouse_button(&mut self, down: bool, target: Option<usize>, dragged: bool) {
        let was_down = self.mouse_down;
        self.mouse_down = down;
        if was_down == down || dragged {
            return;
        }
        let Some(target) = target.filter(|t| *t < self.targets.len()) else { return };
        let draggable = self.targets[target].draggable;
        let clicked = if draggable { was_down && !down } else { !was_down && down };
        if clicked {
            self.start_hats_in(HAT_SPRITE_CLICKED, "", Some(target));
            self.start_hats_in(HAT_STAGE_CLICKED, "", Some(target));
        }
    }

    /// `key pressed?`.
    pub fn key_pressed(&self, key: ValueRef<'_>) -> bool {
        if key == ValueRef::Str("any") {
            return !self.keys_pressed.is_empty();
        }
        let wanted = key_arg_to_scratch_key(key);
        self.keys_pressed.contains(&wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_follow_scratch() {
        assert_eq!(key_string_to_scratch_key(" ").as_deref(), Some("space"));
        assert_eq!(key_string_to_scratch_key("ArrowLeft").as_deref(), Some("left arrow"));
        assert_eq!(key_string_to_scratch_key("left arrow").as_deref(), Some("left arrow"));
        assert_eq!(key_string_to_scratch_key("a").as_deref(), Some("A"));
        assert_eq!(key_string_to_scratch_key("é").as_deref(), Some("É"));
        assert_eq!(key_string_to_scratch_key("Shift"), None);
        assert_eq!(key_string_to_scratch_key(""), None);
        assert_eq!(key_string_to_scratch_key("😀"), None);
    }

    #[test]
    fn the_mouse_ignores_nan_and_stays_on_the_stage() {
        let host = thatch_abi::Host {
            data: std::ptr::null_mut(),
            say: None,
            now_ms: None,
            sprite_changed: None,
            measure: None,
        };
        let rt = Runtime::create(host, 1);
        // SAFETY: a runtime of this test only, with no threads and no callbacks.
        unsafe {
            (*rt).mouse_move(10.4, 1000.0);
            assert_eq!(((*rt).mouse_x, (*rt).mouse_y), (10.0, 180.0));
            (*rt).mouse_move(f64::NAN, -20.0);
            assert_eq!(((*rt).mouse_x, (*rt).mouse_y), (10.0, -20.0));
            (*rt).mouse_move(5.0, f64::NAN);
            assert_eq!(((*rt).mouse_x, (*rt).mouse_y), (5.0, -20.0));
            Runtime::destroy(rt);
        }
    }

    #[test]
    fn key_arguments_follow_scratch() {
        assert_eq!(key_arg_to_scratch_key(ValueRef::Str("space")), "space");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Str("a")), "A");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Str("abc")), "A");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Str(" ")), "space");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Num(65.0)), "A");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Num(48.0)), "0");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Num(32.0)), "space");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Num(37.0)), "left arrow");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Num(5.0)), "5");
        assert_eq!(key_arg_to_scratch_key(ValueRef::Bool(true)), "T");
    }
}

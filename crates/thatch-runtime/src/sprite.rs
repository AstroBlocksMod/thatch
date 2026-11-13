//! Sprite state: position, direction, size, costume, visibility, graphic effects and layer order,
//! and the motion and looks blocks that change them, following scratch-vm's `RenderedTarget` and
//! its motion and looks primitives.
//!
//! scratch-vm behaves differently with and without a renderer, and both matter here. The headless
//! host has none, like the oracle the samples are checked against: positions are not fenced to the
//! stage, sizes are not clamped, `if on edge, bounce` does nothing and no motion asks for a redraw.
//! A host that draws gives the runtime a `sprite_changed` callback, which turns the redraw requests
//! on, and a `measure` callback, which tells the runtime how big a sprite is drawn so it can fence,
//! clamp and bounce exactly as scratch-vm does with its renderer.

use std::rc::Rc;

use thatch_abi::{
    EFFECT_COUNT, MEASURE_BOUNDS, MEASURE_BOX, MEASURE_COSTUME_SIZE, STAGE_HEIGHT, STAGE_WIDTH, Sprite, Value,
};
use thatch_values::{
    ValueRef, clamp, deg_to_rad, is_whitespace, js_number, rad_to_deg, round, wrap_clamp, wrap_direction,
};

use crate::scheduler::Runtime;

/// How close to the edge a sprite may be dragged or moved: `FENCE_WIDTH` in scratch-render.
const FENCE_WIDTH: f64 = 15.0;

/// The index of the ghost effect in `effects`, which is clamped to 0 through 100.
const GHOST: usize = 6;
/// The index of the brightness effect, clamped to -100 through 100.
const BRIGHTNESS: usize = 5;

/// A rectangle in Scratch coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
}

/// What `property of` reports.
pub enum PropertyValue {
    Num(f64),
    Str(Rc<String>),
    /// A variable's value, which the caller must copy with its string retained.
    Var(Value),
}

impl Runtime {
    /// The first sprite with this name, as `getSpriteTargetByName` finds it.
    pub fn sprite_by_name(&self, name: &str) -> Option<usize> {
        self.targets.iter().position(|t| !t.is_stage && t.name == name)
    }

    /// How a target looks, for the host.
    pub fn sprite(&self, target: usize) -> Option<Sprite> {
        let t = self.targets.get(target)?;
        Some(Sprite {
            x: t.x,
            y: t.y,
            direction: t.direction,
            size: t.size,
            costume: t.costume as i64,
            rotation_style: t.rotation_style as i32,
            visible: i8::from(t.visible),
            _reserved: [0; 3],
            effects: t.effects,
        })
    }

    /// Tells the host a target looks different and, when the host draws and `redraw` holds (the
    /// target is visible, for most changes), asks for a redraw, as scratch-vm does with a renderer.
    fn changed(&mut self, target: usize, redraw: bool) {
        if let Some(cb) = self.host.sprite_changed {
            // SAFETY: the host gave this callback and its data for the runtime to call; the runtime
            // pointer is the one this runtime lives behind.
            unsafe { cb(self.host.data, self.opaque(), target as i64) };
            if redraw {
                self.request_redraw();
            }
        }
    }

    /// Asks the host to measure a sprite as it is drawn. None without a host that draws.
    fn measure(&self, target: usize, what: i32) -> Option<[f64; 4]> {
        let cb = self.host.measure?;
        let mut out = [0.0; 4];
        // SAFETY: as `changed`; `out` is four doubles the callback may fill.
        let filled = unsafe { cb(self.host.data, self.opaque(), target as i64, what, out.as_mut_ptr()) };
        (filled != 0).then_some(out)
    }

    fn bounds(&self, target: usize, what: i32) -> Option<Bounds> {
        let out = self.measure(target, what)?;
        Some(Bounds { left: out[0], right: out[1], bottom: out[2], top: out[3] })
    }

    // ---- motion -------------------------------------------------------------------------------

    /// `setXY`: moves a sprite, fenced to the stage when the host can measure it.
    pub fn set_xy(&mut self, target: usize, x: f64, y: f64) {
        if self.targets[target].is_stage {
            return;
        }
        let (px, py) = (self.targets[target].x, self.targets[target].y);
        let (x, y) = match self.bounds(target, MEASURE_BOX) {
            Some(aabb) => fenced(px, py, aabb, x, y),
            None => (x, y),
        };
        let t = &mut self.targets[target];
        t.x = x;
        t.y = y;
        let visible = t.visible;
        self.changed(target, visible);
    }

    /// `setDirection`: wraps into -179 through 180 and ignores NaN and infinities.
    pub fn set_direction(&mut self, target: usize, direction: f64) {
        let t = &mut self.targets[target];
        if t.is_stage || !direction.is_finite() {
            return;
        }
        t.direction = wrap_direction(direction);
        let visible = t.visible;
        self.changed(target, visible);
    }

    pub fn move_steps(&mut self, target: usize, steps: f64) {
        let t = &self.targets[target];
        let radians = deg_to_rad(90.0 - t.direction);
        let dx = steps * radians.cos();
        let dy = steps * radians.sin();
        let (x, y) = (t.x, t.y);
        self.set_xy(target, x + dx, y + dy);
    }

    pub fn change_x(&mut self, target: usize, dx: f64) {
        let (x, y) = (self.targets[target].x, self.targets[target].y);
        self.set_xy(target, x + dx, y);
    }

    pub fn change_y(&mut self, target: usize, dy: f64) {
        let (x, y) = (self.targets[target].x, self.targets[target].y);
        self.set_xy(target, x, y + dy);
    }

    pub fn set_x(&mut self, target: usize, x: f64) {
        let y = self.targets[target].y;
        self.set_xy(target, x, y);
    }

    pub fn set_y(&mut self, target: usize, y: f64) {
        let x = self.targets[target].x;
        self.set_xy(target, x, y);
    }

    /// `getTargetXY`: where `go to` and `glide to` go. None for a sprite that does not exist.
    pub fn target_xy(&mut self, name: &str) -> Option<(f64, f64)> {
        match name {
            "_mouse_" => Some((self.mouse_x, self.mouse_y)),
            "_random_" => {
                let x = round(STAGE_WIDTH * (self.random() - 0.5));
                let y = round(STAGE_HEIGHT * (self.random() - 0.5));
                Some((x, y))
            }
            _ => {
                let i = self.sprite_by_name(name)?;
                Some((self.targets[i].x, self.targets[i].y))
            }
        }
    }

    pub fn go_to(&mut self, target: usize, name: &str) {
        if let Some((x, y)) = self.target_xy(name) {
            self.set_xy(target, x, y);
        }
    }

    pub fn point_towards(&mut self, target: usize, name: &str) {
        let (tx, ty) = match name {
            "_mouse_" => (self.mouse_x, self.mouse_y),
            "_random_" => {
                let direction = round(self.random() * 360.0) - 180.0;
                self.set_direction(target, direction);
                return;
            }
            _ => {
                let Some(i) = self.sprite_by_name(name) else { return };
                (self.targets[i].x, self.targets[i].y)
            }
        };
        let t = &self.targets[target];
        let dx = tx - t.x;
        let dy = ty - t.y;
        let direction = 90.0 - rad_to_deg(dy.atan2(dx));
        self.set_direction(target, direction);
    }

    /// `if on edge, bounce`. Needs the sprite's drawn bounds, so it does nothing headless.
    pub fn bounce(&mut self, target: usize) {
        let Some(bounds) = self.bounds(target, MEASURE_BOUNDS) else { return };
        // Distances to the edges: positive away from an edge, zero at or beyond it.
        let dist_left = js_max(0.0, STAGE_WIDTH / 2.0 + bounds.left);
        let dist_top = js_max(0.0, STAGE_HEIGHT / 2.0 - bounds.top);
        let dist_right = js_max(0.0, STAGE_WIDTH / 2.0 - bounds.right);
        let dist_bottom = js_max(0.0, STAGE_HEIGHT / 2.0 + bounds.bottom);
        let mut nearest = Edge::None;
        let mut min = f64::INFINITY;
        for (dist, edge) in
            [(dist_left, Edge::Left), (dist_top, Edge::Top), (dist_right, Edge::Right), (dist_bottom, Edge::Bottom)]
        {
            if dist < min {
                min = dist;
                nearest = edge;
            }
        }
        if min > 0.0 {
            return;
        }
        let radians = deg_to_rad(90.0 - self.targets[target].direction);
        let mut dx = radians.cos();
        let mut dy = -radians.sin();
        match nearest {
            Edge::Left => dx = js_max(0.2, dx.abs()),
            Edge::Top => dy = js_max(0.2, dy.abs()),
            Edge::Right => dx = 0.0 - js_max(0.2, dx.abs()),
            Edge::Bottom => dy = 0.0 - js_max(0.2, dy.abs()),
            Edge::None => {}
        }
        let direction = rad_to_deg(dy.atan2(dx)) + 90.0;
        self.set_direction(target, direction);
        // `keepInFence` at the current position: push the (now rotated) bounds back onto the stage.
        let Some(b) = self.bounds(target, MEASURE_BOUNDS) else { return };
        let (half_w, half_h) = (STAGE_WIDTH / 2.0, STAGE_HEIGHT / 2.0);
        let mut dx = 0.0;
        let mut dy = 0.0;
        if b.left < -half_w {
            dx += -half_w - b.left;
        }
        if b.right > half_w {
            dx += half_w - b.right;
        }
        if b.top > half_h {
            dy += half_h - b.top;
        }
        if b.bottom < -half_h {
            dy += -half_h - b.bottom;
        }
        let (x, y) = (self.targets[target].x, self.targets[target].y);
        self.set_xy(target, x + dx, y + dy);
    }

    pub fn set_rotation_style(&mut self, target: usize, style: i64) {
        let t = &mut self.targets[target];
        t.rotation_style = style;
        let visible = t.visible;
        self.changed(target, visible);
    }

    // ---- looks --------------------------------------------------------------------------------

    /// `setVisible`. Only showing asks for a redraw, as in scratch-vm.
    pub fn set_visible(&mut self, target: usize, visible: bool) {
        let t = &mut self.targets[target];
        if t.is_stage {
            return;
        }
        t.visible = visible;
        self.changed(target, visible);
    }

    /// `setSize`: in percent, clamped between 5 pixels and one and a half stages when the host can
    /// measure the costume.
    pub fn set_size(&mut self, target: usize, size: f64) {
        if self.targets[target].is_stage {
            return;
        }
        let size = match self.measure(target, MEASURE_COSTUME_SIZE) {
            Some(costume) => {
                let (w, h) = (costume[0], costume[1]);
                let min_scale = js_min(1.0, js_max(5.0 / w, 5.0 / h));
                let max_scale = js_min(1.5 * STAGE_WIDTH / w, 1.5 * STAGE_HEIGHT / h);
                clamp(size / 100.0, min_scale, max_scale) * 100.0
            }
            None => size,
        };
        let t = &mut self.targets[target];
        t.size = size;
        let visible = t.visible;
        self.changed(target, visible);
    }

    /// `setCostume`: rounds the index and wraps it into the costume list.
    pub fn set_costume(&mut self, target: usize, index: f64) {
        let t = &mut self.targets[target];
        let index = round(index);
        let index = if index.is_finite() { index } else { 0.0 };
        let count = t.costumes.len();
        let wrapped = wrap_clamp(index, 0.0, count as f64 - 1.0);
        t.costume =
            if wrapped.is_finite() && wrapped >= 0.0 { (wrapped as usize).min(count.saturating_sub(1)) } else { 0 };
        let visible = t.visible;
        self.changed(target, visible);
    }

    /// `_setCostume` and `_setBackdrop`: a number is a costume number counted from 1 (or from 0 with
    /// `zero_index`); a string names a costume, or is "next costume", "previous costume", or on the
    /// backdrop block "next backdrop", "previous backdrop" or "random backdrop", or else reads as a
    /// number. The keywords follow the block, not the target, as in scratch-vm.
    pub fn switch_costume(&mut self, target: usize, requested: ValueRef<'_>, backdrop_block: bool, zero_index: bool) {
        let one = if zero_index { 0.0 } else { 1.0 };
        match requested {
            ValueRef::Num(n) => self.set_costume(target, n - one),
            ValueRef::Str(_) | ValueRef::Bool(_) => {
                let text = thatch_values::to_string(requested);
                let t = &self.targets[target];
                if let Some(i) = t.costumes.iter().position(|c| **c == text) {
                    self.set_costume(target, i as f64);
                    return;
                }
                let (next, previous) = if backdrop_block {
                    ("next backdrop", "previous backdrop")
                } else {
                    ("next costume", "previous costume")
                };
                let current = t.costume as f64;
                if text == next {
                    self.set_costume(target, current + 1.0);
                } else if text == previous {
                    self.set_costume(target, current - 1.0);
                } else if backdrop_block && text == "random backdrop" {
                    let count = t.costumes.len();
                    if count > 1 {
                        // `inclusiveRandIntWithout`: any backdrop but the current one.
                        let options = (count - 1) as f64;
                        let mut pick = (self.random() * options).floor();
                        if pick >= current {
                            pick += 1.0;
                        }
                        self.set_costume(target, pick);
                    }
                } else {
                    // `isNaN(s)` reads the string as a number; whitespace is not a number here.
                    let n = match requested {
                        ValueRef::Bool(b) => f64::from(b),
                        _ => js_number(&text),
                    };
                    let whitespace = matches!(requested, ValueRef::Str(s) if is_whitespace(s));
                    if !(n.is_nan() || whitespace) {
                        self.set_costume(target, n - one);
                    }
                }
            }
        }
    }

    pub fn set_effect(&mut self, target: usize, effect: usize, value: f64) {
        if effect >= EFFECT_COUNT {
            return;
        }
        let t = &mut self.targets[target];
        t.effects[effect] = clamp_effect(effect, value);
        let visible = t.visible;
        self.changed(target, visible);
    }

    pub fn change_effect(&mut self, target: usize, effect: usize, change: f64) {
        if effect >= EFFECT_COUNT {
            return;
        }
        let value = change + self.targets[target].effects[effect];
        self.set_effect(target, effect, value);
    }

    pub fn clear_effects(&mut self, target: usize) {
        let t = &mut self.targets[target];
        t.effects = [0.0; EFFECT_COUNT];
        let visible = t.visible;
        self.changed(target, visible);
    }

    /// `moveExecutable`: moves a sprite `delta` layers forward (positive) or back in the drawing
    /// order, which is also the reverse of the order hats start in. The stage stays at the back.
    /// scratch-vm does the arithmetic on JavaScript numbers, so a fractional or infinite `delta`
    /// behaves as `Array.prototype.splice` treats such an index.
    pub fn move_layer(&mut self, target: usize, delta: f64) {
        if self.targets[target].is_stage {
            return;
        }
        let Some(old) = self.order.iter().position(|t| *t == target) else { return };
        self.order.remove(old);
        let len = self.order.len() as f64;
        let mut index = old as f64 + delta;
        if index > len {
            index = len;
        }
        if index <= 0.0 {
            index = if self.order.first().is_some_and(|t| self.targets[*t].is_stage) { 1.0 } else { 0.0 };
        }
        let index = if index.is_nan() { 0 } else { (index.trunc() as usize).min(self.order.len()) };
        self.order.insert(index, target);
        // The renderer reorders too, but nothing asks for a redraw.
        self.changed(target, false);
    }

    pub fn go_to_layer(&mut self, target: usize, front: bool) {
        self.move_layer(target, if front { f64::INFINITY } else { f64::NEG_INFINITY });
    }

    // ---- reporters ----------------------------------------------------------------------------

    pub fn costume_name(&self, target: usize) -> Rc<String> {
        let t = &self.targets[target];
        t.costumes.get(t.costume).cloned().unwrap_or_default()
    }

    /// `property of object`.
    pub fn property_of(&self, property: &str, object: &str) -> PropertyValue {
        let target = if object == "_stage_" { Some(self.stage_index()) } else { self.sprite_by_name(object) };
        let Some(i) = target else { return PropertyValue::Num(0.0) };
        let t = &self.targets[i];
        let costume_number = PropertyValue::Num(t.costume as f64 + 1.0);
        if t.is_stage {
            match property {
                "background #" | "backdrop #" => return costume_number,
                "backdrop name" => return PropertyValue::Str(self.costume_name(i)),
                "volume" => return PropertyValue::Num(t.volume),
                _ => {}
            }
        } else {
            match property {
                "x position" => return PropertyValue::Num(t.x),
                "y position" => return PropertyValue::Num(t.y),
                "direction" => return PropertyValue::Num(t.direction),
                "costume #" => return costume_number,
                "costume name" => return PropertyValue::Str(self.costume_name(i)),
                "size" => return PropertyValue::Num(t.size),
                "volume" => return PropertyValue::Num(t.volume),
                _ => {}
            }
        }
        // Otherwise a variable of that target, by name; 0 when there is none.
        if let Some(j) = t.num_names.iter().position(|n| n == property) {
            return PropertyValue::Num(t.nums[j]);
        }
        if let Some(j) = t.any_names.iter().position(|n| n == property) {
            return PropertyValue::Var(t.anys[j]);
        }
        PropertyValue::Num(0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    None,
    Left,
    Top,
    Right,
    Bottom,
}

/// `Math.max`: NaN wins, unlike `f64::max`.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// `Math.min`: NaN wins.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// scratch-render's `getFencedPositionOfDrawable`: a sprite at `(px, py)` with bounding box `aabb`
/// wants to move to `(x, y)`; the result keeps at least 15 pixels of it (or half of it, when it is
/// smaller) on the stage.
fn fenced(px: f64, py: f64, aabb: Bounds, mut x: f64, mut y: f64) -> (f64, f64) {
    let dx = x - px;
    let dy = y - py;
    let width = aabb.right - aabb.left;
    let height = aabb.top - aabb.bottom;
    let inset = (js_min(width, height) / 2.0).floor();
    let sx = STAGE_WIDTH / 2.0 - js_min(FENCE_WIDTH, inset);
    if aabb.right + dx < -sx {
        x = (px - (sx + aabb.right)).ceil();
    } else if aabb.left + dx > sx {
        x = (px + (sx - aabb.left)).floor();
    }
    let sy = STAGE_HEIGHT / 2.0 - js_min(FENCE_WIDTH, inset);
    if aabb.top + dy < -sy {
        y = (py - (sy + aabb.top)).ceil();
    } else if aabb.bottom + dy > sy {
        y = (py + (sy - aabb.bottom)).floor();
    }
    (x, y)
}

/// The looks blocks clamp two effects before storing them.
fn clamp_effect(effect: usize, value: f64) -> f64 {
    match effect {
        GHOST => clamp(value, 0.0, 100.0),
        BRIGHTNESS => clamp(value, -100.0, 100.0),
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fencing_keeps_part_of_the_sprite_on_stage() {
        // A 100 by 100 sprite centered at the origin.
        let aabb = Bounds { left: -50.0, right: 50.0, bottom: -50.0, top: 50.0 };
        assert_eq!(fenced(0.0, 0.0, aabb, 100.0, 0.0), (100.0, 0.0));
        // Far right: at most 15 pixels may stay on the stage, so the left edge stops at 225.
        assert_eq!(fenced(0.0, 0.0, aabb, 1000.0, 0.0), (275.0, 0.0));
        assert_eq!(fenced(0.0, 0.0, aabb, -1000.0, 0.0), (-275.0, 0.0));
        assert_eq!(fenced(0.0, 0.0, aabb, 0.0, 1000.0), (0.0, 215.0));
        // A tiny sprite keeps half of itself on the stage instead.
        let dot = Bounds { left: -2.0, right: 2.0, bottom: -2.0, top: 2.0 };
        assert_eq!(fenced(0.0, 0.0, dot, 1000.0, 0.0), (240.0, 0.0));
    }

    #[test]
    fn effects_clamp() {
        assert_eq!(clamp_effect(GHOST, 150.0), 100.0);
        assert_eq!(clamp_effect(GHOST, -1.0), 0.0);
        assert_eq!(clamp_effect(BRIGHTNESS, -200.0), -100.0);
        assert_eq!(clamp_effect(0, 1000.0), 1000.0);
    }
}

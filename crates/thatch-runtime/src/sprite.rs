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
//!
//! Both callbacks receive the runtime and may read it back, so everything that changes a sprite is
//! a method of [`Rt`], which calls the host with no reference to the runtime alive. What only reads
//! is a method of [`Runtime`].

use std::rc::Rc;

use thatch_abi::{
    EFFECT_COUNT, MEASURE_BOUNDS, MEASURE_BOX, MEASURE_COSTUME_SIZE, STAGE_HEIGHT, STAGE_WIDTH, Sprite, Value,
};
use thatch_values::{
    ValueRef, clamp, deg_to_rad, is_whitespace, js_max, js_min, js_number, rad_to_deg, round, wrap_clamp,
    wrap_direction,
};

use crate::scheduler::{Rt, Runtime};

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

    /// Where `point towards` turns a sprite, or None when it stays as it is.
    fn direction_towards(&mut self, target: usize, name: &str) -> Option<f64> {
        let (tx, ty) = match name {
            "_mouse_" => (self.mouse_x, self.mouse_y),
            "_random_" => return Some(round(self.random() * 360.0) - 180.0),
            _ => {
                let i = self.sprite_by_name(name)?;
                (self.targets[i].x, self.targets[i].y)
            }
        };
        let t = &self.targets[target];
        let dx = tx - t.x;
        let dy = ty - t.y;
        Some(90.0 - rad_to_deg(dy.atan2(dx)))
    }

    /// The costume index `switch costume to` picks, or None when it leaves the costume alone. A
    /// number is a costume number counted from 1 (or from 0 with `zero_index`); a string names a
    /// costume, or is "next costume", "previous costume", or on the backdrop block "next backdrop",
    /// "previous backdrop" or "random backdrop", or else reads as a number. The keywords follow the
    /// block, not the target, as in scratch-vm. The index is not wrapped yet.
    fn costume_for(
        &mut self,
        target: usize,
        requested: ValueRef<'_>,
        backdrop_block: bool,
        zero_index: bool,
    ) -> Option<f64> {
        let one = if zero_index { 0.0 } else { 1.0 };
        if let ValueRef::Num(n) = requested {
            return Some(n - one);
        }
        let text = thatch_values::to_string(requested);
        let t = &self.targets[target];
        if let Some(i) = t.costumes.iter().position(|c| **c == text) {
            return Some(i as f64);
        }
        let (next, previous) =
            if backdrop_block { ("next backdrop", "previous backdrop") } else { ("next costume", "previous costume") };
        let current = t.costume as f64;
        if text == next {
            return Some(current + 1.0);
        }
        if text == previous {
            return Some(current - 1.0);
        }
        if backdrop_block && text == "random backdrop" {
            let count = t.costumes.len();
            if count <= 1 {
                return None;
            }
            // `inclusiveRandIntWithout`: any backdrop but the current one.
            let mut pick = (self.random() * (count - 1) as f64).floor();
            if pick >= current {
                pick += 1.0;
            }
            return Some(pick);
        }
        // `isNaN(s)` reads the value as a number (true is 1 and false 0, as `Number` has them);
        // whitespace is not a number here.
        let n = match requested {
            ValueRef::Bool(b) => f64::from(b),
            _ => js_number(&text),
        };
        let whitespace = matches!(requested, ValueRef::Str(s) if is_whitespace(s));
        (!(n.is_nan() || whitespace)).then_some(n - one)
    }

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

impl Rt {
    /// Tells a host that draws that a target looks different, so it draws after the frame. When
    /// `redraw` holds (the target is visible, for most changes), the change also asks for a redraw
    /// as scratch-vm does with a renderer, which ends the frame after the current pass. Without
    /// `sprite_changed` the host does not draw, and neither happens.
    fn changed(self, target: usize, redraw: bool) {
        let host = self.with(|r| r.host);
        let Some(cb) = host.sprite_changed else { return };
        // SAFETY: the host gave this callback and its data for the runtime to call, and no
        // reference to the runtime is alive, so the callback may read it back.
        unsafe { cb(host.data, self.opaque(), target as i64) };
        self.with(|r| {
            r.host_draw();
            if redraw {
                r.request_redraw();
            }
        });
    }

    /// Asks the host to measure a sprite as it is drawn. None without a host that draws.
    fn measure(self, target: usize, what: i32) -> Option<[f64; 4]> {
        let host = self.with(|r| r.host);
        let cb = host.measure?;
        let mut out = [0.0; 4];
        // SAFETY: as `changed`; `out` is four doubles the callback may fill.
        let filled = unsafe { cb(host.data, self.opaque(), target as i64, what, out.as_mut_ptr()) };
        (filled != 0).then_some(out)
    }

    fn bounds(self, target: usize, what: i32) -> Option<Bounds> {
        let out = self.measure(target, what)?;
        Some(Bounds { left: out[0], right: out[1], bottom: out[2], top: out[3] })
    }

    /// The position of a sprite, or None for the stage.
    fn position(self, target: usize) -> Option<(f64, f64)> {
        self.with(|r| {
            let t = &r.targets[target];
            (!t.is_stage).then_some((t.x, t.y))
        })
    }

    // ---- motion -------------------------------------------------------------------------------

    /// `setXY`: moves a sprite, fenced to the stage when the host can measure it.
    pub fn set_xy(self, target: usize, x: f64, y: f64) {
        let Some((px, py)) = self.position(target) else { return };
        let (x, y) = match self.bounds(target, MEASURE_BOX) {
            Some(aabb) => fenced(px, py, aabb, x, y),
            None => (x, y),
        };
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            t.x = x;
            t.y = y;
            t.visible
        });
        self.changed(target, visible);
    }

    /// `setDirection`: wraps into -179 through 180 and ignores NaN and infinities.
    pub fn set_direction(self, target: usize, direction: f64) {
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            if t.is_stage || !direction.is_finite() {
                return None;
            }
            t.direction = wrap_direction(direction);
            Some(t.visible)
        });
        if let Some(visible) = visible {
            self.changed(target, visible);
        }
    }

    pub fn move_steps(self, target: usize, steps: f64) {
        let (x, y, direction) = self.with(|r| {
            let t = &r.targets[target];
            (t.x, t.y, t.direction)
        });
        let radians = deg_to_rad(90.0 - direction);
        self.set_xy(target, x + steps * radians.cos(), y + steps * radians.sin());
    }

    pub fn change_x(self, target: usize, dx: f64) {
        let (x, y) = self.with(|r| (r.targets[target].x, r.targets[target].y));
        self.set_xy(target, x + dx, y);
    }

    pub fn change_y(self, target: usize, dy: f64) {
        let (x, y) = self.with(|r| (r.targets[target].x, r.targets[target].y));
        self.set_xy(target, x, y + dy);
    }

    pub fn set_x(self, target: usize, x: f64) {
        let y = self.with(|r| r.targets[target].y);
        self.set_xy(target, x, y);
    }

    pub fn set_y(self, target: usize, y: f64) {
        let x = self.with(|r| r.targets[target].x);
        self.set_xy(target, x, y);
    }

    pub fn go_to(self, target: usize, name: &str) {
        if let Some((x, y)) = self.with(|r| r.target_xy(name)) {
            self.set_xy(target, x, y);
        }
    }

    pub fn point_towards(self, target: usize, name: &str) {
        if let Some(direction) = self.with(|r| r.direction_towards(target, name)) {
            self.set_direction(target, direction);
        }
    }

    /// `if on edge, bounce`. Needs the sprite's drawn bounds, so it does nothing headless.
    pub fn bounce(self, target: usize) {
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
        let radians = deg_to_rad(90.0 - self.with(|r| r.targets[target].direction));
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
        let (x, y) = self.with(|r| (r.targets[target].x, r.targets[target].y));
        self.set_xy(target, x + dx, y + dy);
    }

    /// `set rotation style`. The stage keeps its own, which nothing can observe, so that the stage
    /// state a host reads never changes but for the backdrop.
    pub fn set_rotation_style(self, target: usize, style: i64) {
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            if t.is_stage {
                return None;
            }
            t.rotation_style = style;
            Some(t.visible)
        });
        if let Some(visible) = visible {
            self.changed(target, visible);
        }
    }

    // ---- looks --------------------------------------------------------------------------------

    /// `setVisible`. Only showing asks for a redraw, as in scratch-vm, but the host draws either
    /// way.
    pub fn set_visible(self, target: usize, visible: bool) {
        let changed = self.with(|r| {
            let t = &mut r.targets[target];
            if t.is_stage {
                return false;
            }
            t.visible = visible;
            true
        });
        if changed {
            self.changed(target, visible);
        }
    }

    /// `setSize`: in percent, clamped between 5 pixels and one and a half stages when the host can
    /// measure the costume.
    pub fn set_size(self, target: usize, size: f64) {
        if self.position(target).is_none() {
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
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            t.size = size;
            t.visible
        });
        self.changed(target, visible);
    }

    /// `setCostume`: rounds the index and wraps it into the costume list.
    pub fn set_costume(self, target: usize, index: f64) {
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            let index = round(index);
            let index = if index.is_finite() { index } else { 0.0 };
            let count = t.costumes.len();
            let wrapped = wrap_clamp(index, 0.0, count as f64 - 1.0);
            t.costume =
                if wrapped.is_finite() && wrapped >= 0.0 { (wrapped as usize).min(count.saturating_sub(1)) } else { 0 };
            t.visible
        });
        self.changed(target, visible);
    }

    /// `_setCostume` and `_setBackdrop`; see [`Runtime::costume_for`].
    pub fn switch_costume(self, target: usize, requested: ValueRef<'_>, backdrop_block: bool, zero_index: bool) {
        if let Some(index) = self.with(|r| r.costume_for(target, requested, backdrop_block, zero_index)) {
            self.set_costume(target, index);
        }
    }

    pub fn set_effect(self, target: usize, effect: usize, value: f64) {
        if effect >= EFFECT_COUNT {
            return;
        }
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            t.effects[effect] = clamp_effect(effect, value);
            t.visible
        });
        self.changed(target, visible);
    }

    pub fn change_effect(self, target: usize, effect: usize, change: f64) {
        if effect >= EFFECT_COUNT {
            return;
        }
        let value = change + self.with(|r| r.targets[target].effects[effect]);
        self.set_effect(target, effect, value);
    }

    pub fn clear_effects(self, target: usize) {
        let visible = self.with(|r| {
            let t = &mut r.targets[target];
            t.effects = [0.0; EFFECT_COUNT];
            t.visible
        });
        self.changed(target, visible);
    }

    /// `moveExecutable`: moves a sprite `delta` layers forward (positive) or back in the drawing
    /// order, which is also the reverse of the order hats start in. scratch-render's
    /// `setDrawableOrder` clamps the new index between the first sprite layer, just in front of the
    /// stage, and the front, then truncates it as `Array.prototype.splice` does, so a fractional or
    /// infinite `delta` behaves as it does there. A NaN `delta`, which no block passes since they
    /// cast their input to a number first, keeps the sprite where the stage stays at the back.
    pub fn move_layer(self, target: usize, delta: f64) {
        let moved = self.with(|r| {
            if r.targets[target].is_stage {
                return false;
            }
            let Some(old) = r.order.iter().position(|t| *t == target) else { return false };
            r.order.remove(old);
            let min = if r.order.first().is_some_and(|t| r.targets[*t].is_stage) { 1.0 } else { 0.0 };
            let index = js_min(js_max(old as f64 + delta, min), r.order.len() as f64);
            let index = if index.is_nan() { min } else { index };
            r.order.insert((index.trunc() as usize).min(r.order.len()), target);
            true
        });
        // The renderer reorders too, but nothing asks for a redraw.
        if moved {
            self.changed(target, false);
        }
    }

    pub fn go_to_layer(self, target: usize, front: bool) {
        self.move_layer(target, if front { f64::INFINITY } else { f64::NEG_INFINITY });
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

    #[test]
    fn layers_never_go_behind_the_stage() {
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
            let handle = Rt::new(rt);
            handle.with(|r| {
                r.add_target("Stage".into(), true);
                r.add_target("Cat".into(), false);
                r.add_target("Dog".into(), false);
            });
            let order = || handle.with(|r| r.order.clone());
            // Half a layer back from just in front of the stage stays there, as does NaN.
            handle.move_layer(1, -0.5);
            assert_eq!(order(), [0, 1, 2]);
            handle.move_layer(1, f64::NAN);
            assert_eq!(order(), [0, 1, 2]);
            handle.move_layer(1, 1.5);
            assert_eq!(order(), [0, 2, 1]);
            handle.go_to_layer(1, false);
            assert_eq!(order(), [0, 1, 2]);
            handle.move_layer(0, 5.0);
            assert_eq!(order(), [0, 1, 2], "the stage does not move");
            Runtime::destroy(rt);
        }
    }
}

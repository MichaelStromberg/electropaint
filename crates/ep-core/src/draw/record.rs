//! Recording what `drawit` and `drawribbons` draw as a [`Scene`] (with
//! `Options::scene`). Nothing here calls a logged IrisGL function or feeds
//! a value into one, so the call stream is the same with scenes on or off.
//!
//! [`Scene`]: crate::scene::Scene

use super::SlotCopy;
use crate::panel::ids;
use crate::scene::{Ribbon, Shape, Tile};
use crate::sim::Sim;

impl Sim {
    /// Starts recording a new scene, if scenes are on: `drawit` may run
    /// twice in a step (smooth mode), and the picture left is the last.
    pub(super) fn begin_scene(&mut self) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        scene.tiles.clear();
        scene.ribbons.clear();
        scene.background = self.bkg_rgba;
        [scene.smear, scene.smooth, scene.fade] =
            [ids::SMEAR, ids::SMOOTH, ids::FADE].map(|id| self.controls.act(id).on());
        self.scene_drawn = true;
    }

    /// Records the copy of a slot that the modelview now places.
    pub(super) fn record_tile(&mut self, slot: &SlotCopy<'_>) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        let shape = if self.controls.act(ids::TRIANGLES).on() {
            Shape::Triangle
        } else {
            Shape::Square
        };
        let p = slot.p;
        scene.tiles.push(Tile {
            model_view: *self.gl.modelview(),
            shape,
            fill: self.hist.fill[p].then_some(self.col_rgba),
            outline: self.hist.outline[p].then_some(self.outline_rgba),
            fat: self.hist.fatline[p],
            hue: slot.v.hue,
            light: slot.v.light,
            age: slot.age as u16,
            copy: slot.copy,
        });
    }

    /// Starts recording a ribbon (one per quad-mode copy).
    pub(super) fn begin_ribbon(&mut self) {
        if let Some(scene) = &mut self.scene {
            scene.ribbons.push(Ribbon::default());
        }
    }

    /// Records a ribbon slot: its `edge` (in model space, placed by the
    /// modelview now), its colour, already in `col_rgba`, and `hue`.
    pub(super) fn record_ribbon_slot(&mut self, edge: [[f32; 3]; 2], hue: f32) {
        let Some(ribbon) = self.scene.as_mut().and_then(|s| s.ribbons.last_mut()) else {
            return;
        };
        ribbon.edges.push(edge.map(|e| self.gl.eye_point(e)));
        ribbon.colours.push(self.col_rgba);
        ribbon.hues.push(hue);
    }
}

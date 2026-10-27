//! Panel input: libpanel's `pnl_dopanel` and the actuator callbacks.

use super::{HAS_ZBUFFER, Input, Sim};
use crate::panel::{Accumulator, Callback, Kind, Panel, ids};

impl Sim {
    /// `pnl_dopanel`: apply queued input.
    pub(super) fn dopanel(&mut self) {
        while let Some(i) = self.inputs.pop_front() {
            match i {
                Input::Down(id) => self.act_down(id),
                Input::Up(id) => self.act_up(id),
                Input::Drag(id, v) => self.act_drag(id, v),
                Input::Release(id) => {
                    if self.controls.get(id).is_some() {
                        self.panel_callback(id, false);
                        self.callback(self.controls.act(id).up, id);
                    }
                    self.grabbed = None;
                }
                Input::Puck(x, y) => self.puck = [x, y],
            }
        }
    }

    fn act_down(&mut self, id: u16) {
        let Some(a) = self.controls.get_mut(id) else {
            return;
        };
        match a.kind {
            Kind::Toggle | Kind::SliderBounce | Kind::WideToggle => {
                a.val = if a.val == a.min { a.max } else { a.min };
            }
            Kind::Button | Kind::WideButton | Kind::SliderReset | Kind::DialReset | Kind::Mouse => {
                a.val = a.max
            }
            Kind::Radio => a.val = a.max,
            _ => {}
        }
        let (down, active) = (a.down, a.active);
        self.grabbed = Some(id);
        self.callback(down, id);
        self.callback(active, id);
        self.panel_callback(id, true);
    }

    fn act_up(&mut self, id: u16) {
        let Some(a) = self.controls.get_mut(id) else {
            return;
        };
        if matches!(
            a.kind,
            Kind::Button | Kind::WideButton | Kind::SliderReset | Kind::DialReset | Kind::Mouse
        ) {
            a.val = a.min;
        }
        let up = a.up;
        self.callback(up, id);
        self.panel_callback(id, false);
        self.grabbed = None;
    }

    /// A drag on a slider-like control.
    fn act_drag(&mut self, id: u16, v: f32) {
        let Some(a) = self.controls.get(id).cloned() else {
            return;
        };
        let (lo, hi) = a.span();
        let clamp = |v: f32| v.clamp(lo, hi);
        self.grabbed = Some(id);
        match (a.kind, a.slider) {
            (Kind::Slider | Kind::SliderValue, Some(s)) => {
                let sl = self.controls.sliders[s];
                let main = self.controls.act(sl.id);
                let bar = v.clamp(main.min, main.max);
                let up = self.controls.act_mut(sl.upper());
                up.extval = up.extval.max(bar);
                let lo = self.controls.act_mut(sl.lower());
                lo.extval = lo.extval.min(bar);
                self.controls.act_mut(sl.value()).extval = bar;
                self.controls.act_mut(sl.id).val = bar;
                self.grabbed = Some(sl.value());
            }
            (Kind::SliderUpper | Kind::SliderLower, Some(s)) => {
                self.controls.save_range(s);
                let sl = self.controls.sliders[s];
                let main = self.controls.act(sl.id);
                let x = v.clamp(main.min, main.max);
                let (up, lo, val) = (sl.upper(), sl.lower(), sl.value());
                if a.kind == Kind::SliderUpper {
                    self.controls.act_mut(up).extval = x;
                    let vv = self.controls.act(val).extval.min(x);
                    self.controls.act_mut(val).extval = vv;
                    let b = self.controls.act(lo).extval.min(vv);
                    self.controls.act_mut(lo).extval = b;
                } else {
                    self.controls.act_mut(lo).extval = x;
                    let vv = self.controls.act(val).extval.max(x);
                    self.controls.act_mut(val).extval = vv;
                    let t = self.controls.act(up).extval.max(vv);
                    self.controls.act_mut(up).extval = t;
                    self.controls.set_center(s);
                }
                self.controls.act_mut(sl.id).val = x;
            }
            (
                Kind::SliderRate | Kind::HSlider | Kind::FilledSlider | Kind::Dial | Kind::Palette,
                _,
            ) => {
                self.controls.act_mut(id).val = clamp(v);
                self.callback(a.down, id);
                self.callback(a.active, id);
            }
            _ => {
                self.setact(id, v);
                return;
            }
        }
        self.panel_callback(id, true);
    }

    /// The panels' own callbacks: the main panel records actuator limits
    /// (`recordact`); the colours panel also applies dwell and active
    /// button constraints first.
    fn panel_callback(&mut self, id: u16, down: bool) {
        let Some(panel) = self.controls.get(id).and_then(|a| a.panel) else {
            return;
        };
        match panel {
            Panel::Main => self.record_act(id, down),
            Panel::Colors => {
                if down {
                    self.controls.choose_updates(id);
                }
                self.record_act(id, down);
            }
            Panel::Texture => {}
        }
    }

    /// `recordact`: with "auto" on, pressing a control sets its random
    /// walk's lower limit to the current value; with `-o`, releasing it
    /// sets the upper one and enables the walk.
    fn record_act(&mut self, id: u16, down: bool) {
        if !self.controls.act(ids::AUTO).on() || self.recs[usize::from(id)].is_none() {
            return;
        }
        let v = self.controls.act(id).val;
        if down {
            self.rec_mut(id).lower = v;
        } else if self.record {
            // The duration comes from the sequence the parser last edited;
            // the variance is forced to 1.
            let duration = self
                .edit_seq
                .and_then(|e| self.seqs.iter().find(|s| s.id == e))
                .map_or(300.0, |s| s.duration);
            let r = self.rec_mut(id);
            if r.lower < v {
                r.upper = v;
            } else {
                r.upper = r.lower;
                r.lower = v;
            }
            r.duration = duration;
            r.variance = 1.0;
            r.enabled = true;
        }
    }

    /// `setact`: set a value the way scripts (and the random walks) do,
    /// then call the actuator's down, active and up callbacks.
    pub(crate) fn setact(&mut self, id: u16, v: f32) {
        let a = self.controls.act(id);
        if !a.in_acttable {
            // An "R" button: acttable holds an orphan copy with no
            // callbacks, so nothing visible happens.
            self.controls.act_mut(id).orphan_val = v;
            return;
        }
        let (kind, slider, cbs) = (a.kind, a.slider, [a.down, a.active, a.up]);
        match (kind, slider) {
            (Kind::Slider, Some(s)) => self.controls.set_slider_bar(s, v),
            (Kind::FilledSlider, _) => {
                self.controls.act_mut(id).val = v;
                self.controls.choose_updates(id);
            }
            _ => self.controls.act_mut(id).val = v,
        }
        for cb in cbs {
            self.callback(cb, id);
        }
    }

    fn callback(&mut self, cb: Option<Callback>, id: u16) {
        let Some(cb) = cb else { return };
        let on = |s: &Sim, id: u16| s.controls.act(id).on();
        match cb {
            Callback::SetFullscreen => self.set_fullscreen(id),
            Callback::CleanExit => self.exited = true,
            Callback::Reset => self.reset(),
            Callback::SetAuto => {
                if !self.controls.act(id).not_off() {
                    self.stop_anim();
                }
            }
            Callback::SetSmooth => {
                if on(self, ids::SMEAR) {
                    let smooth = on(self, id);
                    self.gl.frontbuffer(!smooth);
                    self.gl.backbuffer(true);
                }
            }
            Callback::SetSmear => self.set_smear(id),
            Callback::SetZbuf => self.set_zbuf(id),
            Callback::SaveRange | Callback::SetCenter | Callback::SetRange => {
                if let Some(s) = self.controls.get(id).and_then(|a| a.slider) {
                    match cb {
                        Callback::SaveRange => self.controls.save_range(s),
                        Callback::SetCenter => self.controls.set_center(s),
                        _ => self.controls.set_range(s),
                    }
                }
            }
            Callback::ResetSlider => self.reset_slider(id),
            Callback::ResetDial => self.controls.reset_dial(id - ids::DIAL_RESET),
            Callback::ToggleChanMode => self.controls.toggle_chan_mode(id),
            // Selects texture 0, "no texture": this build has none.
            Callback::SetTextureIndex => {}
        }
    }

    /// `setfullscreen`, from the full screen button or the mouse.
    fn set_fullscreen(&mut self, id: u16) {
        if self.saver {
            return;
        }
        let full = self.controls.act(ids::FULLSCREEN).on();
        // From the full screen button: follow it; from anything
        // else (the mouse): toggle.
        if full == (id == ids::FULLSCREEN) {
            self.go_fullscreen();
        } else {
            self.go_regularscreen();
        }
    }

    /// `setsmear`: smear on (and smooth off) redraws once and draws into
    /// both buffers; smear off draws into the back buffer only.
    fn set_smear(&mut self, id: u16) {
        if self.controls.act(id).on() {
            if !self.controls.act(ids::SMOOTH).not_off() {
                if !self.controls.act(ids::STOP).not_off() {
                    self.gl.c4f(self.bkg_rgba);
                    self.gl.clear();
                    self.drawit();
                }
                self.gl.frontbuffer(true);
                self.gl.backbuffer(true);
            }
        } else {
            self.gl.frontbuffer(false);
            self.gl.backbuffer(true);
        }
    }

    /// `setzbuf`: `zbuffer(on)`, plus `zclear()` when switching on.
    fn set_zbuf(&mut self, id: u16) {
        if !self.controls.act(id).not_off() {
            self.gl.zbuffer(false);
        } else {
            self.gl.zbuffer(true);
            if HAS_ZBUFFER {
                self.gl.zclear();
            }
        }
    }

    /// `resetslider` (an "R" button): the slider's first stage, or on the
    /// second, the accumulator it drives back to the slider's initial value.
    fn reset_slider(&mut self, id: u16) {
        let Some(s) = self.controls.get(id).and_then(|a| a.slider) else {
            return;
        };
        let init = self.controls.act(self.controls.sliders[s].id).initval;
        match self.controls.reset_slider(s) {
            Some(Accumulator::Wheel) => self.wheel = init,
            Some(Accumulator::Spin) => self.gspin = init,
            Some(Accumulator::Flip) => self.gflip = init,
            Some(Accumulator::Texture) | None => {}
        }
    }

    /// The "reset" button (`reset()`): every "R" pressed twice, the puck
    /// centred, and the buttons, dials and channel controls back to their
    /// initial values (with the buffer callbacks re-run). Random walks
    /// keep going.
    pub(crate) fn reset(&mut self) {
        for id in self.controls.reset_list.clone() {
            let cb = self.controls.act(id).down;
            self.callback(cb, id);
            self.callback(cb, id);
        }
        self.dwheel = 0.0;
        self.puck = [0.0; 2];
        let restore = |s: &mut Sim, id: u16| {
            let a = s.controls.act_mut(id);
            if a.panel.is_some() {
                a.val = a.initval;
            }
        };
        restore(self, ids::SMEAR);
        self.callback(Some(Callback::SetSmear), ids::SMEAR);
        restore(self, ids::ZBUF);
        self.callback(Some(Callback::SetZbuf), ids::ZBUF);
        restore(self, ids::SMOOTH);
        self.callback(Some(Callback::SetSmooth), ids::SMOOTH);
        for id in [
            ids::OUTLINE,
            ids::FILL,
            ids::FATLINE,
            ids::FADE,
            ids::RIBBON,
            ids::BACKGROUND,
            ids::ALPHA_OUT,
            ids::ALPHA_BACK,
            ids::HUE_BACK_RATE,
            ids::LIGHT,
        ] {
            restore(self, id);
        }
        for i in 0..5 {
            restore(self, ids::alpha_dial(i));
            restore(self, ids::dwell(i));
        }
        for id in [ids::CHAN_RATE, ids::HUE_PALETTE, ids::LIGHT_PALETTE] {
            restore(self, id);
        }
        self.controls.toggle_chan_mode(ids::LIGHT_PALETTE);
    }
}

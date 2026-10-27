//! The panels as `defpanel` builds them: every actuator with its
//! layout, range, initial value, key and callbacks.

use super::{Accumulator, Actuator, Callback, Controls, EpSlider, Kind, Panel, ids};

/// Builds every panel into `c` in the original's creation order.
pub(super) fn build_panels(c: Controls) -> Controls {
    let mut b = Builder { c };
    b.main_panel();
    b.colors_panel();
    b.texture_panel();
    b.c
}

/// Builds the panels in `defpanel` order.
struct Builder {
    c: Controls,
}

impl Builder {
    fn add(&mut self, id: u16, kind: Kind, panel: Option<Panel>) -> &mut Actuator {
        let slot = &mut self.c.acts[usize::from(id)];
        assert!(slot.is_none(), "id {id} built twice");
        slot.insert(Actuator::new(id, kind, panel))
    }

    fn wide(&mut self, id: u16, label: &'static str, y: f32, kind: Kind) -> &mut Actuator {
        let a = self.add(id, kind, Some(Panel::Main));
        a.label = Some(label);
        a.pos = [0.0, y];
        a.size = [1.8, 0.4];
        a
    }

    fn toggle(
        &mut self,
        id: u16,
        label: &'static str,
        y: f32,
        key: char,
        panel: Option<Panel>,
    ) -> &mut Actuator {
        let a = self.add(id, Kind::Toggle, panel);
        a.label = Some(label);
        a.pos = [4.5, y];
        a.size = [0.4, 0.4];
        a.key = Some(key);
        a
    }

    fn main_panel(&mut self) {
        use Callback::*;
        let m = Some(Panel::Main);
        let a = self.add(ids::MOUSE, Kind::Mouse, m);
        a.label = Some("mouse");
        a.down = Some(SetFullscreen);
        // The original's credit line; the port shows no text for it.
        self.add(ids::LABEL, Kind::Label, m).pos = [0.2, 19.0];
        let a = self.wide(ids::EXIT, "exit", 0.0, Kind::WideButton);
        a.key = Some('\x1b');
        a.up = Some(CleanExit);
        self.wide(ids::RESET, "reset", -1.0, Kind::WideButton).up = Some(Reset);
        self.wide(ids::AUTO, "auto", -1.5, Kind::WideToggle).up = Some(SetAuto);
        self.toggle(ids::OUTLINE, "outline (O)", 2.5, 'O', m);
        self.toggle(ids::FATLINE, "fat lines (I)", 2.0, 'I', m);
        let a = self.toggle(ids::FILL, "fill (U)", 1.5, 'U', m);
        a.val = 1.0;
        a.initval = 1.0;
        self.toggle(ids::RIBBON, "ribbons (R)", 1.0, 'R', m);
        self.toggle(ids::SMOOTH, "smooth (M)", 0.5, 'M', m).down = Some(SetSmooth);
        self.toggle(ids::SMEAR, "smear (Q)", 0.0, 'Q', m).down = Some(SetSmear);
        // Created but not added in the RGB build: no label, no key, no
        // button; scripts can still reach it.
        let a = self.toggle(ids::FADE, "fade (W)", -0.5, 'W', None);
        a.label = None;
        a.visible = false;
        self.toggle(ids::BACKGROUND, "back (E)", -0.5, 'E', m);
        self.toggle(ids::ZBUF, "z-buf (Z)", -1.0, 'Z', m).down = Some(SetZbuf);
        self.toggle(ids::LIGHT, "light (L)", -1.5, 'L', m);
        self.toggle(ids::STOP, "stop (S)", -2.0, 'S', m);
        self.toggle(ids::FULLSCREEN, "full screen (F)", -2.5, 'F', m)
            .down = Some(SetFullscreen);
        let a = self.add(ids::PUCK, Kind::Puck, m);
        a.pos = [0.0, 1.5];
        a.size = [0.8, 0.8];
        let w = Some(Accumulator::Wheel);
        let sp = Some(Accumulator::Spin);
        let fl = Some(Accumulator::Flip);
        for (id, label, x, y, min, max, init, acc) in [
            (ids::ZOOM, "zoom", 0.0, 3.5, -0.5, 0.5, 0.0, None),
            (ids::TWIST, "twst", 1.0, 3.5, -200.0, 200.0, 0.0, None),
            (ids::SPEED, "spd", 2.0, 3.5, 0.0, 1.0, 1.0, None),
            (ids::N, "n", 3.0, 3.5, 1.0, 127.0, 32.0, None),
            (ids::AMBIENT, "amb", 4.0, 3.5, 0.0, 1.0, 0.0, None),
            (ids::SHINY, "shn", 5.0, 3.5, 0.0, 1.0, 1.0, None),
            (ids::WHEEL, "whl", 0.0, 11.5, -60.0, 60.0, 0.0, w),
            (ids::SPIN, "spn", 1.0, 11.5, -20.0, 20.0, 0.0, sp),
            (ids::FLIP, "flp", 2.0, 11.5, -20.0, 20.0, 0.0, fl),
            (ids::ARM, "arm", 3.0, 11.5, -3.0, 3.0, 0.0, None),
            (ids::WRIST, "wrst", 4.0, 11.5, -3.0, 3.0, 0.0, None),
            (ids::SIZE, "sz", 5.0, 11.5, 0.0, 10.0, 1.0, None),
        ] {
            self.ep_slider(
                id,
                Some(label),
                x,
                y,
                min,
                max,
                init,
                acc,
                true,
                Panel::Main,
                None,
            );
        }
    }

    fn colors_panel(&mut self) {
        use Callback::*;
        let c = Some(Panel::Colors);
        for (i, (id, label, min, max, init)) in [
            (ids::ALPHA_OUT, "outa", 0.0, 1.0, 1.0),
            (ids::ALPHA_BACK, "baka", 0.0, 1.0, 1.0),
            (ids::HUE_BACK_RATE, "outr", -20.0, 20.0, 0.2),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 1.0 + i as f32;
            self.dial(id, Some(label), x, 0.0, min, max, init);
            self.reset_button(id + ids::DIAL_RESET, x, 1.0, Kind::DialReset, c);
        }
        for i in 0..5 {
            let x = i as f32;
            let a = self.dial(ids::alpha_dial(i), None, x, 2.0, 0.0, 1.0, 1.0);
            a.channel = Some(i);
            self.ep_slider(
                ids::hue_slider(i),
                None,
                x,
                3.0,
                0.0,
                2.0,
                1.0,
                None,
                false,
                Panel::Colors,
                Some(i),
            );
            self.ep_slider(
                ids::light_slider(i),
                None,
                x,
                3.0,
                0.0,
                1.0,
                0.5,
                None,
                false,
                Panel::Colors,
                Some(i),
            );
            for off in [0, ids::SLIDER_RATE, ids::SLIDER_BOUNCE] {
                self.c.act_mut(ids::light_slider(i) + off).visible = false;
            }
            if i == 4 {
                let a = self.add(ids::HUE_PALETTE, Kind::PaletteFrame, c);
                a.pos = [5.0, 3.0];
                a.size = [0.3, 5.8];
                a.min = 640.0;
                a.max = 767.0;
                a.down = Some(ToggleChanMode);
                let a = self.add(ids::LIGHT_PALETTE, Kind::Palette, c);
                a.pos = [5.0, 3.0];
                a.size = [0.3, 5.8];
                a.min = 512.0;
                a.max = 638.0;
                a.down = Some(ToggleChanMode);
                a.visible = false;
            }
            let a = self.add(ids::hue_index(i), Kind::Palette, c);
            a.pos = [x, 10.28];
            a.size = [0.8, 0.4];
            a.min = 768.0 + i as f32;
            a.max = a.min;
            a.channel = Some(i);
            let first = if i == 0 { 1.0 } else { 0.0 };
            let a = self.add(ids::dwell(i), Kind::FilledSlider, c);
            a.pos = [x, 10.88];
            a.size = [0.8, 2.8];
            a.val = first;
            a.initval = first;
            a.channel = Some(i);
            let a = self.add(ids::active_button(i), Kind::Button, c);
            a.pos = [x, 13.88];
            a.size = [0.8, 0.4];
            a.val = first;
            a.initval = first;
            a.channel = Some(i);
        }
        let a = self.add(ids::CHAN_RATE, Kind::HSlider, c);
        a.pos = [0.0, 14.48];
        a.size = [4.8, 0.29];
        // Hue channel 0 drifts by itself: its rate starts at 0.01.
        let r = self.c.act_mut(ids::hue_slider(0) + ids::SLIDER_RATE);
        r.val = 0.01;
        r.initval = 0.01;
    }

    fn texture_panel(&mut self) {
        let t = Some(Panel::Texture);
        let y0 = 14.77;
        let a = self.add(ids::NO_TEXTURE, Kind::Radio, t);
        a.label = Some("no texture");
        a.pos = [0.0, y0];
        a.size = [0.4, 0.4];
        a.val = 1.0;
        a.initval = 1.0;
        a.down = Some(Callback::SetTextureIndex);
        let a = self.add(ids::TRIANGLES, Kind::Toggle, t);
        a.label = Some("triangles <T>");
        a.pos = [0.0, y0 + 0.6];
        a.size = [0.4, 0.4];
        a.key = Some('T');
        let y = y0 + 0.6 + 0.5;
        let acc = Some(Accumulator::Texture);
        for (k, (id, label, min, max, init)) in [
            (ids::TEXTURE_T, "t", -0.01, 0.01, 0.0),
            (ids::TEXTURE_S, "s", -0.01, 0.01, 0.0),
            (ids::TEXTURE_SMAX, "S", 0.0, 10.0, 1.0),
            (ids::TEXTURE_TMAX, "T", 0.0, 10.0, 1.0),
        ]
        .into_iter()
        .enumerate()
        {
            self.ep_slider(
                id,
                Some(label),
                k as f32,
                y,
                min,
                max,
                init,
                acc,
                true,
                Panel::Texture,
                None,
            );
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "an `ep_mkact` dial: id, label, position, range and initial value"
    )]
    fn dial(
        &mut self,
        id: u16,
        label: Option<&'static str>,
        x: f32,
        y: f32,
        min: f32,
        max: f32,
        init: f32,
    ) -> &mut Actuator {
        let a = self.add(id, Kind::Dial, Some(Panel::Colors));
        a.label = label;
        a.pos = [x, y];
        a.size = [0.8, 0.8];
        a.min = min;
        a.max = max;
        a.val = init;
        a.initval = init;
        a
    }

    fn reset_button(&mut self, id: u16, x: f32, y: f32, kind: Kind, panel: Option<Panel>) {
        let a = self.add(id, kind, panel);
        a.label = Some("R");
        a.pos = [x, y];
        a.size = [0.8, 0.4];
        a.down = Some(if kind == Kind::DialReset {
            Callback::ResetDial
        } else {
            Callback::ResetSlider
        });
        a.in_acttable = false;
        self.c.reset_list.insert(0, id);
    }

    /// `mkepslider`: seven actuators with consecutive ids.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors mkepslider's argument list (controls.md §3.3), plus id and channel"
    )]
    fn ep_slider(
        &mut self,
        id: u16,
        label: Option<&'static str>,
        x: f32,
        y: f32,
        min: f32,
        max: f32,
        init: f32,
        accumulator: Option<Accumulator>,
        bounce: bool,
        panel: Panel,
        channel: Option<usize>,
    ) {
        use Callback::*;
        let s = self.c.sliders.len();
        self.c.sliders.push(EpSlider {
            label,
            id,
            center: 0.0,
            range: 0.0,
            dir: 1.0,
            accumulator,
        });
        let p = Some(panel);
        let m = self.add(id, Kind::Slider, p);
        m.label = label;
        m.pos = [x, y];
        m.size = [0.8, 5.8];
        m.min = min;
        m.max = max;
        m.val = init;
        m.initval = init;
        let bars = [
            (ids::SLIDER_UPPER, Kind::SliderUpper, max),
            (ids::SLIDER_LOWER, Kind::SliderLower, min),
            (ids::SLIDER_VALUE, Kind::SliderValue, init),
        ];
        for (off, kind, ext) in bars {
            let a = self.add(id + off, kind, p);
            a.pos = [x, y];
            a.size = [0.8, 5.8];
            a.min = min;
            a.max = max;
            a.extval = ext;
            a.val = ext;
            a.initval = ext;
        }
        let up = self.c.act_mut(id + ids::SLIDER_UPPER);
        up.down = Some(SaveRange);
        let lo = self.c.act_mut(id + ids::SLIDER_LOWER);
        lo.down = Some(SaveRange);
        lo.active = Some(SetCenter);
        let r = self.add(id + ids::SLIDER_RATE, Kind::SliderRate, p);
        r.pos = [x, y + 5.9];
        r.size = [0.8, 0.29];
        r.max = (max - min) * 0.5;
        r.down = Some(SaveRange);
        r.active = Some(SetRange);
        let bv = if bounce { 1.0 } else { 0.0 };
        let b = self.add(id + ids::SLIDER_BOUNCE, Kind::SliderBounce, p);
        b.pos = [x, y + 6.29];
        b.size = [0.8, 0.29];
        b.min = 1.0;
        b.max = 0.0;
        b.val = bv;
        b.initval = bv;
        self.reset_button(id + ids::SLIDER_RESET, x, y + 6.68, Kind::SliderReset, p);
        for off in 0..=ids::SLIDER_RESET {
            let a = self.c.act_mut(id + off);
            a.slider = Some(s);
            a.channel = channel;
        }
    }
}

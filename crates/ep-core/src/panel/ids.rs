//! Actuator ids (controls.md §4–§5).

pub const MOUSE: u16 = 13;
pub const LABEL: u16 = 14;
pub const EXIT: u16 = 15;
pub const RESET: u16 = 16;
pub const AUTO: u16 = 17;
pub const OUTLINE: u16 = 18;
pub const FATLINE: u16 = 19;
pub const FILL: u16 = 20;
pub const RIBBON: u16 = 21;
pub const SMOOTH: u16 = 22;
pub const SMEAR: u16 = 23;
pub const FADE: u16 = 24;
pub const BACKGROUND: u16 = 25;
pub const ZBUF: u16 = 26;
pub const LIGHT: u16 = 27;
pub const STOP: u16 = 28;
pub const FULLSCREEN: u16 = 29;
pub const PUCK: u16 = 30;
pub const ZOOM: u16 = 31;
pub const TWIST: u16 = 38;
pub const SPEED: u16 = 45;
pub const N: u16 = 52;
pub const AMBIENT: u16 = 59;
pub const SHINY: u16 = 66;
pub const WHEEL: u16 = 73;
pub const SPIN: u16 = 80;
pub const FLIP: u16 = 87;
pub const ARM: u16 = 94;
pub const WRIST: u16 = 101;
pub const SIZE: u16 = 108;
pub const ALPHA_OUT: u16 = 116;
pub const ALPHA_BACK: u16 = 118;
pub const HUE_BACK_RATE: u16 = 120;
pub const HUE_PALETTE: u16 = 209;
pub const LIGHT_PALETTE: u16 = 210;
pub const CHAN_RATE: u16 = 214;
pub const NO_TEXTURE: u16 = 316;
pub const TRIANGLES: u16 = 317;
pub const TEXTURE_T: u16 = 318;
pub const TEXTURE_S: u16 = 325;
pub const TEXTURE_SMAX: u16 = 332;
pub const TEXTURE_TMAX: u16 = 339;

/// Offsets of the sub-actuators of an ep slider from its main id.
pub const SLIDER_UPPER: u16 = 1;
pub const SLIDER_LOWER: u16 = 2;
pub const SLIDER_VALUE: u16 = 3;
pub const SLIDER_RATE: u16 = 4;
pub const SLIDER_BOUNCE: u16 = 5;
pub const SLIDER_RESET: u16 = 6;
/// Offset of a colour-panel dial's "R" button from the dial.
pub const DIAL_RESET: u16 = 1;

/// First id of colour channel `i` (0..5): its alpha dial.
pub const fn channel_base(i: usize) -> u16 {
    if i < 4 { 122 + 18 * i as u16 } else { 194 }
}
pub const fn alpha_dial(i: usize) -> u16 {
    channel_base(i)
}
pub const fn hue_slider(i: usize) -> u16 {
    channel_base(i) + 1
}
pub const fn light_slider(i: usize) -> u16 {
    channel_base(i) + 8
}
pub const fn hue_index(i: usize) -> u16 {
    if i < 4 { channel_base(i) + 15 } else { 211 }
}
pub const fn dwell(i: usize) -> u16 {
    if i < 4 { channel_base(i) + 16 } else { 212 }
}
pub const fn active_button(i: usize) -> u16 {
    if i < 4 { channel_base(i) + 17 } else { 213 }
}

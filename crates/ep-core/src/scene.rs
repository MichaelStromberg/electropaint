//! What one step drew, as a scene rather than as IrisGL output.
//!
//! Every tile comes with its transform and colours, for renderers that light
//! and shade the strand themselves (`Options::scene`). Recording it calls
//! nothing that IrisGL would see, so the call stream is unchanged.

/// A column-major 4x4 matrix, laid out as Metal's `float4x4` (and IrisGL's).
pub type Mat4 = [f32; 16];

/// The shape drawn per slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    /// The 0.2-unit square centred on the slot's origin.
    #[default]
    Square,
    /// The equilateral triangle with a corner at the slot's origin.
    Triangle,
}

/// One copy of one slot's shape (four copies per slot in quad mode).
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Model space (the shape in the z = 0 plane) to eye space.
    pub model_view: Mat4,
    pub shape: Shape,
    /// The fill colour, when the slot is filled.
    pub fill: Option<[f32; 4]>,
    /// The outline colour, when the slot is outlined.
    pub outline: Option<[f32; 4]>,
    /// Outlined with fat (3-pixel) lines.
    pub fat: bool,
    /// The slot's hue and lightness (0..1), which the colours come from.
    pub hue: f32,
    pub light: f32,
    /// Position along the strand: 0 for the newest slot.
    pub age: u16,
    /// Which of the quad-mode copies this is (0 outside quad mode).
    pub copy: u8,
}

/// One ribbon: two points per slot, newest first, in eye space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ribbon {
    pub edges: Vec<[[f32; 3]; 2]>,
    /// The fill colour of each slot.
    pub colours: Vec<[f32; 4]>,
}

/// Everything one picture shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub tiles: Vec<Tile>,
    /// Ribbons, when the strand is drawn as ribbons instead of tiles (one per
    /// quad-mode copy).
    pub ribbons: Vec<Ribbon>,
    /// The clear colour, or the colour trails fade towards.
    pub background: [f32; 4],
    /// The panel's buffer effects: trails, two passes per step, and the
    /// dissolve.
    pub smear: bool,
    pub smooth: bool,
    pub fade: bool,
}

//! What one simulation step asks the renderer to do.
//!
//! The recorder plays the part of IrisGL on an Indy, where libgl transforms,
//! lights and shades on the CPU. Vertices arrive in eye space with their final
//! colour, so the renderer only has to project, rasterize, depth-test and
//! blend. Operations must be executed in order.

/// A vertex in eye space (after the modelview matrix) with its final RGBA
/// colour (after lighting and flat shading are resolved). `repr(C)` so a
/// renderer can upload slices of it directly (28 bytes, packed floats).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

/// Which colour buffer(s) drawing goes to (`frontbuffer`/`backbuffer`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawBuffer {
    #[default]
    Back,
    Front,
    Both,
}

/// Fixed-function state for a draw. IrisGL's z-buffer comparison is
/// less-or-equal; blending, when on, is `blendfunction(BF_SA, BF_MSA)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawState {
    pub depth_test: bool,
    pub blend: bool,
}

/// The projection in force (`perspective(fovy, aspect, near, far)`).
/// `fovy_tenths` is in tenths of a degree, as IrisGL takes it. `aspect` is
/// the original window's; a renderer filling a wider screen keeps `fovy` and
/// substitutes its own aspect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub fovy_tenths: i16,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    SetProjection(Projection),
    SetDrawBuffer(DrawBuffer),
    /// Fill the current colour buffer(s) with an RGBA colour.
    Clear([f32; 4]),
    /// Reset the depth buffer to the far value.
    ZClear,
    /// Filled triangles (polygons and tmeshes already split).
    Triangles {
        state: DrawState,
        verts: Vec<Vertex>,
    },
    /// Line segments as vertex pairs; `width` is in original screen pixels
    /// (`linewidth`), relative to the original 1280x1024 display.
    Lines {
        state: DrawState,
        width: f32,
        verts: Vec<Vertex>,
    },
    /// The stippled dissolve: fill the pixels selected by a 16x16 bit
    /// pattern (row-major, bit 15 = leftmost, rows from the bottom of the
    /// window), tiled from the window origin, with an RGBA colour.
    StippleFill {
        pattern: [u16; 16],
        color: [f32; 4],
    },
    /// `swapbuffers`: the back buffer becomes visible.
    Swap,
}

/// Everything recorded during one simulation tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub ops: Vec<Op>,
}

impl Frame {
    /// How many `swapbuffers` the tick did.
    pub fn swap_count(&self) -> usize {
        self.ops.iter().filter(|op| matches!(op, Op::Swap)).count()
    }

    /// How long the tick takes on screen, in 60 Hz retraces: one per swap,
    /// and one for a tick that didn't swap (rare), so time still moves.
    pub fn retraces(&self) -> u32 {
        u32::try_from(self.swap_count()).map_or(u32::MAX, |n| n.max(1))
    }
}

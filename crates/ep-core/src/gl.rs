//! A small IrisGL emulation: the subset of calls ep makes, with the state
//! they imply (matrix stack, colour, normal, lighting, buffers, patterns).
//!
//! Every call feeds two outputs:
//!
//! * the renderer's [`Frame`] ops. As on an Indy, where libgl transforms,
//!   lights and shades on the CPU, vertices leave here in eye space with
//!   their final colour: lighting is evaluated per vertex, flat shading is
//!   resolved (the last vertex of a polygon, the third of a tmesh
//!   triangle), polygons become triangles and closed lines become segments;
//! * optionally a call log ([`IrisCall`]) with the same names and argument
//!   values the IRIX tracer records, for numeric comparison with real
//!   traces.
//!
//! Matrices follow IrisGL's `mmode(MVIEWING)` rules: `perspective` loads the
//! projection matrix; `loadmatrix`, `polarview`, `rotate`, `translate`,
//! `scale` act on the modelview stack. IrisGL's row-vector matrices are
//! stored here as column-major column-vector matrices, which is the same
//! memory layout, and every transform call post-multiplies (the last call
//! applies to the vertex first).

use crate::frame::{DrawBuffer, DrawState, Frame, Op, Projection, Vertex};

/// A logged argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    /// Any integer type (`short`, `long`, `char`, colour indices, tokens).
    Int(i64),
    Float(f32),
    /// A pointer argument logged by value (`c4f`, `v2f`, `lmdef`, …).
    /// A NaN element marks a value the original reads from uninitialised
    /// memory; comparisons accept anything there.
    Floats(Vec<f32>),
    /// A double return value (`drand48`).
    Double(f64),
    /// A pointer whose contents are not logged.
    Ptr,
}

/// One IrisGL (or libc) call as the IRIX tracer records it. Return values
/// that matter (only `drand48`'s) are appended to `args`.
#[derive(Clone, Debug, PartialEq)]
pub struct IrisCall {
    pub name: &'static str,
    pub args: Vec<Arg>,
}

impl std::fmt::Display for IrisCall {
    fn fmt(&self, w: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(w, "{}(", self.name)?;
        for (i, a) in self.args.iter().enumerate() {
            if i > 0 {
                write!(w, ", ")?;
            }
            match a {
                Arg::Int(v) => write!(w, "{v}")?,
                Arg::Float(v) => write!(w, "{v:e}")?,
                Arg::Double(v) => write!(w, "{v:e}")?,
                Arg::Ptr => write!(w, "ptr")?,
                Arg::Floats(v) => {
                    write!(w, "[")?;
                    for (j, x) in v.iter().enumerate() {
                        write!(w, "{}{x:e}", if j > 0 { " " } else { "" })?;
                    }
                    write!(w, "]")?;
                }
            }
        }
        write!(w, ")")
    }
}

/// Column-major 4x4 matrix (same memory order as an IrisGL `Matrix`).
pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// `a · b`.
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

/// The rotation IrisGL's `rotate(a, axis)` applies: `a` tenths of a degree,
/// right-handed (counter-clockwise looking down the axis).
pub fn rotation(tenths: i16, axis: char) -> Mat4 {
    let rad = f64::from(tenths) / 10.0 * std::f64::consts::PI / 180.0;
    let (s, c) = (rad.sin() as f32, rad.cos() as f32);
    let mut m = IDENTITY;
    let (i, j) = match axis {
        'x' | 'X' => (1, 2),
        'y' | 'Y' => (2, 0),
        _ => (0, 1),
    };
    m[i * 4 + i] = c;
    m[i * 4 + j] = s;
    m[j * 4 + i] = -s;
    m[j * 4 + j] = c;
    m
}

pub fn translation(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

pub fn scaling(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

/// `perspective(fovy, aspect, near, far)` as a matrix (OpenGL clip space).
pub fn perspective_matrix(p: &Projection) -> Mat4 {
    let f = 1.0 / (f64::from(p.fovy_tenths) / 20.0).to_radians().tan();
    let (n, fa) = (f64::from(p.near), f64::from(p.far));
    let mut m = [0.0f32; 16];
    m[0] = (f / f64::from(p.aspect)) as f32;
    m[5] = f as f32;
    m[10] = ((fa + n) / (n - fa)) as f32;
    m[11] = -1.0;
    m[14] = (2.0 * fa * n / (n - fa)) as f32;
    m
}

fn transform_point(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r])
}

fn transform_dir(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2])
}

/// Normal transform: the inverse transpose of the upper 3x3, applied to
/// `n` (up to a positive scale factor; callers normalise).
fn transform_normal(m: &Mat4, n: [f32; 3]) -> [f32; 3] {
    let a = |r: usize, c: usize| f64::from(m[c * 4 + r]);
    // Cofactor matrix of the 3x3 = det · inverse transpose.
    let cof = [
        [
            a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1),
            a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2),
            a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0),
        ],
        [
            a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2),
            a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0),
            a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1),
        ],
        [
            a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1),
            a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2),
            a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0),
        ],
    ];
    let det = a(0, 0) * cof[0][0] + a(0, 1) * cof[0][1] + a(0, 2) * cof[0][2];
    let sign = if det < 0.0 { -1.0 } else { 1.0 };
    std::array::from_fn(|r| {
        (sign
            * (cof[r][0] * f64::from(n[0])
                + cof[r][1] * f64::from(n[1])
                + cof[r][2] * f64::from(n[2]))) as f32
    })
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { v.map(|x| x / l) } else { v }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// IrisGL `lmdef` tokens.
pub mod lm {
    pub const DEFMATERIAL: i16 = 0;
    pub const DEFLIGHT: i16 = 100;
    pub const DEFLMODEL: i16 = 200;
    pub const LMNULL: f32 = 0.0;
    pub const EMISSION: f32 = 1.0;
    pub const AMBIENT: f32 = 2.0;
    pub const DIFFUSE: f32 = 3.0;
    pub const SPECULAR: f32 = 4.0;
    pub const SHININESS: f32 = 5.0;
    pub const COLORINDEXES: f32 = 6.0;
    pub const ALPHA: f32 = 7.0;
    pub const LCOLOR: f32 = 101.0;
    pub const POSITION: f32 = 102.0;
    pub const SPOTDIRECTION: f32 = 103.0;
    pub const SPOTLIGHT: f32 = 104.0;
    pub const LOCALVIEWER: f32 = 201.0;
    pub const ATTENUATION: f32 = 202.0;
    pub const ATTENUATION2: f32 = 203.0;
    pub const TWOSIDE: f32 = 204.0;
    /// `lmbind` targets.
    pub const MATERIAL: i16 = 1000;
    pub const BACKMATERIAL: i16 = 1001;
    pub const LIGHT0: i16 = 1100;
    pub const LMODEL: i16 = 1200;
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Material {
    emission: [f32; 3],
    ambient: [f32; 3],
    diffuse: [f32; 3],
    specular: [f32; 3],
    shininess: f32,
    alpha: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            emission: [0.0; 3],
            ambient: [0.2; 3],
            diffuse: [0.8; 3],
            specular: [0.0; 3],
            shininess: 0.0,
            alpha: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Light {
    ambient: [f32; 3],
    color: [f32; 3],
    position: [f32; 4],
}

impl Default for Light {
    fn default() -> Self {
        Light {
            ambient: [0.0; 3],
            color: [1.0; 3],
            position: [0.0, 0.0, 1.0, 0.0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LModel {
    ambient: [f32; 3],
    local_viewer: bool,
}

impl Default for LModel {
    fn default() -> Self {
        LModel {
            ambient: [0.2; 3],
            local_viewer: false,
        }
    }
}

/// A light bound to a slot: its definition with the position already in
/// eye space (IrisGL transforms it by the modelview at `lmbind` time).
#[derive(Clone, Copy, Debug, PartialEq)]
struct BoundLight {
    light: Light,
    eye_pos: [f32; 4],
}

/// What `color(index)` does in RGB mode. The man page calls it an error;
/// ep issues it in its fade (rendering.md §8.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RgbColorIndex {
    /// The index replaces the low 16 bits of the packed `cpack` colour:
    /// red becomes its low byte and green its high byte, while blue and
    /// alpha stay. This is what an Indy does: ep's fade with `color(0)`
    /// dissolves the trail towards pure blues (screen grab comparison in
    /// docs/ground-truth.md).
    #[default]
    PackedLow16,
    /// Ignored: the current RGB colour stays.
    Ignore,
    /// Honoured through the colour map (`mapcolor` entries; unset ones are
    /// black).
    Colormap,
}

#[derive(Clone, Debug, PartialEq)]
enum Prim {
    None,
    Polygon(Vec<Vertex>),
    ClosedLine(Vec<Vertex>),
    /// A triangle mesh: the two saved vertices and which one is older.
    TMesh {
        saved: Vec<Vertex>,
        older: usize,
    },
}

/// The emulated IrisGL context.
#[derive(Clone, Debug)]
pub struct Gl {
    log: Option<Vec<IrisCall>>,
    ops: Vec<Op>,
    stack: Vec<Mat4>,
    projection: Option<Projection>,
    color: [f32; 4],
    normal: [f32; 3],
    /// Whether a normal (rather than a colour) was the last of the two sent:
    /// with `lmcolor(LMC_COLOR)` that decides whether a vertex is lit.
    normal_last: bool,
    flat: bool,
    zbuffer: bool,
    blend: bool,
    line_width: i16,
    front: bool,
    back: bool,
    patterns: Vec<(i16, [u16; 16])>,
    pattern: i16,
    materials: Vec<(i16, Material)>,
    lights: Vec<(i16, Light)>,
    lmodels: Vec<(i16, LModel)>,
    material: Option<Material>,
    lmodel: Option<LModel>,
    bound_lights: [Option<BoundLight>; 8],
    prim: Prim,
    colormap: Vec<[f32; 3]>,
    pub rgb_color_index: RgbColorIndex,
}

impl Gl {
    pub fn new(log: bool) -> Gl {
        Gl {
            log: log.then(Vec::new),
            ops: Vec::new(),
            stack: vec![IDENTITY],
            projection: None,
            color: [1.0; 4],
            normal: [0.0, 0.0, 1.0],
            normal_last: false,
            flat: false,
            zbuffer: false,
            blend: false,
            line_width: 1,
            front: false,
            back: true,
            patterns: Vec::new(),
            pattern: 0,
            materials: Vec::new(),
            lights: Vec::new(),
            lmodels: Vec::new(),
            material: None,
            lmodel: None,
            bound_lights: [None; 8],
            prim: Prim::None,
            colormap: vec![[0.0; 3]; 4096],
            rgb_color_index: RgbColorIndex::PackedLow16,
        }
    }

    // ---------------------------------------------------------------- output

    /// Take the renderer ops recorded so far.
    pub fn take_frame(&mut self) -> Frame {
        Frame {
            ops: std::mem::take(&mut self.ops),
        }
    }

    /// Take the call log recorded so far (empty when logging is off).
    pub fn take_calls(&mut self) -> Vec<IrisCall> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    pub fn logging(&self) -> bool {
        self.log.is_some()
    }

    /// Record a call that has no GL state effect here (window management,
    /// `srand48`, `drand48`, …).
    pub fn log_call(&mut self, name: &'static str, args: impl FnOnce() -> Vec<Arg>) {
        if let Some(l) = self.log.as_mut() {
            l.push(IrisCall { name, args: args() });
        }
    }

    fn log(&mut self, name: &'static str, args: impl FnOnce() -> Vec<Arg>) {
        self.log_call(name, args);
    }

    /// The modelview matrix in force.
    pub fn modelview(&self) -> &Mat4 {
        self.stack.last().expect("matrix stack is never empty")
    }

    fn modelview_mut(&mut self) -> &mut Mat4 {
        self.stack.last_mut().expect("matrix stack is never empty")
    }

    fn post_multiply(&mut self, m: &Mat4) {
        let top = mat_mul(self.modelview(), m);
        *self.modelview_mut() = top;
    }

    fn state(&self) -> DrawState {
        DrawState {
            depth_test: self.zbuffer,
            blend: self.blend,
        }
    }

    fn push_op_triangles(&mut self, verts: &[Vertex]) {
        let state = self.state();
        if let Some(Op::Triangles { state: s, verts: v }) = self.ops.last_mut()
            && *s == state
        {
            v.extend_from_slice(verts);
            return;
        }
        self.ops.push(Op::Triangles {
            state,
            verts: verts.to_vec(),
        });
    }

    fn push_op_lines(&mut self, verts: &[Vertex]) {
        let state = self.state();
        let width = f32::from(self.line_width);
        if let Some(Op::Lines {
            state: s,
            width: w,
            verts: v,
        }) = self.ops.last_mut()
            && *s == state
            && *w == width
        {
            v.extend_from_slice(verts);
            return;
        }
        self.ops.push(Op::Lines {
            state,
            width,
            verts: verts.to_vec(),
        });
    }

    fn draw_buffer(&self) -> DrawBuffer {
        match (self.front, self.back) {
            (true, true) => DrawBuffer::Both,
            (true, false) => DrawBuffer::Front,
            _ => DrawBuffer::Back,
        }
    }

    // ---------------------------------------------------------------- config

    pub fn doublebuffer(&mut self) {
        self.log("doublebuffer", Vec::new);
    }

    pub fn rgbmode(&mut self) {
        self.log("RGBmode", Vec::new);
    }

    pub fn gconfig(&mut self) {
        self.log("gconfig", Vec::new);
    }

    pub fn subpixel(&mut self, on: bool) {
        self.log("subpixel", || vec![Arg::Int(on.into())]);
    }

    /// `mmode(MVIEWING = 2)` selects separate projection and modelview
    /// matrices. ep (a lit build) sets it before any matrix call, so only
    /// MVIEWING is modelled.
    pub fn mmode(&mut self, mode: i16) {
        self.log("mmode", || vec![Arg::Int(mode.into())]);
    }

    pub fn blendfunction(&mut self, sfactor: i32, dfactor: i32) {
        self.log("blendfunction", || {
            vec![Arg::Int(sfactor.into()), Arg::Int(dfactor.into())]
        });
        // BF_ONE, BF_ZERO is "no blending"; ep only ever sets BF_SA, BF_MSA.
        self.blend = !(sfactor == 1 && dfactor == 0);
    }

    /// `shademodel(FLAT = 0 | GOURAUD = 1)`.
    pub fn shademodel(&mut self, mode: i32) {
        self.log("shademodel", || vec![Arg::Int(mode.into())]);
        self.flat = mode == 0;
    }

    pub fn concave(&mut self, on: bool) {
        self.log("concave", || vec![Arg::Int(on.into())]);
    }

    pub fn glcompat(&mut self, what: i32, value: i32) {
        self.log("glcompat", || {
            vec![Arg::Int(what.into()), Arg::Int(value.into())]
        });
    }

    pub fn lsetdepth(&mut self, near: i32, far: i32) {
        self.log("lsetdepth", || {
            vec![Arg::Int(near.into()), Arg::Int(far.into())]
        });
    }

    pub fn reshapeviewport(&mut self) {
        self.log("reshapeviewport", Vec::new);
    }

    pub fn zbuffer(&mut self, on: bool) {
        self.log("zbuffer", || vec![Arg::Int(on.into())]);
        self.zbuffer = on;
    }

    pub fn frontbuffer(&mut self, on: bool) {
        self.log("frontbuffer", || vec![Arg::Int(on.into())]);
        self.front = on;
        self.ops.push(Op::SetDrawBuffer(self.draw_buffer()));
    }

    pub fn backbuffer(&mut self, on: bool) {
        self.log("backbuffer", || vec![Arg::Int(on.into())]);
        self.back = on;
        self.ops.push(Op::SetDrawBuffer(self.draw_buffer()));
    }

    pub fn linewidth(&mut self, w: i16) {
        self.log("linewidth", || vec![Arg::Int(w.into())]);
        self.line_width = w;
    }

    /// `defpattern(index, size, words)`: rows from the bottom, bit 15 is the
    /// leftmost pixel. The tracer logs the pointer only.
    pub fn defpattern(&mut self, index: i16, words: [u16; 16]) {
        self.log("defpattern", || {
            vec![Arg::Int(index.into()), Arg::Int(16), Arg::Ptr]
        });
        match self.patterns.iter_mut().find(|p| p.0 == index) {
            Some(p) => p.1 = words,
            None => self.patterns.push((index, words)),
        }
    }

    pub fn setpattern(&mut self, index: i16) {
        self.log("setpattern", || vec![Arg::Int(index.into())]);
        self.pattern = index;
    }

    pub fn mapcolor(&mut self, index: u16, r: i16, g: i16, b: i16) {
        self.log("mapcolor", || {
            vec![
                Arg::Int(index.into()),
                Arg::Int(r.into()),
                Arg::Int(g.into()),
                Arg::Int(b.into()),
            ]
        });
        if let Some(e) = self.colormap.get_mut(usize::from(index)) {
            *e = [r, g, b].map(|c| f32::from(c) / 255.0);
        }
    }

    // ---------------------------------------------------------------- matrices

    pub fn perspective(&mut self, fovy_tenths: i16, aspect: f32, near: f32, far: f32) {
        self.log("perspective", || {
            vec![
                Arg::Int(fovy_tenths.into()),
                Arg::Float(aspect),
                Arg::Float(near),
                Arg::Float(far),
            ]
        });
        let p = Projection {
            fovy_tenths,
            aspect,
            near,
            far,
        };
        self.projection = Some(p);
        self.ops.push(Op::SetProjection(p));
    }

    pub fn loadmatrix(&mut self, m: &Mat4) {
        self.log("loadmatrix", || vec![Arg::Floats(m.to_vec())]);
        *self.modelview_mut() = *m;
    }

    pub fn multmatrix(&mut self, m: &Mat4) {
        self.log("multmatrix", || vec![Arg::Floats(m.to_vec())]);
        self.post_multiply(m);
    }

    /// `polarview(dist, azim, inc, twist)` = `translate(0,0,-dist)`,
    /// `rotate(-twist,'z')`, `rotate(-inc,'x')`, `rotate(-azim,'z')`.
    pub fn polarview(&mut self, dist: f32, azim: i16, inc: i16, twist: i16) {
        self.log("polarview", || {
            vec![
                Arg::Float(dist),
                Arg::Int(azim.into()),
                Arg::Int(inc.into()),
                Arg::Int(twist.into()),
            ]
        });
        let mut m = translation(0.0, 0.0, -dist);
        m = mat_mul(&m, &rotation(twist.wrapping_neg(), 'z'));
        m = mat_mul(&m, &rotation(inc.wrapping_neg(), 'x'));
        m = mat_mul(&m, &rotation(azim.wrapping_neg(), 'z'));
        self.post_multiply(&m);
    }

    pub fn pushmatrix(&mut self) {
        self.log("pushmatrix", Vec::new);
        let top = *self.modelview();
        self.stack.push(top);
    }

    pub fn popmatrix(&mut self) {
        self.log("popmatrix", Vec::new);
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// `rotate(a, axis)`: `a` in tenths of a degree, axis `'x'`, `'y'` or `'z'`.
    pub fn rotate(&mut self, tenths: i16, axis: char) {
        self.log("rotate", || {
            vec![Arg::Int(tenths.into()), Arg::Int(i64::from(axis as u32))]
        });
        self.post_multiply(&rotation(tenths, axis));
    }

    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        self.log("translate", || {
            vec![Arg::Float(x), Arg::Float(y), Arg::Float(z)]
        });
        self.post_multiply(&translation(x, y, z));
    }

    pub fn scale(&mut self, x: f32, y: f32, z: f32) {
        self.log("scale", || {
            vec![Arg::Float(x), Arg::Float(y), Arg::Float(z)]
        });
        self.post_multiply(&scaling(x, y, z));
    }

    // ---------------------------------------------------------------- colour

    pub fn c4f(&mut self, c: [f32; 4]) {
        self.log("c4f", || vec![Arg::Floats(c.to_vec())]);
        self.color = c;
        self.normal_last = false;
    }

    /// `cpack(0xAABBGGRR)`.
    pub fn cpack(&mut self, v: u32) {
        self.log("cpack", || vec![Arg::Int(v.into())]);
        self.color = [0, 8, 16, 24].map(|s| ((v >> s) & 0xff) as f32 / 255.0);
        self.normal_last = false;
    }

    /// `color(index)`: a colour-index command. ep is in RGB mode; see
    /// [`RgbColorIndex`].
    pub fn color(&mut self, index: u16) {
        self.log("color", || vec![Arg::Int(index.into())]);
        match self.rgb_color_index {
            RgbColorIndex::PackedLow16 => {
                self.color[0] = f32::from(index & 0xff) / 255.0;
                self.color[1] = f32::from(index >> 8) / 255.0;
                self.normal_last = false;
            }
            RgbColorIndex::Ignore => {}
            RgbColorIndex::Colormap => {
                let [r, g, b] = self
                    .colormap
                    .get(usize::from(index))
                    .copied()
                    .unwrap_or([0.0; 3]);
                self.color = [r, g, b, 1.0];
                self.normal_last = false;
            }
        }
    }

    pub fn n3f(&mut self, n: [f32; 3]) {
        self.log("n3f", || vec![Arg::Floats(n.to_vec())]);
        self.normal = n;
        self.normal_last = true;
    }

    // ---------------------------------------------------------------- lighting

    /// `lmdef(deftype, index, np, props)`. Properties not given keep their
    /// previous values (or the IrisGL defaults for a new index).
    pub fn lmdef(&mut self, deftype: i16, index: i16, props: &[f32]) {
        self.log("lmdef", || {
            vec![
                Arg::Int(deftype.into()),
                Arg::Int(index.into()),
                Arg::Int(props.len() as i64),
                Arg::Floats(props.to_vec()),
            ]
        });
        // NaN marks uninitialised memory in the original (see Arg::Floats);
        // use 0 for the state.
        let p: Vec<f32> = props
            .iter()
            .map(|&v| if v.is_nan() { 0.0 } else { v })
            .collect();
        let mut i = 0;
        let mut take = |n: usize| -> Option<&[f32]> {
            let s = p.get(i..i + n)?;
            i += n;
            Some(s)
        };
        let rgb = |s: &[f32]| [s[0], s[1], s[2]];
        match deftype {
            lm::DEFMATERIAL => {
                let mut m = lookup(&self.materials, index).unwrap_or_default();
                while let Some(&[tok]) = take(1) {
                    match tok {
                        lm::EMISSION => m.emission = take(3).map(rgb).unwrap_or(m.emission),
                        lm::AMBIENT => m.ambient = take(3).map(rgb).unwrap_or(m.ambient),
                        lm::DIFFUSE => m.diffuse = take(3).map(rgb).unwrap_or(m.diffuse),
                        lm::SPECULAR => m.specular = take(3).map(rgb).unwrap_or(m.specular),
                        lm::SHININESS => m.shininess = take(1).map(|s| s[0]).unwrap_or(m.shininess),
                        lm::COLORINDEXES => {
                            take(3);
                        }
                        lm::ALPHA => m.alpha = take(1).map(|s| s[0]).unwrap_or(m.alpha),
                        _ => break,
                    }
                }
                store(&mut self.materials, index, m);
            }
            lm::DEFLIGHT => {
                let mut l = lookup(&self.lights, index).unwrap_or_default();
                while let Some(&[tok]) = take(1) {
                    match tok {
                        lm::AMBIENT => l.ambient = take(3).map(rgb).unwrap_or(l.ambient),
                        lm::LCOLOR => l.color = take(3).map(rgb).unwrap_or(l.color),
                        lm::POSITION => {
                            if let Some(s) = take(4) {
                                l.position = [s[0], s[1], s[2], s[3]];
                            }
                        }
                        lm::SPOTDIRECTION => {
                            take(3);
                        }
                        lm::SPOTLIGHT => {
                            take(2);
                        }
                        _ => break,
                    }
                }
                store(&mut self.lights, index, l);
            }
            lm::DEFLMODEL => {
                let mut l = lookup(&self.lmodels, index).unwrap_or_default();
                while let Some(&[tok]) = take(1) {
                    match tok {
                        lm::AMBIENT => l.ambient = take(3).map(rgb).unwrap_or(l.ambient),
                        lm::LOCALVIEWER => l.local_viewer = take(1).is_some_and(|s| s[0] != 0.0),
                        lm::ATTENUATION => {
                            take(2);
                        }
                        lm::ATTENUATION2 | lm::TWOSIDE => {
                            take(1);
                        }
                        _ => break,
                    }
                }
                store(&mut self.lmodels, index, l);
            }
            _ => {}
        }
    }

    /// `lmbind(target, index)`. Index 0 unbinds. A light's position is
    /// transformed by the modelview in force now.
    pub fn lmbind(&mut self, target: i16, index: i16) {
        self.log("lmbind", || {
            vec![Arg::Int(target.into()), Arg::Int(index.into())]
        });
        match target {
            lm::MATERIAL => {
                self.material =
                    (index != 0).then(|| lookup(&self.materials, index).unwrap_or_default())
            }
            lm::LMODEL => {
                self.lmodel = (index != 0).then(|| lookup(&self.lmodels, index).unwrap_or_default())
            }
            t if (lm::LIGHT0..lm::LIGHT0 + 8).contains(&t) => {
                let slot = usize::from((t - lm::LIGHT0) as u8);
                self.bound_lights[slot] = (index != 0).then(|| {
                    let light = lookup(&self.lights, index).unwrap_or_default();
                    let p = light.position;
                    let mv = self.modelview();
                    let eye_pos = if p[3] == 0.0 {
                        let d = transform_dir(mv, [p[0], p[1], p[2]]);
                        [d[0], d[1], d[2], 0.0]
                    } else {
                        let q = transform_point(mv, [p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
                        [q[0], q[1], q[2], 1.0]
                    };
                    BoundLight { light, eye_pos }
                });
            }
            _ => {}
        }
    }

    /// Whether IrisGL lighting is enabled: a material and a lighting model
    /// must both be bound.
    pub fn lighting_enabled(&self) -> bool {
        self.material.is_some() && self.lmodel.is_some()
    }

    /// The colour of a vertex at eye-space position `pos`: lit if lighting
    /// is enabled and the last colour/normal command was a normal
    /// (`lmcolor(LMC_COLOR)`), else the current colour.
    fn vertex_color(&self, pos: [f32; 3]) -> [f32; 4] {
        let (Some(m), Some(lmodel)) = (self.material, self.lmodel) else {
            return self.color;
        };
        if !self.normal_last {
            return self.color;
        }
        let n = normalize(transform_normal(self.modelview(), self.normal));
        let mut c: [f32; 3] =
            std::array::from_fn(|i| m.emission[i] + lmodel.ambient[i] * m.ambient[i]);
        let view = if lmodel.local_viewer {
            normalize(pos.map(|x| -x))
        } else {
            [0.0, 0.0, 1.0]
        };
        for b in self.bound_lights.iter().flatten() {
            let l = if b.eye_pos[3] == 0.0 {
                normalize([b.eye_pos[0], b.eye_pos[1], b.eye_pos[2]])
            } else {
                normalize([
                    b.eye_pos[0] - pos[0],
                    b.eye_pos[1] - pos[1],
                    b.eye_pos[2] - pos[2],
                ])
            };
            for (i, ci) in c.iter_mut().enumerate() {
                *ci += b.light.ambient[i] * m.ambient[i];
            }
            let d = dot(n, l);
            if d > 0.0 {
                for (i, ci) in c.iter_mut().enumerate() {
                    *ci += d * b.light.color[i] * m.diffuse[i];
                }
                // Shininess 0 turns specular off in IrisGL.
                if m.shininess > 0.0 {
                    let h = normalize([l[0] + view[0], l[1] + view[1], l[2] + view[2]]);
                    let s = dot(n, h).max(0.0).powf(m.shininess);
                    for (i, ci) in c.iter_mut().enumerate() {
                        *ci += s * b.light.color[i] * m.specular[i];
                    }
                }
            }
        }
        [
            c[0].clamp(0.0, 1.0),
            c[1].clamp(0.0, 1.0),
            c[2].clamp(0.0, 1.0),
            m.alpha,
        ]
    }

    // ---------------------------------------------------------------- buffers

    /// `clear()`: fill the viewport with the current colour, through the
    /// current pattern if one is set; blending and z are ignored.
    pub fn clear(&mut self) {
        self.log("clear", Vec::new);
        let color = self.color;
        let pattern = (self.pattern != 0)
            .then(|| {
                self.patterns
                    .iter()
                    .find(|p| p.0 == self.pattern)
                    .map(|p| p.1)
            })
            .flatten();
        self.ops.push(match pattern {
            Some(pattern) => Op::StippleFill { pattern, color },
            None => Op::Clear(color),
        });
    }

    pub fn zclear(&mut self) {
        self.log("zclear", Vec::new);
        self.ops.push(Op::ZClear);
    }

    pub fn swapbuffers(&mut self) {
        self.log("swapbuffers", Vec::new);
        self.ops.push(Op::Swap);
    }

    // ---------------------------------------------------------------- primitives

    pub fn bgnpolygon(&mut self) {
        self.log("bgnpolygon", Vec::new);
        self.prim = Prim::Polygon(Vec::new());
    }

    /// Polygons are convex here (`concave(FALSE)`): a fan. Flat shading
    /// gives every triangle the colour of the polygon's last vertex.
    pub fn endpolygon(&mut self) {
        self.log("endpolygon", Vec::new);
        if let Prim::Polygon(mut v) = std::mem::replace(&mut self.prim, Prim::None) {
            if v.len() < 3 {
                return;
            }
            if self.flat {
                let c = v[v.len() - 1].color;
                v.iter_mut().for_each(|x| x.color = c);
            }
            let tris: Vec<Vertex> = (1..v.len() - 1)
                .flat_map(|i| [v[0], v[i], v[i + 1]])
                .collect();
            self.push_op_triangles(&tris);
        }
    }

    pub fn bgnclosedline(&mut self) {
        self.log("bgnclosedline", Vec::new);
        self.prim = Prim::ClosedLine(Vec::new());
    }

    /// A closed line loop as segments; flat shading colours each segment
    /// with its second vertex.
    pub fn endclosedline(&mut self) {
        self.log("endclosedline", Vec::new);
        if let Prim::ClosedLine(v) = std::mem::replace(&mut self.prim, Prim::None) {
            if v.len() < 2 {
                return;
            }
            let mut segs = Vec::with_capacity(v.len() * 2);
            for i in 0..v.len() {
                let (mut a, b) = (v[i], v[(i + 1) % v.len()]);
                if self.flat {
                    a.color = b.color;
                }
                segs.extend([a, b]);
            }
            self.push_op_lines(&segs);
        }
    }

    /// `bgntmesh()`. A `bgntmesh` while a mesh is open (ep's quad ribbons
    /// issue four with a single `endtmesh`) starts a new strip.
    pub fn bgntmesh(&mut self) {
        self.log("bgntmesh", Vec::new);
        self.prim = Prim::TMesh {
            saved: Vec::new(),
            older: 0,
        };
    }

    pub fn endtmesh(&mut self) {
        self.log("endtmesh", Vec::new);
        self.prim = Prim::None;
    }

    pub fn v2f(&mut self, v: [f32; 2]) {
        self.log("v2f", || vec![Arg::Floats(v.to_vec())]);
        self.vertex([v[0], v[1], 0.0]);
    }

    pub fn v3f(&mut self, v: [f32; 3]) {
        self.log("v3f", || vec![Arg::Floats(v.to_vec())]);
        self.vertex(v);
    }

    pub fn t2f(&mut self, t: [f32; 2]) {
        self.log("t2f", || vec![Arg::Floats(t.to_vec())]);
    }

    fn vertex(&mut self, v: [f32; 3]) {
        let pos = transform_point(self.modelview(), v);
        let vert = Vertex {
            pos,
            color: self.vertex_color(pos),
        };
        let flat = self.flat;
        let mut tri = None;
        match &mut self.prim {
            Prim::None => {}
            Prim::Polygon(p) | Prim::ClosedLine(p) => p.push(vert),
            // IrisGL tmesh: each new vertex makes a triangle with the two
            // saved ones and then replaces the older of them.
            Prim::TMesh { saved, older } => {
                if saved.len() < 2 {
                    saved.push(vert);
                } else {
                    let mut t = [saved[0], saved[1], vert];
                    if flat {
                        t[0].color = vert.color;
                        t[1].color = vert.color;
                    }
                    tri = Some(t);
                    saved[*older] = vert;
                    *older ^= 1;
                }
            }
        }
        if let Some(t) = tri {
            self.push_op_triangles(&t);
        }
    }
}

fn lookup<T: Copy>(v: &[(i16, T)], index: i16) -> Option<T> {
    v.iter().find(|e| e.0 == index).map(|e| e.1)
}

fn store<T>(v: &mut Vec<(i16, T)>, index: i16, x: T) {
    match v.iter_mut().find(|e| e.0 == index) {
        Some(e) => e.1 = x,
        None => v.push((index, x)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn transform_order_matches_irisgl() {
        // translate then rotate: the rotation applies to the vertex first.
        let mut gl = Gl::new(false);
        gl.mmode(2);
        gl.translate(1.0, 0.0, 0.0);
        gl.rotate(900, 'z');
        assert!(close(
            transform_point(gl.modelview(), [1.0, 0.0, 0.0]),
            [1.0, 1.0, 0.0]
        ));
        gl.pushmatrix();
        gl.scale(2.0, 2.0, 1.0);
        assert!(close(
            transform_point(gl.modelview(), [1.0, 0.0, 0.0]),
            [1.0, 2.0, 0.0]
        ));
        gl.popmatrix();
        assert!(close(
            transform_point(gl.modelview(), [1.0, 0.0, 0.0]),
            [1.0, 1.0, 0.0]
        ));
    }

    #[test]
    fn rotations_are_right_handed() {
        assert!(close(
            transform_point(&rotation(900, 'x'), [0.0, 1.0, 0.0]),
            [0.0, 0.0, 1.0]
        ));
        assert!(close(
            transform_point(&rotation(900, 'y'), [0.0, 0.0, 1.0]),
            [1.0, 0.0, 0.0]
        ));
        assert!(close(
            transform_point(&rotation(900, 'z'), [1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));
        assert!(close(
            transform_point(&rotation(1800, 'z'), [1.0, 2.0, 3.0]),
            [-1.0, -2.0, 3.0]
        ));
    }

    #[test]
    fn polarview_is_a_translation_when_angles_are_zero() {
        let mut gl = Gl::new(false);
        gl.mmode(2);
        gl.loadmatrix(&IDENTITY);
        gl.polarview(10.0, 0, 0, 0);
        assert_eq!(*gl.modelview(), translation(0.0, 0.0, -10.0));
    }

    #[test]
    fn flat_polygon_and_lines() {
        let mut gl = Gl::new(true);
        gl.shademodel(0);
        gl.c4f([1.0, 0.0, 0.0, 1.0]);
        gl.bgnpolygon();
        gl.v2f([0.0, 0.0]);
        gl.v2f([1.0, 0.0]);
        gl.c4f([0.0, 1.0, 0.0, 1.0]);
        gl.v2f([1.0, 1.0]);
        gl.v2f([0.0, 1.0]);
        gl.endpolygon();
        gl.bgnclosedline();
        for v in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]] {
            gl.v2f(v);
        }
        gl.endclosedline();
        let f = gl.take_frame();
        let Op::Triangles { verts, .. } = &f.ops[0] else {
            panic!()
        };
        assert_eq!(verts.len(), 6);
        assert!(verts.iter().all(|v| v.color == [0.0, 1.0, 0.0, 1.0]));
        let Op::Lines { verts, width, .. } = &f.ops[1] else {
            panic!()
        };
        assert_eq!((verts.len(), *width), (6, 1.0));
        assert_eq!(verts[4].pos, [1.0, 1.0, 0.0]);
        assert_eq!(verts[5].pos, [0.0, 0.0, 0.0]);
        assert_eq!(gl.take_calls().len(), 14);
    }

    #[test]
    fn tmesh_flat_uses_newest_vertex_and_restarts_on_bgntmesh() {
        let mut gl = Gl::new(false);
        gl.shademodel(0);
        gl.bgntmesh();
        for (i, v) in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            gl.c4f([i as f32, 0.0, 0.0, 1.0]);
            gl.v2f(v);
        }
        gl.bgntmesh();
        gl.v2f([5.0, 5.0]);
        gl.v2f([6.0, 5.0]);
        gl.endtmesh();
        let f = gl.take_frame();
        let Op::Triangles { verts, .. } = &f.ops[0] else {
            panic!()
        };
        assert_eq!(verts.len(), 6);
        assert!(verts[..3].iter().all(|v| v.color[0] == 2.0));
        assert!(verts[3..].iter().all(|v| v.color[0] == 3.0));
        // Second triangle: the older saved vertex (0,0) was replaced.
        let p: Vec<[f32; 3]> = verts[3..].iter().map(|v| v.pos).collect();
        assert_eq!(p, [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
    }

    #[test]
    fn lighting_needs_material_and_model() {
        let mut gl = Gl::new(false);
        gl.mmode(2);
        gl.lmdef(
            lm::DEFMATERIAL,
            1,
            &[
                2.0, 0.0, 0.0, 0.0, 3.0, 1.0, 0.5, 0.0, 4.0, 1.0, 1.0, 1.0, 5.0, 0.0, 7.0, 0.5, 0.0,
            ],
        );
        gl.lmdef(
            lm::DEFLIGHT,
            5,
            &[101.0, 1.0, 1.0, 1.0, 102.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        );
        gl.lmdef(lm::DEFLMODEL, 1, &[2.0, 1.0, 1.0, 1.0, 0.0]);
        gl.c4f([0.1, 0.2, 0.3, 1.0]);
        gl.n3f([0.0, 0.0, 1.0]);
        gl.lmbind(lm::MATERIAL, 1);
        gl.lmbind(lm::LIGHT0 + 5, 5);
        assert_eq!(gl.vertex_color([0.0; 3]), [0.1, 0.2, 0.3, 1.0]);
        gl.lmbind(lm::LMODEL, 1);
        assert_eq!(gl.vertex_color([0.0; 3]), [1.0, 0.5, 0.0, 0.5]);
        // A colour command after the normal turns lighting off per vertex.
        gl.c4f([0.1, 0.2, 0.3, 1.0]);
        assert_eq!(gl.vertex_color([0.0; 3]), [0.1, 0.2, 0.3, 1.0]);
    }

    #[test]
    fn pattern_clear_is_a_stipple_fill() {
        let mut gl = Gl::new(false);
        let mut pat = [0u16; 16];
        pat[2] = 0x4444;
        gl.defpattern(2, pat);
        gl.c4f([0.5, 0.5, 0.5, 1.0]);
        gl.setpattern(2);
        gl.clear();
        gl.setpattern(0);
        gl.clear();
        let f = gl.take_frame();
        assert_eq!(
            f.ops[0],
            Op::StippleFill {
                pattern: pat,
                color: [0.5, 0.5, 0.5, 1.0]
            }
        );
        assert_eq!(f.ops[1], Op::Clear([0.5, 0.5, 0.5, 1.0]));
    }
}

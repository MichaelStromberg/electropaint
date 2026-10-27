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

mod calllog;
mod light;
mod math;

use crate::frame::{DrawBuffer, DrawState, Frame, Op, Projection, Vertex};

pub use calllog::{Arg, IrisCall};
pub(crate) use light::lm;
use light::{BoundLight, LModel, Light, Material};
use math::transform_point;
pub(crate) use math::{IDENTITY, Mat4, rotation};
use math::{mat_mul, scaling, translation};

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
pub(crate) struct Gl {
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
    pub(crate) fn new(log: bool) -> Gl {
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
    pub(crate) fn take_frame(&mut self) -> Frame {
        Frame {
            ops: std::mem::take(&mut self.ops),
        }
    }

    /// Take the call log recorded so far (empty when logging is off).
    pub(crate) fn take_calls(&mut self) -> Vec<IrisCall> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Record a call when logging is on. The GL calls here log themselves;
    /// calls with no GL state effect (window management, `srand48`,
    /// `drand48`, …) are logged by their callers.
    pub(crate) fn log(&mut self, name: &'static str, args: impl FnOnce() -> Vec<Arg>) {
        if let Some(l) = self.log.as_mut() {
            l.push(IrisCall { name, args: args() });
        }
    }

    /// The modelview matrix in force.
    pub(crate) fn modelview(&self) -> &Mat4 {
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

    pub(crate) fn doublebuffer(&mut self) {
        self.log("doublebuffer", Vec::new);
    }

    pub(crate) fn rgbmode(&mut self) {
        self.log("RGBmode", Vec::new);
    }

    pub(crate) fn gconfig(&mut self) {
        self.log("gconfig", Vec::new);
    }

    pub(crate) fn subpixel(&mut self, on: bool) {
        self.log("subpixel", || vec![Arg::Int(on.into())]);
    }

    /// `mmode(MVIEWING = 2)` selects separate projection and modelview
    /// matrices. ep (a lit build) sets it before any matrix call, so only
    /// MVIEWING is modelled.
    pub(crate) fn mmode(&mut self, mode: i16) {
        self.log("mmode", || vec![Arg::Int(mode.into())]);
    }

    pub(crate) fn blendfunction(&mut self, sfactor: i32, dfactor: i32) {
        self.log("blendfunction", || {
            vec![Arg::Int(sfactor.into()), Arg::Int(dfactor.into())]
        });
        // BF_ONE, BF_ZERO is "no blending"; ep only ever sets BF_SA, BF_MSA.
        self.blend = !(sfactor == 1 && dfactor == 0);
    }

    /// `shademodel(FLAT = 0 | GOURAUD = 1)`.
    pub(crate) fn shademodel(&mut self, mode: i32) {
        self.log("shademodel", || vec![Arg::Int(mode.into())]);
        self.flat = mode == 0;
    }

    pub(crate) fn concave(&mut self, on: bool) {
        self.log("concave", || vec![Arg::Int(on.into())]);
    }

    pub(crate) fn glcompat(&mut self, what: i32, value: i32) {
        self.log("glcompat", || {
            vec![Arg::Int(what.into()), Arg::Int(value.into())]
        });
    }

    pub(crate) fn lsetdepth(&mut self, near: i32, far: i32) {
        self.log("lsetdepth", || {
            vec![Arg::Int(near.into()), Arg::Int(far.into())]
        });
    }

    pub(crate) fn reshapeviewport(&mut self) {
        self.log("reshapeviewport", Vec::new);
    }

    pub(crate) fn zbuffer(&mut self, on: bool) {
        self.log("zbuffer", || vec![Arg::Int(on.into())]);
        self.zbuffer = on;
    }

    pub(crate) fn frontbuffer(&mut self, on: bool) {
        self.log("frontbuffer", || vec![Arg::Int(on.into())]);
        self.front = on;
        self.ops.push(Op::SetDrawBuffer(self.draw_buffer()));
    }

    pub(crate) fn backbuffer(&mut self, on: bool) {
        self.log("backbuffer", || vec![Arg::Int(on.into())]);
        self.back = on;
        self.ops.push(Op::SetDrawBuffer(self.draw_buffer()));
    }

    pub(crate) fn linewidth(&mut self, w: i16) {
        self.log("linewidth", || vec![Arg::Int(w.into())]);
        self.line_width = w;
    }

    /// `defpattern(index, size, words)`: rows from the bottom, bit 15 is the
    /// leftmost pixel. The tracer logs the pointer only.
    pub(crate) fn defpattern(&mut self, index: i16, words: [u16; 16]) {
        self.log("defpattern", || {
            vec![Arg::Int(index.into()), Arg::Int(16), Arg::Ptr]
        });
        match self.patterns.iter_mut().find(|p| p.0 == index) {
            Some(p) => p.1 = words,
            None => self.patterns.push((index, words)),
        }
    }

    pub(crate) fn setpattern(&mut self, index: i16) {
        self.log("setpattern", || vec![Arg::Int(index.into())]);
        self.pattern = index;
    }

    pub(crate) fn mapcolor(&mut self, index: u16, r: i16, g: i16, b: i16) {
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

    pub(crate) fn perspective(&mut self, fovy_tenths: i16, aspect: f32, near: f32, far: f32) {
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

    pub(crate) fn loadmatrix(&mut self, m: &Mat4) {
        self.log("loadmatrix", || vec![Arg::Floats(m.to_vec())]);
        *self.modelview_mut() = *m;
    }

    /// `polarview(dist, azim, inc, twist)` = `translate(0,0,-dist)`,
    /// `rotate(-twist,'z')`, `rotate(-inc,'x')`, `rotate(-azim,'z')`.
    pub(crate) fn polarview(&mut self, dist: f32, azim: i16, inc: i16, twist: i16) {
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

    pub(crate) fn pushmatrix(&mut self) {
        self.log("pushmatrix", Vec::new);
        let top = *self.modelview();
        self.stack.push(top);
    }

    pub(crate) fn popmatrix(&mut self) {
        self.log("popmatrix", Vec::new);
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// `rotate(a, axis)`: `a` in tenths of a degree, axis `'x'`, `'y'` or `'z'`.
    pub(crate) fn rotate(&mut self, tenths: i16, axis: char) {
        self.log("rotate", || {
            vec![Arg::Int(tenths.into()), Arg::Int(i64::from(axis as u32))]
        });
        self.post_multiply(&rotation(tenths, axis));
    }

    pub(crate) fn translate(&mut self, x: f32, y: f32, z: f32) {
        self.log("translate", || {
            vec![Arg::Float(x), Arg::Float(y), Arg::Float(z)]
        });
        self.post_multiply(&translation(x, y, z));
    }

    pub(crate) fn scale(&mut self, x: f32, y: f32, z: f32) {
        self.log("scale", || {
            vec![Arg::Float(x), Arg::Float(y), Arg::Float(z)]
        });
        self.post_multiply(&scaling(x, y, z));
    }

    // ---------------------------------------------------------------- colour

    pub(crate) fn c4f(&mut self, c: [f32; 4]) {
        self.log("c4f", || vec![Arg::Floats(c.to_vec())]);
        self.color = c;
        self.normal_last = false;
    }

    /// `color(index)`: a colour-index command. ep is in RGB mode; see
    /// [`RgbColorIndex`].
    pub(crate) fn color(&mut self, index: u16) {
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

    pub(crate) fn n3f(&mut self, n: [f32; 3]) {
        self.log("n3f", || vec![Arg::Floats(n.to_vec())]);
        self.normal = n;
        self.normal_last = true;
    }

    // ---------------------------------------------------------------- buffers

    /// `clear()`: fill the viewport with the current colour, through the
    /// current pattern if one is set; blending and z are ignored.
    pub(crate) fn clear(&mut self) {
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

    pub(crate) fn zclear(&mut self) {
        self.log("zclear", Vec::new);
        self.ops.push(Op::ZClear);
    }

    pub(crate) fn swapbuffers(&mut self) {
        self.log("swapbuffers", Vec::new);
        self.ops.push(Op::Swap);
    }

    // ---------------------------------------------------------------- primitives

    pub(crate) fn bgnpolygon(&mut self) {
        self.log("bgnpolygon", Vec::new);
        self.prim = Prim::Polygon(Vec::new());
    }

    /// Polygons are convex here (`concave(FALSE)`): a fan. Flat shading
    /// gives every triangle the colour of the polygon's last vertex.
    pub(crate) fn endpolygon(&mut self) {
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

    pub(crate) fn bgnclosedline(&mut self) {
        self.log("bgnclosedline", Vec::new);
        self.prim = Prim::ClosedLine(Vec::new());
    }

    /// A closed line loop as segments; flat shading colours each segment
    /// with its second vertex.
    pub(crate) fn endclosedline(&mut self) {
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
    pub(crate) fn bgntmesh(&mut self) {
        self.log("bgntmesh", Vec::new);
        self.prim = Prim::TMesh {
            saved: Vec::new(),
            older: 0,
        };
    }

    pub(crate) fn endtmesh(&mut self) {
        self.log("endtmesh", Vec::new);
        self.prim = Prim::None;
    }

    pub(crate) fn v2f(&mut self, v: [f32; 2]) {
        self.log("v2f", || vec![Arg::Floats(v.to_vec())]);
        self.vertex([v[0], v[1], 0.0]);
    }

    pub(crate) fn v3f(&mut self, v: [f32; 3]) {
        self.log("v3f", || vec![Arg::Floats(v.to_vec())]);
        self.vertex(v);
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

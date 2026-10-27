//! electropaint Remastered: ep's strand drawn as a modern renderer would,
//! from the [`Scene`] each step records instead of its IrisGL output.
//!
//! Every tile is a bevelled slab of iridescent chrome lit by a studio of
//! soft lights (and ep's own two lights as highlights), drawn in floating
//! point with 4x multisampling and bloom, and shown with extended dynamic
//! range where the display has it. The choreography is ep's: the same
//! simulation decides where every tile goes. The shaders are in
//! `remastered.metal`.
//!
//! It needs an Apple GPU (Apple silicon): the trail is built with
//! framebuffer fetch and the multisampled targets are memoryless.

use ep_core::Frame;
use ep_core::scene::{Scene, Shape, Tile};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedLinearSRGB};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCompareFunction, MTLCreateSystemDefaultDevice, MTLDepthStencilDescriptor, MTLDevice,
    MTLDrawable, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLStorageMode,
    MTLStoreAction, MTLTexture,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

mod mesh;

use mesh::{RibbonVertex, ribbon_strip, slab};

use crate::gpu::{check_size, fits, read_back, set_bytes, texture, upload};
use crate::show::{Picture, Show};
use crate::surface::main_strand;
use crate::{Buffer, CommandBuffer, DepthState, Device, Encoder, Pipeline, RendererError, Texture};

const SAMPLES: usize = 4;
const HDR_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA16Float;
const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;
/// Snapshots are 8-bit, sRGB encoded.
const SNAPSHOT_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// ep's vertical field of view, and how far its camera stands from the
/// strand (`perspective(300, …)`, `polarview(10, …)`).
const HALF_FOVY: f32 = 15.0 * std::f32::consts::PI / 180.0;
const DISTANCE: f32 = 10.0;

/// How much of the trail a step keeps: smear, and smear with fade.
const SMEAR_KEEP: f32 = 0.965;
const FADE_KEEP: f32 = 0.82;
/// Steps a snapshot replays to build up its trail: by then the oldest has
/// faded to nothing (0.965^300 is 2e-5).
const SNAPSHOT_TRAIL: usize = 300;

/// Bloom: from values above `THRESHOLD`, over this many halvings, added in
/// at `BLOOM`.
const BLOOM_LEVELS: usize = 6;
const THRESHOLD: f32 = 1.0;
const BLOOM: f32 = 0.12;

/// Per-pass constants; layout matches `View` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
struct View {
    proj: [[f32; 4]; 4],
    background: [f32; 4],
}

/// One tile; layout matches `Instance` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
struct Instance {
    model_view: [f32; 16],
    fill: [f32; 4],
    outline: [f32; 4],
    hue: f32,
    flags: u32,
    pad: [f32; 2],
}

// The shaders' layouts: a mismatch would draw garbage, not fail.
const _: () =
    assert!(size_of::<View>() == 80 && size_of::<Instance>() == 112 && size_of::<Post>() == 64);

const FILL: u32 = 1;
const OUTLINE: u32 = 2;
const FAT: u32 = 4;

/// Post-processing constants; layout matches `Post` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Post {
    background: [f32; 4],
    texel: [f32; 2],
    decay: f32,
    threshold: f32,
    headroom: f32,
    tan_half_fov: f32,
    aspect: f32,
    bloom: f32,
    srgb: u32,
    pad: [u32; 3],
}

struct Mesh {
    buffer: Retained<Buffer>,
    count: usize,
}

struct Pipelines {
    tile: Retained<Pipeline>,
    ribbon: Retained<Pipeline>,
    accumulate: Retained<Pipeline>,
    /// The trail under the layer, in place.
    merge: Retained<Pipeline>,
    bloom_down: Retained<Pipeline>,
    bloom_up: Retained<Pipeline>,
    /// The final pass, into a floating-point drawable or an 8-bit snapshot.
    output: Retained<Pipeline>,
    snapshot: Retained<Pipeline>,
    depth: Retained<DepthState>,
}

struct Targets {
    width: usize,
    height: usize,
    /// Multisampled colour and depth, which live only in tile memory.
    msaa: Retained<Texture>,
    depth: Retained<Texture>,
    /// The strand on a transparent background, resolved.
    layer: Retained<Texture>,
    /// The strand's trail while smearing.
    trail: Retained<Texture>,
    trail_fresh: bool,
    /// Halving sizes, from half the picture down.
    bloom: Vec<Retained<Texture>>,
}

/// Draws shows' scenes into one layer, several strands side by side in one
/// shared space.
pub struct Remastered {
    device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: Pipelines,
    square: Mesh,
    triangle: Mesh,
    targets: Option<Targets>,
    /// Each strand's latest scene.
    scenes: Vec<Option<Box<Scene>>>,
    /// The background of the last picture drawn (linear).
    background: [f32; 4],
}

impl Remastered {
    /// A renderer on the system's default Metal device.
    pub fn system_default() -> Result<Self, RendererError> {
        let device = MTLCreateSystemDefaultDevice().ok_or(RendererError::NoDevice)?;
        let queue = device
            .newCommandQueue()
            .ok_or(RendererError::Create("command queue"))?;
        let pipelines = Pipelines::new(&device)?;
        let mesh = |shape| {
            let verts = slab(shape);
            Ok::<_, RendererError>(Mesh {
                buffer: upload(&device, &verts).ok_or(RendererError::Create("mesh"))?,
                count: verts.len(),
            })
        };
        Ok(Remastered {
            square: mesh(Shape::Square)?,
            triangle: mesh(Shape::Triangle)?,
            device,
            queue,
            pipelines,
            targets: None,
            scenes: Vec::new(),
            background: [0.0; 4],
        })
    }

    /// Lets go of the picture and its memory while another style draws; the
    /// next picture starts afresh.
    pub(crate) fn release(&mut self) {
        self.targets = None;
        self.scenes.clear();
    }

    /// Configures a layer for this renderer: floating point, linear,
    /// extended range (values over 1 are brighter than SDR white).
    pub(crate) fn configure_layer(&self, layer: &CAMetalLayer) {
        layer.setDevice(Some(&self.device));
        layer.setPixelFormat(HDR_FORMAT);
        layer.setFramebufferOnly(true);
        layer.setWantsExtendedDynamicRangeContent(true);
        // SAFETY: a constant Core Graphics defines.
        let space: Option<CFRetained<CGColorSpace>> =
            CGColorSpace::with_name(Some(unsafe { kCGColorSpaceExtendedLinearSRGB }));
        layer.setColorspace(space.as_deref());
    }

    /// Advances each show to `time` seconds and, if any has something new,
    /// draws them all and presents. `headroom` is how far above SDR white
    /// the display can go (1 without EDR).
    pub(crate) fn render(
        &mut self,
        layer: &CAMetalLayer,
        shows: Vec<&mut Show>,
        time: f64,
        headroom: f64,
    ) {
        self.scenes.resize(shows.len(), None);
        let (mut changed, mut steps) = (false, 0);
        for (show, latest) in shows.into_iter().zip(&mut self.scenes) {
            match show.picture(time) {
                Picture::Ticks(frames) => {
                    steps = steps.max(frames.len());
                    if let Some(scene) = frames.into_iter().rev().find_map(|f| f.scene) {
                        *latest = Some(scene);
                    }
                    changed = true;
                }
                Picture::Between(frame) => {
                    if let Some(scene) = frame.scene {
                        *latest = Some(scene);
                        changed = true;
                    }
                }
                Picture::Same => {}
            }
        }
        let size = layer.drawableSize();
        let (width, height) = (size.width as usize, size.height as usize);
        let resized = self
            .targets
            .as_ref()
            .is_none_or(|t| (t.width, t.height) != (width, height));
        if !(changed || resized) || !self.fit(width, height) {
            return;
        }
        let Some(drawable) = layer.nextDrawable() else {
            return;
        };
        let Some(cb) = self.queue.commandBuffer() else {
            return;
        };
        self.encode_step(&cb, steps);
        self.encode_output(&cb, &drawable.texture(), false, headroom as f32);
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
        cb.presentDrawable(drawable);
        cb.commit();
    }

    /// Plays each strand's frames (one list per strand, in step) and returns
    /// the last picture as `width` x `height` BGRA8 rows, top row first,
    /// sRGB encoded for an SDR display.
    ///
    /// # Errors
    ///
    /// If a side is 0 or over 16384 pixels, or Metal can't allocate what
    /// the picture needs.
    pub fn render_offscreen(
        &mut self,
        width: usize,
        height: usize,
        strands: &[Vec<Frame>],
    ) -> Result<Vec<u8>, RendererError> {
        check_size(width, height)?;
        if !self.fit(width, height) {
            return Err(RendererError::Create("render targets"));
        }
        if let Some(t) = &mut self.targets {
            t.trail_fresh = true;
        }
        self.scenes = vec![None; strands.len()];
        let steps = strands.iter().map(Vec::len).max().unwrap_or(0);
        let first = steps.saturating_sub(SNAPSHOT_TRAIL);
        for step in 0..steps {
            for (strand, latest) in strands.iter().zip(&mut self.scenes) {
                if let Some(scene) = strand.get(step).and_then(|f| f.scene.clone()) {
                    *latest = Some(scene);
                }
            }
            if step >= first && step + 1 < steps {
                let cb = self
                    .queue
                    .commandBuffer()
                    .ok_or(RendererError::Create("command buffer"))?;
                self.encode_step(&cb, 1);
                cb.commit();
            }
        }
        let cb = self
            .queue
            .commandBuffer()
            .ok_or(RendererError::Create("command buffer"))?;
        self.encode_step(&cb, 1);
        let target = texture(
            &self.device,
            SNAPSHOT_FORMAT,
            width,
            height,
            1,
            MTLStorageMode::Private,
        )
        .ok_or(RendererError::Create("snapshot target"))?;
        self.encode_output(&cb, &target, true, 1.0);
        read_back(&self.device, &cb, &target)
    }

    /// Makes the targets `width` x `height` if they aren't; false if Metal
    /// can't.
    fn fit(&mut self, width: usize, height: usize) -> bool {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| (t.width, t.height) == (width, height))
        {
            return true;
        }
        self.targets = Targets::new(&self.device, width, height);
        self.targets.is_some()
    }

    /// Draws the strands' current scenes into the layer. Strands that smear
    /// are drawn first, over what `steps` steps leave of their trail, and the
    /// trail goes under the others.
    fn encode_step(&mut self, cb: &CommandBuffer, steps: usize) {
        let Some((width, height)) = self.targets.as_ref().map(|t| (t.width, t.height)) else {
            return;
        };
        let aspect = width as f32 / height as f32;
        let gathered = gather(&self.scenes, aspect);
        self.background = gathered.background;
        let view = View {
            proj: perspective(aspect),
            background: gathered.background,
        };
        let instances = upload(&self.device, &gathered.instances);
        let ribbons = upload(&self.device, &gathered.ribbons);
        let buffers = (instances.as_deref(), ribbons.as_deref());
        let Some(t) = &self.targets else {
            return;
        };
        let [smearing, still] = gathered.groups;
        let trail = !smearing.is_empty();
        if trail {
            self.encode_strand(cb, t, &view, smearing, buffers);
            let keep = if gathered.fade { FADE_KEEP } else { SMEAR_KEEP };
            let post = Post {
                decay: if t.trail_fresh {
                    0.0
                } else {
                    keep.powi(steps as i32)
                },
                ..Post::default()
            };
            let load = if t.trail_fresh {
                MTLLoadAction::Clear
            } else {
                MTLLoadAction::Load
            };
            if let Some(enc) = encoder(cb, &t.trail, load) {
                fullscreen(&enc, &self.pipelines.accumulate, &[&t.layer], &post);
                enc.endEncoding();
            }
        }
        self.encode_strand(cb, t, &view, still, buffers);
        if trail && let Some(enc) = encoder(cb, &t.layer, MTLLoadAction::Load) {
            fullscreen(&enc, &self.pipelines.merge, &[&t.trail], &Post::default());
            enc.endEncoding();
        }
        if let Some(t) = &mut self.targets {
            t.trail_fresh = !trail;
        }
    }

    /// Draws `group` of the instances and ribbon vertices in `buffers` into
    /// the layer, which starts transparent.
    fn encode_strand(
        &self,
        cb: &CommandBuffer,
        t: &Targets,
        view: &View,
        group: Group,
        buffers: (Option<&Buffer>, Option<&Buffer>),
    ) {
        let (instances, ribbons) = buffers;
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: attachment 0 always exists (Metal supports at least eight).
        let att = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        att.setTexture(Some(&t.msaa));
        att.setResolveTexture(Some(&t.layer));
        att.setLoadAction(MTLLoadAction::Clear);
        att.setClearColor(MTLClearColor {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.0,
        });
        att.setStoreAction(MTLStoreAction::MultisampleResolve);
        let depth = pass.depthAttachment();
        depth.setTexture(Some(&t.depth));
        depth.setLoadAction(MTLLoadAction::Clear);
        depth.setClearDepth(1.0);
        depth.setStoreAction(MTLStoreAction::DontCare);
        let Some(enc) = cb.renderCommandEncoderWithDescriptor(&pass) else {
            return;
        };
        enc.setDepthStencilState(Some(&self.pipelines.depth));
        set_bytes(&enc, view, 2, 0);
        if let Some(instances) = instances {
            enc.setRenderPipelineState(&self.pipelines.tile);
            for (mesh, first, n) in [
                (&self.square, group.first, group.squares),
                (&self.triangle, group.first + group.squares, group.triangles),
            ] {
                if n == 0 {
                    continue;
                }
                // SAFETY: the mesh holds `mesh.count` vertices and the instance
                // buffer holds instances `first..first + n` (`gather`).
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(&mesh.buffer), 0, 0);
                    enc.setVertexBuffer_offset_atIndex(
                        Some(instances),
                        first * size_of::<Instance>(),
                        1,
                    );
                    enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        mesh.count,
                        n,
                    );
                }
            }
        }
        if let Some(ribbons) = ribbons
            && group.ribbon_count > 0
        {
            enc.setRenderPipelineState(&self.pipelines.ribbon);
            // SAFETY: the buffer holds vertices `ribbon_first..ribbon_first +
            // ribbon_count`, whole triangles (`gather`).
            unsafe {
                enc.setVertexBuffer_offset_atIndex(Some(ribbons), 0, 0);
                enc.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::Triangle,
                    group.ribbon_first,
                    group.ribbon_count,
                );
            }
        }
        enc.endEncoding();
    }

    /// Bloom and the final picture into `target`.
    fn encode_output(&self, cb: &CommandBuffer, target: &Texture, srgb: bool, headroom: f32) {
        let Some(t) = &self.targets else {
            return;
        };
        let strand = &t.layer;
        let background = self.background;
        let texel = |tex: &Texture| [1.0 / tex.width() as f32, 1.0 / tex.height() as f32];

        let mut source: &Texture = strand;
        for (level, dest) in t.bloom.iter().enumerate() {
            let post = Post {
                texel: texel(source),
                threshold: if level == 0 { THRESHOLD } else { 0.0 },
                ..Post::default()
            };
            if let Some(enc) = encoder(cb, dest, MTLLoadAction::DontCare) {
                fullscreen(&enc, &self.pipelines.bloom_down, &[source], &post);
                enc.endEncoding();
            }
            source = dest;
        }
        for pair in t.bloom.windows(2).rev() {
            let post = Post {
                texel: texel(&pair[1]),
                ..Post::default()
            };
            if let Some(enc) = encoder(cb, &pair[0], MTLLoadAction::Load) {
                fullscreen(&enc, &self.pipelines.bloom_up, &[&pair[1]], &post);
                enc.endEncoding();
            }
        }

        let post = Post {
            background,
            headroom,
            tan_half_fov: HALF_FOVY.tan(),
            aspect: t.width as f32 / t.height as f32,
            bloom: BLOOM,
            srgb: u32::from(srgb),
            ..Post::default()
        };
        let pipeline = if srgb {
            &self.pipelines.snapshot
        } else {
            &self.pipelines.output
        };
        if let Some(enc) = encoder(cb, target, MTLLoadAction::DontCare) {
            fullscreen(&enc, pipeline, &[strand, &t.bloom[0]], &post);
            enc.endEncoding();
        }
    }
}

/// A run of the gathered instances and ribbon vertices: squares, then
/// triangles, from `first`; ribbon vertices from `ribbon_first`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Group {
    first: usize,
    squares: usize,
    triangles: usize,
    ribbon_first: usize,
    ribbon_count: usize,
}

impl Group {
    fn is_empty(&self) -> bool {
        self.squares + self.triangles + self.ribbon_count == 0
    }
}

/// The strands' scenes, ready for the GPU.
#[derive(Default)]
struct Gathered {
    instances: Vec<Instance>,
    ribbons: Vec<RibbonVertex>,
    /// The strands that smear, and the others.
    groups: [Group; 2],
    /// The middle strand's background (linear), or the first one's there is.
    background: [f32; 4],
    /// Whether the smearing strands fade.
    fade: bool,
}

/// The tiles and ribbons of `scenes` (one per strand, left to right), each
/// strand moved sideways to the middle of its share of an `aspect`-wide view.
fn gather(scenes: &[Option<Box<Scene>>], aspect: f32) -> Gathered {
    let mut out = Gathered::default();
    let count = scenes.len().max(1);
    let half_width = DISTANCE * HALF_FOVY.tan() * aspect;
    let offset =
        |index: usize| ((2 * index + 1) as f32 / count as f32).mul_add(half_width, -half_width);
    out.background = scenes
        .get(main_strand(count))
        .and_then(Option::as_deref)
        .or_else(|| scenes.iter().find_map(Option::as_deref))
        .map_or([0.0; 4], |s| linear(s.background));
    for (group, smearing) in out.groups.iter_mut().zip([true, false]) {
        let strands = || {
            scenes
                .iter()
                .enumerate()
                .filter_map(|(i, s)| Some((offset(i), s.as_deref()?)))
                .filter(move |(_, s)| s.smear == smearing)
        };
        group.first = out.instances.len();
        for (shape, n) in [
            (Shape::Square, &mut group.squares),
            (Shape::Triangle, &mut group.triangles),
        ] {
            let before = out.instances.len();
            for (dx, scene) in strands() {
                let tiles = scene.tiles.iter().filter(|t| t.shape == shape);
                out.instances.extend(tiles.map(|t| instance(t, dx)));
            }
            *n = out.instances.len() - before;
        }
        group.ribbon_first = out.ribbons.len();
        for (dx, scene) in strands() {
            out.fade |= smearing && scene.fade;
            for ribbon in &scene.ribbons {
                ribbon_strip(&mut out.ribbons, ribbon, dx);
            }
        }
        group.ribbon_count = out.ribbons.len() - group.ribbon_first;
    }
    out
}

/// A tile as the shaders take it, moved `dx` sideways in eye space.
fn instance(tile: &Tile, dx: f32) -> Instance {
    let mut model_view = tile.model_view;
    for column in model_view.as_chunks_mut::<4>().0 {
        column[0] = dx.mul_add(column[3], column[0]);
    }
    let flags = [
        (tile.fill.is_some(), FILL),
        (tile.outline.is_some(), OUTLINE),
        (tile.fat, FAT),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0, |flags, (_, bit)| flags | bit);
    Instance {
        model_view,
        fill: linear(tile.fill.unwrap_or_default()),
        outline: linear(tile.outline.unwrap_or_default()),
        hue: tile.hue,
        flags,
        pad: [0.0; 2],
    }
}

/// A render pass into `texture` alone.
fn encoder(
    cb: &CommandBuffer,
    texture: &Texture,
    load: MTLLoadAction,
) -> Option<Retained<Encoder>> {
    let pass = MTLRenderPassDescriptor::new();
    // SAFETY: attachment 0 always exists (Metal supports at least eight).
    let att = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    att.setTexture(Some(texture));
    att.setLoadAction(load);
    att.setClearColor(MTLClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    });
    att.setStoreAction(MTLStoreAction::Store);
    cb.renderCommandEncoderWithDescriptor(&pass)
}

/// A full-screen pass: `textures` bound in order, `post` as buffer 0.
fn fullscreen(enc: &Encoder, pipeline: &Pipeline, textures: &[&Texture], post: &Post) {
    enc.setRenderPipelineState(pipeline);
    for (index, tex) in textures.iter().enumerate() {
        // SAFETY: the shaders read each texture at its own index, as a
        // float texture.
        unsafe { enc.setFragmentTexture_atIndex(Some(tex), index) };
    }
    set_bytes(enc, post, 0, 0);
    // SAFETY: the screen shader makes its triangle from the vertex id.
    unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
}

/// ep's colours are display (sRGB-like) values; shading wants linear ones.
fn linear(c: [f32; 4]) -> [f32; 4] {
    let l = |v: f32| v.max(0.0).powf(2.2);
    [l(c[0]), l(c[1]), l(c[2]), c[3]]
}

/// ep's projection (30 degree vertical field) at the view's aspect, with
/// Metal's [0, 1] depth, column-major.
fn perspective(aspect: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / HALF_FOVY.tan();
    // ep's camera is 10 units from the strand, which can reach far beyond
    // that either way.
    let (n, far) = (0.05, 1e4);
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (n - far), -1.0],
        [0.0, 0.0, n * far / (n - far), 0.0],
    ]
}

impl Targets {
    /// `None` if Metal can't allocate them, or won't at that size.
    fn new(device: &Device, width: usize, height: usize) -> Option<Self> {
        if !fits(width, height) {
            return None;
        }
        let memoryless = |format| {
            texture(
                device,
                format,
                width,
                height,
                SAMPLES,
                MTLStorageMode::Memoryless,
            )
        };
        let full = || {
            texture(
                device,
                HDR_FORMAT,
                width,
                height,
                1,
                MTLStorageMode::Private,
            )
        };
        let mut bloom = Vec::new();
        let (mut w, mut h) = (width, height);
        while bloom.len() < BLOOM_LEVELS && w > 1 && h > 1 {
            (w, h) = (w.div_ceil(2), h.div_ceil(2));
            bloom.push(texture(
                device,
                HDR_FORMAT,
                w,
                h,
                1,
                MTLStorageMode::Private,
            )?);
        }
        if bloom.is_empty() {
            bloom.push(full()?);
        }
        Some(Targets {
            width,
            height,
            msaa: memoryless(HDR_FORMAT)?,
            depth: memoryless(DEPTH_FORMAT)?,
            layer: full()?,
            trail: full()?,
            trail_fresh: true,
            bloom,
        })
    }
}

impl Pipelines {
    fn new(device: &Device) -> Result<Self, RendererError> {
        let source = NSString::from_str(include_str!("remastered.metal"));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .map_err(|e| RendererError::ShaderCompile(e.to_string()))?;
        let func = |name: &str| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or(RendererError::Create("shader function"))
        };
        let pipeline = |vertex: &str,
                        fragment: &str,
                        format: MTLPixelFormat,
                        samples: usize,
                        additive: bool| {
            let desc = MTLRenderPipelineDescriptor::new();
            let (vertex, fragment) = (func(vertex)?, func(fragment)?);
            desc.setVertexFunction(Some(&vertex));
            desc.setFragmentFunction(Some(&fragment));
            desc.setRasterSampleCount(samples);
            if samples > 1 {
                desc.setDepthAttachmentPixelFormat(DEPTH_FORMAT);
            }
            // SAFETY: attachment 0 always exists (Metal supports at least eight).
            let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            att.setPixelFormat(format);
            if additive {
                att.setBlendingEnabled(true);
                att.setSourceRGBBlendFactor(MTLBlendFactor::One);
                att.setDestinationRGBBlendFactor(MTLBlendFactor::One);
                att.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                att.setDestinationAlphaBlendFactor(MTLBlendFactor::One);
            }
            device
                .newRenderPipelineStateWithDescriptor_error(&desc)
                .map_err(|e| RendererError::Pipeline(e.to_string()))
        };
        let depth = MTLDepthStencilDescriptor::new();
        depth.setDepthCompareFunction(MTLCompareFunction::Less);
        depth.setDepthWriteEnabled(true);
        Ok(Pipelines {
            tile: pipeline("tile_vertex", "shade_fragment", HDR_FORMAT, SAMPLES, false)?,
            ribbon: pipeline(
                "ribbon_vertex",
                "shade_fragment",
                HDR_FORMAT,
                SAMPLES,
                false,
            )?,
            accumulate: pipeline("screen_vertex", "accumulate_fragment", HDR_FORMAT, 1, false)?,
            merge: pipeline("screen_vertex", "merge_fragment", HDR_FORMAT, 1, false)?,
            bloom_down: pipeline("screen_vertex", "bloom_down_fragment", HDR_FORMAT, 1, false)?,
            bloom_up: pipeline("screen_vertex", "bloom_up_fragment", HDR_FORMAT, 1, true)?,
            output: pipeline("screen_vertex", "final_fragment", HDR_FORMAT, 1, false)?,
            snapshot: pipeline("screen_vertex", "final_fragment", SNAPSHOT_FORMAT, 1, false)?,
            depth: device
                .newDepthStencilStateWithDescriptor(&depth)
                .ok_or(RendererError::Create("depth state"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(shape: Shape) -> Tile {
        let mut model_view = [0.0; 16];
        for i in 0..4 {
            model_view[i * 5] = 1.0;
        }
        Tile {
            model_view,
            shape,
            fill: Some([1.0; 4]),
            outline: None,
            fat: false,
            hue: 0.5,
            light: 0.5,
            age: 0,
            copy: 0,
        }
    }

    /// A strand's slot: a scene with a triangle and a square.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "a strand's slot, which can also be empty"
    )]
    fn scene(smear: bool, background: f32) -> Option<Box<Scene>> {
        Some(Box::new(Scene {
            tiles: vec![tile(Shape::Triangle), tile(Shape::Square)],
            background: [background, 0.0, 0.0, 1.0],
            smear,
            ..Scene::default()
        }))
    }

    /// Where each instance was moved sideways to.
    fn offsets(g: &Gathered) -> Vec<f32> {
        g.instances.iter().map(|i| i.model_view[12]).collect()
    }

    #[test]
    fn strands_spread_across_the_view_squares_first() {
        let half = DISTANCE * HALF_FOVY.tan() * 3.0;
        let g = gather(
            &[scene(false, 0.0), scene(false, 1.0), scene(false, 0.0)],
            3.0,
        );
        let [smearing, still] = g.groups;
        assert!(smearing.is_empty());
        assert_eq!((still.first, still.squares, still.triangles), (0, 3, 3));
        let third = half * 2.0 / 3.0;
        for (got, want) in offsets(&g)
            .iter()
            .zip([-third, 0.0, third, -third, 0.0, third])
        {
            assert!((got - want).abs() < 1e-4, "{got} {want}");
        }
        // The middle strand's background.
        assert_eq!(g.background[0], 1.0);
        let one = gather(&[scene(false, 0.0)], 1.25);
        assert_eq!(offsets(&one), [0.0, 0.0]);
    }

    #[test]
    fn only_strands_that_smear_leave_trails() {
        let g = gather(&[scene(false, 0.0), scene(true, 0.0), None], 2.0);
        let [smearing, still] = g.groups;
        assert_eq!(
            (smearing.first, smearing.squares, smearing.triangles),
            (0, 1, 1)
        );
        assert_eq!((still.first, still.squares, still.triangles), (2, 1, 1));
        // The middle strand has a scene; with none there, the first that does.
        let g = gather(&[scene(false, 0.5), None, scene(false, 1.0)], 2.0);
        assert_eq!(g.background[0], linear([0.5; 4])[0]);
    }
}

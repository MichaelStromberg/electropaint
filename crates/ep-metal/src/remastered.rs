//! electropaint Remastered: ep's strand drawn as a modern renderer would,
//! from the [`Scene`] each step records instead of its IrisGL output.
//!
//! Every tile is a bevelled slab of iridescent chrome lit by a studio of
//! soft lights (and ep's own two lights as highlights), drawn in floating
//! point with 4x multisampling and bloom, and shown with extended dynamic
//! range where the display has it. The choreography is ep's: the same
//! simulation decides where every tile goes. The shaders are in
//! `remastered.metal`.

use std::ffi::c_void;
use std::ptr::NonNull;

use ep_core::Frame;
use ep_core::scene::{Scene, Shape, Tile};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedLinearSRGB};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCompareFunction, MTLCreateSystemDefaultDevice,
    MTLDepthStencilDescriptor, MTLDevice, MTLDrawable, MTLLibrary, MTLLoadAction, MTLOrigin,
    MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLResourceOptions, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use crate::show::{Picture, Show};
use crate::{DepthState, Device, Encoder, MAX_SIDE, Pipeline, RendererError, Texture};

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

/// The slab: its edge radius (half its thickness), the radius of its
/// corners seen from the front, and how many segments make each curve.
const EDGE_RADIUS: f32 = 0.007;
const CORNER_RADIUS: f32 = 0.018;
const CORNER_SEGMENTS: usize = 4;
const EDGE_SEGMENTS: usize = 4;
/// How far a face's normals lean out at its edge (the tangent of the angle).
const DOME: f32 = 0.35;

/// Per-pass constants; layout matches `View` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
struct View {
    proj: [[f32; 4]; 4],
    background: [f32; 4],
}

/// Layout matches `MeshVertex` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
struct MeshVertex {
    /// Model space; w is how near the rim (0 at the centre, 1 at the edge).
    pos: [f32; 4],
    normal: [f32; 4],
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

const FILL: u32 = 1;
const OUTLINE: u32 = 2;
const FAT: u32 = 4;

/// Layout matches `RibbonVertex` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
struct RibbonVertex {
    pos: [f32; 4],
    /// w is the hue the film thickness follows.
    normal: [f32; 4],
    colour: [f32; 4],
}

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

type Buffer = ProtocolObject<dyn MTLBuffer>;
type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;

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
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
            return Err(RendererError::Size { width, height });
        }
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
        .get(count / 2)
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

/// Sets `value` as vertex buffer `vertex` and fragment buffer `fragment`.
fn set_bytes<T: Copy>(enc: &Encoder, value: &T, vertex: usize, fragment: usize) {
    let ptr = NonNull::from(value).cast::<c_void>();
    // SAFETY: `ptr` points to a live `T` of exactly the length given, which
    // Metal copies; its `repr(C)` layout matches the shaders'.
    unsafe {
        enc.setVertexBytes_length_atIndex(ptr, size_of::<T>(), vertex);
        enc.setFragmentBytes_length_atIndex(ptr, size_of::<T>(), fragment);
    }
}

/// A shared buffer holding a copy of `items`; `None` if there are none.
fn upload<T: Copy>(device: &Device, items: &[T]) -> Option<Retained<Buffer>> {
    if items.is_empty() {
        return None;
    }
    let ptr = NonNull::from(items).cast::<c_void>();
    // SAFETY: `ptr` points to `size_of_val(items)` initialised bytes, which
    // Metal copies before returning.
    unsafe {
        device.newBufferWithBytes_length_options(
            ptr,
            size_of_val(items),
            MTLResourceOptions::StorageModeShared,
        )
    }
}

/// Waits for `cb` after copying `target` (BGRA8) into memory.
fn read_back(
    device: &Device,
    cb: &CommandBuffer,
    target: &Texture,
) -> Result<Vec<u8>, RendererError> {
    let (width, height) = (target.width(), target.height());
    let row = width * 4;
    let buf = device
        .newBufferWithLength_options(row * height, MTLResourceOptions::StorageModeShared)
        .ok_or(RendererError::Create("readback buffer"))?;
    let blit = cb
        .blitCommandEncoder()
        .ok_or(RendererError::Create("blit encoder"))?;
    // SAFETY: `target` is a `width` x `height` BGRA8 texture and `buf` holds
    // exactly `row * height` bytes; both outlive the command buffer, which
    // is waited for below.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            target,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize { width, height, depth: 1 },
            &buf,
            0,
            row,
            row * height,
        );
    }
    blit.endEncoding();
    cb.commit();
    cb.waitUntilCompleted();
    let ptr = buf.contents().as_ptr() as *const u8;
    // SAFETY: shared storage of `row * height` bytes, which the GPU finished
    // writing (`waitUntilCompleted`) and nothing else touches.
    Ok(unsafe { std::slice::from_raw_parts(ptr, row * height) }.to_vec())
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

/// Appends a ribbon's triangles: a quad between each pair of neighbouring
/// slots, flat shaded in the newer slot's colour.
fn ribbon_strip(out: &mut Vec<RibbonVertex>, ribbon: &ep_core::scene::Ribbon, dx: f32) {
    let moved = |p: [f32; 3]| [p[0] + dx, p[1], p[2], 1.0];
    for (k, pair) in ribbon.edges.windows(2).enumerate() {
        let [newer, newer_far] = pair[0];
        let [older, older_far] = pair[1];
        let across = cross(sub(newer_far, newer), sub(older, newer));
        let norm = length(across);
        if norm < 1e-9 {
            continue;
        }
        let hue = ribbon.hues.get(k).copied().unwrap_or(0.0);
        let normal = [across[0] / norm, across[1] / norm, across[2] / norm, hue];
        let colour = linear(ribbon.colours[k]);
        for p in [newer, newer_far, older_far, newer, older_far, older] {
            out.push(RibbonVertex {
                pos: moved(p),
                normal,
                colour,
            });
        }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1].mul_add(b[2], -a[2] * b[1]),
        a[2].mul_add(b[0], -a[0] * b[2]),
        a[0].mul_add(b[1], -a[1] * b[0]),
    ]
}

fn length(v: [f32; 3]) -> f32 {
    v[0].mul_add(v[0], v[1].mul_add(v[1], v[2] * v[2])).sqrt()
}

/// The tile `shape` as a closed slab: its outline with rounded corners,
/// faces front and back, and a rounded edge between them, as a triangle
/// list. The outline keeps ep's size, so the slab covers what ep's tile did.
fn slab(shape: Shape) -> Vec<MeshVertex> {
    // Both shapes have an incircle: shrinking towards its centre offsets
    // every edge inwards by the same distance.
    let (corners, centre, inradius): (&[[f32; 2]], [f32; 2], f32) = match shape {
        Shape::Square => (
            &[[-0.1, -0.1], [0.1, -0.1], [0.1, 0.1], [-0.1, 0.1]],
            [0.0, 0.0],
            0.1,
        ),
        Shape::Triangle => {
            let h = 0.173_205_08;
            (&[[0.0, 0.0], [0.2, 0.0], [0.1, h]], [0.1, h / 3.0], h / 3.0)
        }
    };
    let shrink = (inradius - CORNER_RADIUS) / inradius;
    let inner: Vec<[f32; 2]> = corners
        .iter()
        .map(|c| {
            [
                centre[0] + (c[0] - centre[0]) * shrink,
                centre[1] + (c[1] - centre[1]) * shrink,
            ]
        })
        .collect();
    // The outward normal's angle of an edge, the corners going anticlockwise.
    let edge_normal = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).atan2(b[1] - a[1]);
    // The outline: around each corner, an arc between its edges' normals.
    let mut outline: Vec<([f32; 2], [f32; 2])> = Vec::new();
    let sides = inner.len();
    for i in 0..sides {
        let prev = inner[(i + sides - 1) % sides];
        let next = inner[(i + 1) % sides];
        let a0 = edge_normal(prev, inner[i]);
        let mut a1 = edge_normal(inner[i], next);
        if a1 < a0 {
            a1 += std::f32::consts::TAU;
        }
        for k in 0..=CORNER_SEGMENTS {
            let a = a0 + (a1 - a0) * k as f32 / CORNER_SEGMENTS as f32;
            outline.push((inner[i], [a.cos(), a.sin()]));
        }
    }
    // Rings from the front face's edge round to the back face's.
    let core = CORNER_RADIUS - EDGE_RADIUS;
    let ring = |j: usize| -> Vec<MeshVertex> {
        let phi =
            std::f32::consts::FRAC_PI_2 - std::f32::consts::PI * j as f32 / EDGE_SEGMENTS as f32;
        let (s, c) = phi.sin_cos();
        outline
            .iter()
            .map(|&(p, d)| MeshVertex {
                pos: [
                    p[0] + d[0] * (core + EDGE_RADIUS * c),
                    p[1] + d[1] * (core + EDGE_RADIUS * c),
                    EDGE_RADIUS * s,
                    1.0,
                ],
                normal: [d[0] * c, d[1] * c, s, 0.0],
            })
            .collect()
    };
    let rings: Vec<Vec<MeshVertex>> = (0..=EDGE_SEGMENTS).map(ring).collect();
    let mut out = Vec::new();
    let m = outline.len();
    for pair in rings.windows(2) {
        let [this, next] = [&pair[0], &pair[1]];
        for k in 0..m {
            let k1 = (k + 1) % m;
            out.extend([this[k], this[k1], next[k1], this[k], next[k1], next[k]]);
        }
    }
    // The faces are flat, but shaded as if gently domed (their normals lean
    // outwards towards the edge), so a reflection sweeps across each tile.
    for (ring, z) in [(&rings[0], 1.0f32), (&rings[EDGE_SEGMENTS], -1.0)] {
        let middle = MeshVertex {
            pos: [centre[0], centre[1], EDGE_RADIUS * z, 0.0],
            normal: [0.0, 0.0, z, 0.0],
        };
        let domed = |v: MeshVertex| {
            let lean = [
                (v.pos[0] - centre[0]) / inradius * DOME,
                (v.pos[1] - centre[1]) / inradius * DOME,
            ];
            let scale = length([lean[0], lean[1], 1.0]);
            MeshVertex {
                normal: [lean[0] / scale, lean[1] / scale, z / scale, 0.0],
                ..v
            }
        };
        for k in 0..m {
            out.extend([middle, domed(ring[k]), domed(ring[(k + 1) % m])]);
        }
    }
    out
}

impl Targets {
    /// `None` if Metal can't allocate them, or won't at that size.
    fn new(device: &Device, width: usize, height: usize) -> Option<Self> {
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
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

fn texture(
    device: &Device,
    format: MTLPixelFormat,
    width: usize,
    height: usize,
    samples: usize,
    storage: MTLStorageMode,
) -> Option<Retained<Texture>> {
    // SAFETY: a plain descriptor; callers rule out zero sizes.
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, width, height, false,
        )
    };
    if samples > 1 {
        desc.setTextureType(MTLTextureType::Type2DMultisample);
        // SAFETY: 4x multisampling is supported by every Apple GPU.
        unsafe { desc.setSampleCount(samples) };
    }
    desc.setStorageMode(storage);
    let usage = if storage == MTLStorageMode::Memoryless {
        MTLTextureUsage::RenderTarget
    } else {
        MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead
    };
    desc.setUsage(usage);
    device.newTextureWithDescriptor(&desc)
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

    #[test]
    fn slabs_keep_eps_outline() {
        // The edges lie on ep's (the corners are rounded inside them): the
        // square's sides at x = ±0.1, the triangle's base at y = 0.
        for (shape, axis, sign, edge) in [
            (Shape::Square, 0, 1.0, 0.1),
            (Shape::Triangle, 1, -1.0, 0.0),
        ] {
            let verts = slab(shape);
            assert_eq!(verts.len() % 3, 0);
            let extreme = verts
                .iter()
                .map(|v| sign * v.pos[axis])
                .fold(f32::MIN, f32::max);
            assert!((extreme - sign * edge).abs() < 1e-4, "{shape:?}: {extreme}");
            let depth = verts.iter().map(|v| v.pos[2].abs()).fold(0.0, f32::max);
            assert!((depth - EDGE_RADIUS).abs() < 1e-6);
            for v in &verts {
                let n = v.normal;
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-4, "{n:?}");
            }
        }
    }
}

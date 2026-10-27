//! Metal renderers for electropaint [`Frame`]s: [`Renderer`], the classic
//! picture described here, and [`remastered::Remastered`], a modern one.
//! [`surface::Surface`] hosts either in a view, with one or more shows.
//!
//! The classic renderer emulates the parts of an SGI double-buffered RGB visual that ep relies
//! on: two persistent colour buffers that `swapbuffers` exchanges (so smear
//! trails survive), front/back write enables, `clear()` as an unblended
//! full-viewport fill that honours the 16x16 stipple pattern, and a z-buffer
//! that ep clears itself. Buffers are 4x multisampled and resolved for
//! display; everything is 8 bits per channel with no dither.
//!
//! Sizes from the original are scaled from its 1280x1024 screen: line widths
//! and stipple pixels grow with the drawable height. The projection keeps
//! ep's vertical field of view and takes the drawable's own aspect, so the
//! picture fills the screen instead of being letterboxed.

mod gpu;
pub mod remastered;
pub mod show;
pub mod surface;

use ep_core::frame::{DrawBuffer, DrawState, Frame, Op, Projection, Vertex};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSString, ns_string};
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder,
    MTLCommandQueue, MTLCompareFunction, MTLCreateSystemDefaultDevice, MTLDepthStencilDescriptor,
    MTLDepthStencilState, MTLDevice, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTexture,
};
use objc2_quartz_core::CAMetalLayer;

use gpu::{check_size, fits, read_back, set_bytes, texture, upload};

/// Height of the original display; line widths and stipple pixels are
/// expressed in its pixels.
const ORIGINAL_HEIGHT: f32 = 1024.0;
const SAMPLES: usize = 4;
const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;
/// The widest and tallest texture every Mac GPU supports. Metal aborts the
/// process when asked for a larger one, rather than failing the call.
const MAX_SIDE: usize = 16384;

type Device = ProtocolObject<dyn MTLDevice>;
type Texture = ProtocolObject<dyn MTLTexture>;
type Pipeline = ProtocolObject<dyn MTLRenderPipelineState>;
type DepthState = ProtocolObject<dyn MTLDepthStencilState>;
type Encoder = ProtocolObject<dyn MTLRenderCommandEncoder>;
type Buffer = ProtocolObject<dyn MTLBuffer>;
type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;

// The shaders' layout: a mismatch would draw garbage, not fail.
const _: () = assert!(size_of::<Uniforms>() == 176);

/// Per-draw constants; layout matches `Uniforms` in shaders.metal.
#[repr(C)]
#[derive(Clone, Copy)]
struct Uniforms {
    proj: [[f32; 4]; 4],
    viewport: [f32; 2],
    px_scale: f32,
    line_width: f32,
    fill_color: [f32; 4],
    pattern: [u32; 16],
    use_pattern: u32,
    _pad: [u32; 3],
}

/// One of the two IrisGL colour buffers: a multisampled render target plus
/// its single-sample resolve, which is what gets displayed.
struct ColorBuffer {
    msaa: Retained<Texture>,
    resolved: Retained<Texture>,
    /// True until the first pass has cleared it.
    fresh: bool,
}

struct Targets {
    width: usize,
    height: usize,
    color: [ColorBuffer; 2],
    depth: Retained<Texture>,
    depth_fresh: bool,
}

#[derive(Clone)]
struct Pipelines {
    tri: [Retained<Pipeline>; 2], // [opaque, blended]
    line: [Retained<Pipeline>; 2],
    fill: Retained<Pipeline>,
    zclear: Retained<Pipeline>,
    depth_test: Retained<DepthState>,
    depth_off: Retained<DepthState>,
    depth_overwrite: Retained<DepthState>,
}

/// Why a renderer (or a [`surface::Surface`]) could not be set up, or an
/// offscreen picture could not be drawn.
#[derive(Debug)]
pub enum RendererError {
    NoDevice,
    /// Metal could not create the named object.
    Create(&'static str),
    ShaderCompile(String),
    Pipeline(String),
    /// An offscreen picture with a side of 0 or over `MAX_SIDE` pixels.
    Size {
        width: usize,
        height: usize,
    },
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoDevice => f.write_str("no Metal device"),
            RendererError::Create(what) => write!(f, "could not create Metal {what}"),
            RendererError::ShaderCompile(e) => write!(f, "shader compile failed: {e}"),
            RendererError::Pipeline(e) => write!(f, "pipeline: {e}"),
            RendererError::Size { width, height } => write!(
                f,
                "a {width}x{height} picture: each side must be 1 to {MAX_SIDE} pixels"
            ),
        }
    }
}

impl std::error::Error for RendererError {}

/// Renders the frames of one show into its own pair of colour buffers,
/// which persist from frame to frame as an IrisGL visual's did.
pub struct Renderer {
    device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: Pipelines,
    targets: Option<Targets>,
    /// Index of the front buffer, the one ep's last swap displayed.
    front: usize,
    /// Index of the buffer to show: the front one, or the back one while it
    /// holds an in-between picture.
    shown: usize,
    draw_buffer: DrawBuffer,
    projection: Projection,
    /// Every vertex of the frames being encoded, kept to reuse its allocation.
    vertices: Vec<Vertex>,
}

impl Renderer {
    /// A renderer on the system's default Metal device.
    pub fn system_default() -> Result<Self, RendererError> {
        let device = MTLCreateSystemDefaultDevice().ok_or(RendererError::NoDevice)?;
        let queue = device
            .newCommandQueue()
            .ok_or(RendererError::Create("command queue"))?;
        let pipelines = Pipelines::new(&device)?;
        Ok(Renderer::with(device, queue, pipelines))
    }

    fn with(
        device: Retained<Device>,
        queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
        pipelines: Pipelines,
    ) -> Self {
        Renderer {
            device,
            queue,
            pipelines,
            targets: None,
            front: 0,
            shown: 0,
            draw_buffer: DrawBuffer::Back,
            projection: Projection {
                fovy_tenths: 300,
                aspect: 1.25,
                near: 0.01,
                far: 1e6,
            },
            vertices: Vec::new(),
        }
    }

    /// A renderer for another show, sharing this one's device, queue and
    /// pipelines (so no shaders to compile) but with buffers of its own.
    /// Work for the two goes through the one queue, in order.
    #[must_use]
    pub fn sibling(&self) -> Self {
        Renderer::with(
            self.device.clone(),
            self.queue.clone(),
            self.pipelines.clone(),
        )
    }

    /// Configures a layer for this renderer: our device, BGRA8, and a
    /// drawable we can blit into.
    pub(crate) fn configure_layer(&self, layer: &CAMetalLayer) {
        layer.setDevice(Some(&self.device));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(false);
    }

    /// Executes the frames' operations in order at `width` x `height`; the
    /// front buffer is then the one to show.
    pub(crate) fn draw(&mut self, width: usize, height: usize, frames: &[Frame]) {
        if let Some(cb) = self.encode(width, height, frames) {
            cb.commit();
            self.shown = self.front;
        }
    }

    /// Draws a picture that is not part of ep's own sequence (an in-between
    /// from [`ep_core::Sim::frame_between`]) into the back buffer, which is
    /// then the one to show, without a swap: the next real frame (which
    /// clears the back buffer first) finds the buffers as ep left them.
    pub(crate) fn draw_between(&mut self, width: usize, height: usize, frame: &Frame) {
        let saved = std::mem::replace(&mut self.draw_buffer, DrawBuffer::Back);
        let cb = self.encode(width, height, std::slice::from_ref(frame));
        self.draw_buffer = saved;
        if let Some(cb) = cb {
            cb.commit();
            self.shown = self.front ^ 1;
        }
    }

    /// Picks a show up again after missing its frames (while another style
    /// drew): drawing goes to `draw_buffer` with `projection`, as ep left
    /// them, and the buffers start black again, as after IrisGL's `gconfig`.
    pub(crate) fn resync(&mut self, draw_buffer: DrawBuffer, projection: Option<Projection>) {
        self.release();
        self.front = 0;
        self.shown = 0;
        self.draw_buffer = draw_buffer;
        if let Some(projection) = projection {
            self.projection = projection;
        }
    }

    /// Lets go of the buffers and their memory while another style draws.
    pub(crate) fn release(&mut self) {
        self.targets = None;
    }

    /// The size of the buffers, once something has been drawn.
    pub(crate) fn size(&self) -> Option<(usize, usize)> {
        self.targets.as_ref().map(|t| (t.width, t.height))
    }

    /// A command buffer on this renderer's queue, for presenting.
    pub(crate) fn command_buffer(&self) -> Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        self.queue.commandBuffer()
    }

    /// Copies the buffer to show into `dest` with its left edge at `x`, if it
    /// fits there.
    pub(crate) fn blit_into(
        &self,
        blit: &ProtocolObject<dyn MTLBlitCommandEncoder>,
        dest: &Texture,
        x: usize,
    ) {
        let Some(targets) = &self.targets else {
            return;
        };
        let (width, height) = (targets.width, targets.height);
        if x + width > dest.width() || height > dest.height() {
            return;
        }
        // SAFETY: the region lies inside both textures (checked above), which
        // share a pixel format (`configure_layer`); the command buffer
        // retains both until it completes.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &targets.color[self.shown].resolved,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize { width, height, depth: 1 },
                dest,
                0,
                0,
                MTLOrigin { x, y: 0, z: 0 },
            );
        }
    }

    /// Executes frames into a `width` x `height` target without a window and
    /// returns the displayed (front) buffer as BGRA8 rows, top row first.
    ///
    /// # Errors
    ///
    /// If a side is 0 or over 16384 pixels, or Metal can't allocate what
    /// the picture needs. A size error leaves the renderer as it was; after
    /// an allocation error its buffers no longer follow the frames, so it
    /// should be dropped.
    pub fn render_offscreen(
        &mut self,
        width: usize,
        height: usize,
        frames: &[Frame],
    ) -> Result<Vec<u8>, RendererError> {
        check_size(width, height)?;
        let cb = self
            .encode(width, height, frames)
            .ok_or(RendererError::Create("render targets"))?;
        let targets = self
            .targets
            .as_ref()
            .ok_or(RendererError::Create("render targets"))?;
        read_back(&self.device, &cb, &targets.color[self.front].resolved)
    }

    /// Encodes the frames' operations into a new command buffer, (re)creating
    /// the targets if the size changed.
    fn encode(
        &mut self,
        w: usize,
        h: usize,
        frames: &[Frame],
    ) -> Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        if w == 0 || h == 0 {
            return None;
        }
        let cb = self.queue.commandBuffer()?;
        if self
            .targets
            .as_ref()
            .is_none_or(|t| t.width != w || t.height != h)
        {
            // Without targets (Metal refused the memory) the frames are
            // dropped, as for a zero-sized drawable.
            self.targets = Some(Targets::new(&self.device, w, h)?);
            // New buffers start black, as after IrisGL's gconfig.
            for index in 0..2 {
                self.encode_pass(&cb, None, index, &[]);
            }
        }
        let (vbuf, offsets) = self.upload_vertices(frames);
        let mut segment: Vec<(usize, &Op)> = Vec::new();
        let mut op_index = 0;
        for frame in frames {
            for op in &frame.ops {
                match op {
                    Op::SetDrawBuffer(b) => {
                        self.flush(&cb, vbuf.as_deref(), &segment);
                        segment.clear();
                        self.draw_buffer = *b;
                    }
                    Op::Swap => {
                        self.flush(&cb, vbuf.as_deref(), &segment);
                        segment.clear();
                        self.front ^= 1;
                    }
                    _ => segment.push((offsets[op_index], op)),
                }
                op_index += 1;
            }
        }
        self.flush(&cb, vbuf.as_deref(), &segment);
        Some(cb)
    }

    /// Copies every vertex of every draw into one shared buffer. Returns the
    /// buffer and, per op, its byte offset into it.
    fn upload_vertices(
        &mut self,
        frames: &[Frame],
    ) -> (Option<Retained<ProtocolObject<dyn MTLBuffer>>>, Vec<usize>) {
        let all = &mut self.vertices;
        all.clear();
        let mut offsets = Vec::new();
        for op in frames.iter().flat_map(|f| &f.ops) {
            offsets.push(all.len() * size_of::<Vertex>());
            if let Op::Triangles { verts, .. } | Op::Lines { verts, .. } = op {
                all.extend_from_slice(verts);
            }
        }
        (upload(&self.device, all), offsets)
    }

    /// Encodes a run of drawing ops into each buffer the current
    /// `frontbuffer`/`backbuffer` setting writes to.
    fn flush(
        &mut self,
        cb: &ProtocolObject<dyn MTLCommandBuffer>,
        vbuf: Option<&ProtocolObject<dyn MTLBuffer>>,
        ops: &[(usize, &Op)],
    ) {
        if ops.is_empty() {
            return;
        }
        let back = self.front ^ 1;
        let which: &[usize] = match self.draw_buffer {
            DrawBuffer::Back => &[back],
            DrawBuffer::Front => &[self.front],
            DrawBuffer::Both => &[back, self.front],
        };
        // A projection set inside the run applies from that point on, in every pass.
        let start_projection = self.projection;
        for &index in which {
            self.projection = start_projection;
            self.encode_pass(cb, vbuf, index, ops);
        }
    }

    fn encode_pass(
        &mut self,
        cb: &ProtocolObject<dyn MTLCommandBuffer>,
        vbuf: Option<&ProtocolObject<dyn MTLBuffer>>,
        index: usize,
        ops: &[(usize, &Op)],
    ) {
        let targets = self.targets.as_mut().expect("encode created the targets");
        let (w, h) = (targets.width as f32, targets.height as f32);
        let pass = MTLRenderPassDescriptor::new();
        let color = &mut targets.color[index];
        // SAFETY: attachment 0 always exists (Metal supports at least eight).
        let att = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        att.setTexture(Some(&color.msaa));
        att.setResolveTexture(Some(&color.resolved));
        att.setLoadAction(if color.fresh {
            MTLLoadAction::Clear
        } else {
            MTLLoadAction::Load
        });
        att.setStoreAction(MTLStoreAction::StoreAndMultisampleResolve);
        let depth = pass.depthAttachment();
        depth.setTexture(Some(&targets.depth));
        depth.setLoadAction(if targets.depth_fresh {
            MTLLoadAction::Clear
        } else {
            MTLLoadAction::Load
        });
        depth.setClearDepth(1.0);
        depth.setStoreAction(MTLStoreAction::Store);
        color.fresh = false;
        targets.depth_fresh = false;
        let Some(enc) = cb.renderCommandEncoderWithDescriptor(&pass) else {
            return;
        };

        let mut u = Uniforms {
            proj: projection_matrix(&self.projection, w / h),
            viewport: [w, h],
            px_scale: h / ORIGINAL_HEIGHT,
            line_width: 1.0,
            fill_color: [0.0; 4],
            pattern: [0; 16],
            use_pattern: 0,
            _pad: [0; 3],
        };
        for &(offset, op) in ops {
            let pipes = &self.pipelines;
            match op {
                Op::SetProjection(proj) => {
                    self.projection = *proj;
                    u.proj = projection_matrix(&self.projection, w / h);
                }
                Op::Clear(c) => {
                    u.fill_color = *c;
                    u.use_pattern = 0;
                    draw_fill(&enc, &pipes.fill, &pipes.depth_off, &u);
                }
                Op::StippleFill { pattern, color } => {
                    u.fill_color = *color;
                    u.pattern = pattern.map(u32::from);
                    u.use_pattern = 1;
                    draw_fill(&enc, &pipes.fill, &pipes.depth_off, &u);
                }
                Op::ZClear => draw_fill(&enc, &pipes.zclear, &pipes.depth_overwrite, &u),
                Op::Triangles { state, verts } => {
                    let Some(vbuf) = vbuf else { continue };
                    self.bind(&enc, &self.pipelines.tri, *state, vbuf, offset, &u);
                    // SAFETY: `bind` set the vertex buffer at this op's
                    // offset, where its `verts.len()` vertices start.
                    unsafe {
                        enc.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            verts.len(),
                        )
                    };
                }
                Op::Lines {
                    state,
                    width,
                    verts,
                } => {
                    let Some(vbuf) = vbuf else { continue };
                    u.line_width = *width;
                    self.bind(&enc, &self.pipelines.line, *state, vbuf, offset, &u);
                    // SAFETY: each instance is one segment, two of the
                    // `verts.len()` vertices `bind` set up; `line_vertex`
                    // makes its six corners from them.
                    unsafe {
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                            verts.len() / 2,
                        )
                    };
                }
                Op::SetDrawBuffer(_) | Op::Swap => unreachable!("handled by encode"),
            }
        }
        enc.endEncoding();
    }

    fn bind(
        &self,
        enc: &Encoder,
        pipelines: &[Retained<Pipeline>; 2],
        state: DrawState,
        vbuf: &ProtocolObject<dyn MTLBuffer>,
        offset: usize,
        u: &Uniforms,
    ) {
        enc.setRenderPipelineState(&pipelines[state.blend as usize]);
        enc.setDepthStencilState(Some(if state.depth_test {
            &self.pipelines.depth_test
        } else {
            &self.pipelines.depth_off
        }));
        // SAFETY: `offset` came from `upload_vertices`, so it lies within `vbuf`.
        unsafe { enc.setVertexBuffer_offset_atIndex(Some(vbuf), offset, 0) };
        set_bytes(enc, u, 1, 1);
    }
}

/// Fills the whole target through `pipeline` (colour or depth).
fn draw_fill(enc: &Encoder, pipeline: &Pipeline, depth: &DepthState, u: &Uniforms) {
    enc.setRenderPipelineState(pipeline);
    enc.setDepthStencilState(Some(depth));
    set_bytes(enc, u, 1, 1);
    // SAFETY: the fill shaders make their full-screen triangle from the
    // vertex id and read no vertex buffer.
    unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
}

/// IrisGL `perspective` with Metal's [0, 1] depth range, column-major.
/// The window's aspect replaces ep's, keeping the vertical field of view.
fn projection_matrix(p: &Projection, aspect: f32) -> [[f32; 4]; 4] {
    let fovy = (p.fovy_tenths as f32 / 10.0).to_radians();
    let f = 1.0 / (fovy / 2.0).tan();
    let (n, far) = (p.near, p.far);
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
        let buffer = || {
            Some(ColorBuffer {
                msaa: texture(
                    device,
                    COLOR_FORMAT,
                    width,
                    height,
                    SAMPLES,
                    MTLStorageMode::Private,
                )?,
                resolved: texture(
                    device,
                    COLOR_FORMAT,
                    width,
                    height,
                    1,
                    MTLStorageMode::Private,
                )?,
                fresh: true,
            })
        };
        Some(Targets {
            width,
            height,
            color: [buffer()?, buffer()?],
            depth: texture(
                device,
                DEPTH_FORMAT,
                width,
                height,
                SAMPLES,
                MTLStorageMode::Private,
            )?,
            depth_fresh: true,
        })
    }
}

impl Pipelines {
    fn new(device: &Device) -> Result<Self, RendererError> {
        let source = NSString::from_str(include_str!("shaders.metal"));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .map_err(|e| RendererError::ShaderCompile(e.to_string()))?;
        let func = |name: &NSString| {
            library
                .newFunctionWithName(name)
                .ok_or(RendererError::Create("shader function"))
        };

        let pipeline = |vertex: &NSString, fragment: &NSString, blend: bool, color_writes: bool| {
            let desc = MTLRenderPipelineDescriptor::new();
            let vertex = func(vertex)?;
            desc.setVertexFunction(Some(&vertex));
            let fragment = func(fragment)?;
            desc.setFragmentFunction(Some(&fragment));
            desc.setRasterSampleCount(SAMPLES);
            desc.setDepthAttachmentPixelFormat(DEPTH_FORMAT);
            // SAFETY: attachment 0 always exists (Metal supports at least eight).
            let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            att.setPixelFormat(COLOR_FORMAT);
            if !color_writes {
                att.setWriteMask(objc2_metal::MTLColorWriteMask::empty());
            }
            if blend {
                // blendfunction(BF_SA, BF_MSA)
                att.setBlendingEnabled(true);
                att.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                att.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                att.setSourceAlphaBlendFactor(MTLBlendFactor::SourceAlpha);
                att.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            }
            device
                .newRenderPipelineStateWithDescriptor_error(&desc)
                .map_err(|e| RendererError::Pipeline(e.to_string()))
        };
        let depth = |compare: MTLCompareFunction, write: bool| {
            let desc = MTLDepthStencilDescriptor::new();
            desc.setDepthCompareFunction(compare);
            desc.setDepthWriteEnabled(write);
            device
                .newDepthStencilStateWithDescriptor(&desc)
                .ok_or(RendererError::Create("depth state"))
        };
        let (tri, line, color) = (
            ns_string!("tri_vertex"),
            ns_string!("line_vertex"),
            ns_string!("color_fragment"),
        );
        let (fill_v, fill_f, zclear_f) = (
            ns_string!("fill_vertex"),
            ns_string!("fill_fragment"),
            ns_string!("zclear_fragment"),
        );
        Ok(Pipelines {
            tri: [
                pipeline(tri, color, false, true)?,
                pipeline(tri, color, true, true)?,
            ],
            line: [
                pipeline(line, color, false, true)?,
                pipeline(line, color, true, true)?,
            ],
            fill: pipeline(fill_v, fill_f, false, true)?,
            zclear: pipeline(fill_v, zclear_f, false, false)?,
            depth_test: depth(MTLCompareFunction::LessEqual, true)?,
            depth_off: depth(MTLCompareFunction::Always, false)?,
            depth_overwrite: depth(MTLCompareFunction::Always, true)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use ep_core::{Options, Sim, ids};

    use super::*;

    /// A renderer that missed frames (while another style drew) draws the
    /// same as one that saw them all once both pick the show up again:
    /// nothing of what either drew before carries over.
    #[test]
    fn resync_follows_ep_and_forgets_the_missed_frames() -> Result<(), RendererError> {
        let (w, h) = (64, 48);
        let mut sim = Sim::new(Options::new(3));
        let (mut seen, mut missed) = (Renderer::system_default()?, Renderer::system_default()?);
        for tick in 0..40 {
            // Smear switches the draw buffer to both, which `missed` never sees.
            if tick == 20 {
                sim.press(ids::SMEAR);
            }
            let frame = sim.tick();
            seen.draw(w, h, std::slice::from_ref(&frame));
            if tick < 10 {
                missed.draw(w, h, std::slice::from_ref(&frame));
            }
        }
        let (draw_buffer, projection) = sim.draw_state();
        assert_eq!(draw_buffer, DrawBuffer::Both);
        seen.resync(draw_buffer, projection);
        missed.resync(draw_buffer, projection);
        let frames: Vec<_> = (0..10).map(|_| sim.tick()).collect();
        let picture = missed.render_offscreen(w, h, &frames)?;
        assert_eq!(seen.render_offscreen(w, h, &frames)?, picture);
        // Smear draws into both buffers, so the one behind holds the same
        // picture; drawing into the back one alone would leave them apart.
        let swap = Frame {
            ops: vec![Op::Swap],
            ..Frame::default()
        };
        assert_eq!(missed.render_offscreen(w, h, &[swap])?, picture);
        Ok(())
    }
}

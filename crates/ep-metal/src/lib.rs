//! Metal renderer for electropaint [`Frame`]s.
//!
//! It emulates the parts of an SGI double-buffered RGB visual that ep relies
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

pub mod show;

use std::ffi::c_void;
use std::ptr::NonNull;

use ep_core::frame::{DrawBuffer, DrawState, Frame, Op, Projection, Vertex};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSString, ns_string};
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder,
    MTLCommandQueue, MTLCompareFunction, MTLDepthStencilDescriptor, MTLDepthStencilState,
    MTLDevice, MTLDrawable, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

/// Height of the original display; line widths and stipple pixels are
/// expressed in its pixels.
const ORIGINAL_HEIGHT: f32 = 1024.0;
const SAMPLES: usize = 4;
const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;

type Device = ProtocolObject<dyn MTLDevice>;
type Texture = ProtocolObject<dyn MTLTexture>;
type Pipeline = ProtocolObject<dyn MTLRenderPipelineState>;
type DepthState = ProtocolObject<dyn MTLDepthStencilState>;
type Encoder = ProtocolObject<dyn MTLRenderCommandEncoder>;

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

struct Pipelines {
    tri: [Retained<Pipeline>; 2], // [opaque, blended]
    line: [Retained<Pipeline>; 2],
    fill: Retained<Pipeline>,
    zclear: Retained<Pipeline>,
    depth_test: Retained<DepthState>,
    depth_off: Retained<DepthState>,
    depth_overwrite: Retained<DepthState>,
}

/// Renders frames into one `CAMetalLayer`. One per window/display.
pub struct Renderer {
    device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: Pipelines,
    targets: Option<Targets>,
    /// Index of the buffer currently displayed.
    front: usize,
    draw_buffer: DrawBuffer,
    projection: Projection,
}

impl Renderer {
    pub fn new(device: Retained<Device>) -> Self {
        let queue = device.newCommandQueue().expect("command queue");
        let pipelines = Pipelines::new(&device);
        Renderer {
            device,
            queue,
            pipelines,
            targets: None,
            front: 0,
            draw_buffer: DrawBuffer::Back,
            projection: Projection {
                fovy_tenths: 300,
                aspect: 1.25,
                near: 0.01,
                far: 1e6,
            },
        }
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Configures a layer for this renderer: our device, BGRA8, and a
    /// drawable we can blit into.
    pub fn configure_layer(&self, layer: &CAMetalLayer) {
        layer.setDevice(Some(&self.device));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(false);
    }

    /// Executes the frames' operations in order, then shows the front buffer
    /// on `layer`. With no frames it just re-presents the current picture,
    /// which is needed after a resize.
    pub fn render(&mut self, layer: &CAMetalLayer, frames: &[Frame]) {
        let size = layer.drawableSize();
        let Some(cb) = self.encode(size.width as usize, size.height as usize, frames) else {
            return;
        };
        self.present(&cb, layer, self.front);
        cb.commit();
    }

    /// Shows a picture that is not part of ep's own sequence (an in-between
    /// from [`ep_core::Sim::frame_between`]): it is drawn into the back
    /// buffer and presented from there, without a swap, so the next real
    /// frame (which clears the back buffer first) finds the buffers as ep
    /// left them.
    pub fn render_between(&mut self, layer: &CAMetalLayer, frame: &Frame) {
        let size = layer.drawableSize();
        let saved = std::mem::replace(&mut self.draw_buffer, DrawBuffer::Back);
        let cb = self.encode(
            size.width as usize,
            size.height as usize,
            std::slice::from_ref(frame),
        );
        self.draw_buffer = saved;
        let Some(cb) = cb else { return };
        self.present(&cb, layer, self.front ^ 1);
        cb.commit();
    }

    /// Executes frames into a `width` x `height` target without a window and
    /// returns the displayed (front) buffer as BGRA8 rows, top row first.
    pub fn render_offscreen(&mut self, width: usize, height: usize, frames: &[Frame]) -> Vec<u8> {
        let cb = self.encode(width, height, frames).expect("encode");
        let row = width * 4;
        let buf = self
            .device
            .newBufferWithLength_options(row * height, MTLResourceOptions::StorageModeShared)
            .expect("readback buffer");
        let blit = cb.blitCommandEncoder().expect("blit encoder");
        let src = &self.targets.as_ref().unwrap().color[self.front].resolved;
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                src,
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
        unsafe { std::slice::from_raw_parts(ptr, row * height) }.to_vec()
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
            self.targets = Some(Targets::new(&self.device, w, h));
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
        &self,
        frames: &[Frame],
    ) -> (Option<Retained<ProtocolObject<dyn MTLBuffer>>>, Vec<usize>) {
        let mut all: Vec<Vertex> = Vec::new();
        let mut offsets = Vec::new();
        for op in frames.iter().flat_map(|f| &f.ops) {
            offsets.push(all.len() * size_of::<Vertex>());
            if let Op::Triangles { verts, .. } | Op::Lines { verts, .. } = op {
                all.extend_from_slice(verts);
            }
        }
        if all.is_empty() {
            return (None, offsets);
        }
        let bytes = all.len() * size_of::<Vertex>();
        let ptr = NonNull::new(all.as_ptr() as *mut c_void).unwrap();
        let buf = unsafe {
            self.device.newBufferWithBytes_length_options(
                ptr,
                bytes,
                MTLResourceOptions::StorageModeShared,
            )
        };
        (buf, offsets)
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
        let targets = self.targets.as_mut().unwrap();
        let (w, h) = (targets.width as f32, targets.height as f32);
        let pass = MTLRenderPassDescriptor::new();
        let color = &mut targets.color[index];
        unsafe {
            let att = pass.colorAttachments().objectAtIndexedSubscript(0);
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
        }
        color.fresh = false;
        targets.depth_fresh = false;
        let Some(enc) = cb.renderCommandEncoderWithDescriptor(&pass) else {
            return;
        };

        let mut u = Uniforms {
            proj: [[0.0; 4]; 4],
            viewport: [w, h],
            px_scale: h / ORIGINAL_HEIGHT,
            line_width: 1.0,
            fill_color: [0.0; 4],
            pattern: [0; 16],
            use_pattern: 0,
            _pad: [0; 3],
        };
        for &(offset, op) in ops {
            u.proj = projection_matrix(&self.projection, w / h);
            match op {
                Op::SetProjection(p) => self.projection = *p,
                Op::Clear(c) => {
                    u.fill_color = *c;
                    u.use_pattern = 0;
                    self.draw_fill(&enc, &self.pipelines.fill, &u);
                }
                Op::StippleFill { pattern, color } => {
                    u.fill_color = *color;
                    u.pattern = pattern.map(u32::from);
                    u.use_pattern = 1;
                    self.draw_fill(&enc, &self.pipelines.fill, &u);
                }
                Op::ZClear => {
                    enc.setDepthStencilState(Some(&self.pipelines.depth_overwrite));
                    self.draw_fill(&enc, &self.pipelines.zclear, &u);
                }
                Op::Triangles { state, verts } => {
                    let Some(vbuf) = vbuf else { continue };
                    self.bind(&enc, &self.pipelines.tri, state, vbuf, offset, &u);
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
                    self.bind(&enc, &self.pipelines.line, state, vbuf, offset, &u);
                    unsafe {
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                            verts.len() / 2,
                        )
                    };
                }
                Op::SetDrawBuffer(_) | Op::Swap => unreachable!("handled by render"),
            }
        }
        enc.endEncoding();
    }

    fn bind(
        &self,
        enc: &Encoder,
        pipelines: &[Retained<Pipeline>; 2],
        state: &DrawState,
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
        unsafe {
            enc.setVertexBuffer_offset_atIndex(Some(vbuf), offset, 0);
            set_uniforms(enc, u);
        }
    }

    fn draw_fill(&self, enc: &Encoder, pipeline: &Pipeline, u: &Uniforms) {
        enc.setRenderPipelineState(pipeline);
        if !std::ptr::eq(pipeline, &*self.pipelines.zclear) {
            enc.setDepthStencilState(Some(&self.pipelines.depth_off));
        }
        unsafe {
            set_uniforms(enc, u);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
    }

    /// Blits colour buffer `index` to the layer's next drawable and presents it.
    fn present(
        &self,
        cb: &ProtocolObject<dyn MTLCommandBuffer>,
        layer: &CAMetalLayer,
        index: usize,
    ) {
        let Some(drawable) = layer.nextDrawable() else {
            return;
        };
        let Some(blit) = cb.blitCommandEncoder() else {
            return;
        };
        let src = &self.targets.as_ref().unwrap().color[index].resolved;
        unsafe { blit.copyFromTexture_toTexture(src, &drawable.texture()) };
        blit.endEncoding();
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
        cb.presentDrawable(drawable);
    }
}

unsafe fn set_uniforms(enc: &Encoder, u: &Uniforms) {
    let ptr = NonNull::from(u).cast::<c_void>();
    unsafe {
        enc.setVertexBytes_length_atIndex(ptr, size_of::<Uniforms>(), 1);
        enc.setFragmentBytes_length_atIndex(ptr, size_of::<Uniforms>(), 1);
    }
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
    fn new(device: &Device, width: usize, height: usize) -> Self {
        let buffer = || ColorBuffer {
            msaa: texture(device, COLOR_FORMAT, width, height, SAMPLES),
            resolved: texture(device, COLOR_FORMAT, width, height, 1),
            fresh: true,
        };
        Targets {
            width,
            height,
            color: [buffer(), buffer()],
            depth: texture(device, DEPTH_FORMAT, width, height, SAMPLES),
            depth_fresh: true,
        }
    }
}

fn texture(
    device: &Device,
    format: MTLPixelFormat,
    width: usize,
    height: usize,
    samples: usize,
) -> Retained<Texture> {
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, width, height, false,
        )
    };
    unsafe {
        if samples > 1 {
            desc.setTextureType(MTLTextureType::Type2DMultisample);
            desc.setSampleCount(samples);
        }
        desc.setStorageMode(MTLStorageMode::Private);
    }
    desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    device.newTextureWithDescriptor(&desc).expect("texture")
}

impl Pipelines {
    fn new(device: &Device) -> Self {
        let source = NSString::from_str(include_str!("shaders.metal"));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .unwrap_or_else(|e| panic!("shader compile failed: {e}"));
        let func = |name: &NSString| library.newFunctionWithName(name).expect("shader function");

        let pipeline = |vertex: &NSString, fragment: &NSString, blend: bool, color_writes: bool| {
            let desc = MTLRenderPipelineDescriptor::new();
            desc.setVertexFunction(Some(&func(vertex)));
            desc.setFragmentFunction(Some(&func(fragment)));
            desc.setRasterSampleCount(SAMPLES);
            desc.setDepthAttachmentPixelFormat(DEPTH_FORMAT);
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
                .unwrap_or_else(|e| panic!("pipeline: {e}"))
        };
        let depth = |compare: MTLCompareFunction, write: bool| {
            let desc = MTLDepthStencilDescriptor::new();
            desc.setDepthCompareFunction(compare);
            desc.setDepthWriteEnabled(write);
            device
                .newDepthStencilStateWithDescriptor(&desc)
                .expect("depth state")
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
        Pipelines {
            tri: [
                pipeline(tri, color, false, true),
                pipeline(tri, color, true, true),
            ],
            line: [
                pipeline(line, color, false, true),
                pipeline(line, color, true, true),
            ],
            fill: pipeline(fill_v, fill_f, false, true),
            zclear: pipeline(fill_v, zclear_f, false, false),
            depth_test: depth(MTLCompareFunction::LessEqual, true),
            depth_off: depth(MTLCompareFunction::Always, false),
            depth_overwrite: depth(MTLCompareFunction::Always, true),
        }
    }
}

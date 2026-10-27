//! Small Metal helpers both renderers use: textures, buffers, constants
//! and reading a picture back.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLOrigin,
    MTLPixelFormat, MTLRenderCommandEncoder, MTLResourceOptions, MTLSize, MTLStorageMode,
    MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};

use crate::{Buffer, CommandBuffer, Device, Encoder, MAX_SIDE, RendererError, Texture};

/// Whether Metal can make a `width` x `height` picture.
pub(crate) fn fits(width: usize, height: usize) -> bool {
    (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height)
}

/// An error for an offscreen picture Metal can't make at that size.
pub(crate) fn check_size(width: usize, height: usize) -> Result<(), RendererError> {
    if fits(width, height) {
        Ok(())
    } else {
        Err(RendererError::Size { width, height })
    }
}

/// A 2D render target that shaders can also read (unless memoryless, which
/// lives only in tile memory); `samples` > 1 for a multisampled one.
pub(crate) fn texture(
    device: &Device,
    format: MTLPixelFormat,
    width: usize,
    height: usize,
    samples: usize,
    storage: MTLStorageMode,
) -> Option<Retained<Texture>> {
    // SAFETY: a plain descriptor; callers rule out zero sizes (`fits`).
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

/// Sets `value` as vertex buffer `vertex` and fragment buffer `fragment`.
pub(crate) fn set_bytes<T: Copy>(enc: &Encoder, value: &T, vertex: usize, fragment: usize) {
    let ptr = NonNull::from(value).cast::<c_void>();
    // SAFETY: `ptr` points to a live `T` of exactly the length given, which
    // Metal copies; its `repr(C)` layout matches the shaders'.
    unsafe {
        enc.setVertexBytes_length_atIndex(ptr, size_of::<T>(), vertex);
        enc.setFragmentBytes_length_atIndex(ptr, size_of::<T>(), fragment);
    }
}

/// A shared buffer holding a copy of `items`; `None` if there are none.
pub(crate) fn upload<T: Copy>(device: &Device, items: &[T]) -> Option<Retained<Buffer>> {
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
pub(crate) fn read_back(
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

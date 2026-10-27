//! The Metal side of a view that hosts a show: a `CAMetalLayer` and the
//! renderer that draws into it. The view adopts the layer and drives
//! [`Surface::render`] from its display link.

use objc2::rc::Retained;
use objc2_foundation::NSSize;
use objc2_metal::MTLCreateSystemDefaultDevice;
use objc2_quartz_core::CAMetalLayer;

use crate::show::Show;
use crate::{Renderer, RendererError};

pub struct Surface {
    renderer: Renderer,
    layer: Retained<CAMetalLayer>,
}

impl Surface {
    /// A layer set up for a renderer on the system's default Metal device.
    pub fn new() -> Result<Self, RendererError> {
        let device = MTLCreateSystemDefaultDevice().ok_or(RendererError::NoDevice)?;
        let renderer = Renderer::try_new(device)?;
        let layer = CAMetalLayer::new();
        renderer.configure_layer(&layer);
        Ok(Surface { renderer, layer })
    }

    /// The layer, for the hosting view to adopt.
    pub fn layer(&self) -> &CAMetalLayer {
        &self.layer
    }

    /// Keeps the drawable at `points` (the view's size) times `scale`, in
    /// device pixels.
    pub fn fit(&self, points: NSSize, scale: f64) {
        let size = NSSize::new(points.width * scale, points.height * scale);
        if self.layer.drawableSize() != size {
            self.layer.setContentsScale(scale);
            self.layer.setDrawableSize(size);
        }
    }

    /// Advances `show` to `time` seconds and renders what it has.
    pub fn render(&mut self, show: &mut Show, time: f64) {
        show.render(&mut self.renderer, &self.layer, time);
    }
}

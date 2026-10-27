//! The Metal side of a view that hosts shows: a `CAMetalLayer` and the
//! renderers that draw into it. The view adopts the layer and drives
//! [`Surface::render`] from its display link.
//!
//! A wide view can run several shows side by side, one *strand* each: every
//! show gets a renderer and an equal-width region of the drawable, so each
//! region is a picture of its own, as if on its own screen.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSSize;
use objc2_metal::{MTLCommandBuffer, MTLCommandEncoder, MTLDrawable};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use crate::show::{Picture, Show};
use crate::{Renderer, RendererError};

/// The aspect ratio of ep's own screen (1280x1024): `auto_strands` fits
/// strands of about this shape across a view.
const EP_ASPECT: f64 = 1.25;

/// How many strands fit across a `width` x `height` view, each about the
/// shape of ep's screen: one on any ordinary display, three on 32:9.
pub fn auto_strands(width: f64, height: f64) -> usize {
    if width <= 0.0 || height <= 0.0 {
        return 1;
    }
    ((width / height / EP_ASPECT).round() as usize).max(1)
}

/// The left edge and width of strand `index` of `count` across `width`
/// pixels. The regions tile the width exactly.
pub fn region(width: usize, count: usize, index: usize) -> (usize, usize) {
    let edge = |i: usize| width * i / count;
    (edge(index), edge(index + 1) - edge(index))
}

pub struct Surface {
    /// One per strand; the first made the others (`Renderer::sibling`).
    renderers: Vec<Renderer>,
    layer: Retained<CAMetalLayer>,
}

impl Surface {
    /// A layer set up for a renderer on the system's default Metal device.
    pub fn new() -> Result<Self, RendererError> {
        let renderer = Renderer::system_default()?;
        let layer = CAMetalLayer::new();
        renderer.configure_layer(&layer);
        Ok(Surface {
            renderers: vec![renderer],
            layer,
        })
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

    /// Advances each show to `time` seconds, draws what it has into its
    /// strand, and presents the strands side by side, in order.
    pub fn render<'a>(&mut self, shows: impl IntoIterator<Item = &'a mut Show>, time: f64) {
        let size = self.layer.drawableSize();
        let (width, height) = (size.width as usize, size.height as usize);
        let shows: Vec<&mut Show> = shows.into_iter().collect();
        let count = shows.len().max(1);
        while self.renderers.len() < count {
            let sibling = self.renderers[0].sibling();
            self.renderers.push(sibling);
        }
        let mut changed = false;
        for (index, (show, renderer)) in shows.into_iter().zip(&mut self.renderers).enumerate() {
            let (_, w) = region(width, count, index);
            match show.picture(time) {
                Picture::Ticks(frames) => renderer.draw(w, height, &frames),
                Picture::Between(frame) => renderer.draw_between(w, height, &frame),
                Picture::Same => continue,
            }
            changed = true;
        }
        // With nothing new the drawable on screen still shows it all. After a
        // resize, wait until every strand has drawn at its new size.
        let fits = (0..count)
            .all(|i| self.renderers[i].size() == Some((region(width, count, i).1, height)));
        if changed && fits {
            self.present(width, count);
        }
    }

    /// Copies every strand into its region of the next drawable and presents it.
    fn present(&self, width: usize, count: usize) {
        let Some(drawable) = self.layer.nextDrawable() else {
            return;
        };
        let Some(cb) = self.renderers[0].command_buffer() else {
            return;
        };
        let Some(blit) = cb.blitCommandEncoder() else {
            return;
        };
        let texture = drawable.texture();
        for (index, renderer) in self.renderers.iter().take(count).enumerate() {
            renderer.blit_into(&blit, &texture, region(width, count, index).0);
        }
        blit.endEncoding();
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
        cb.presentDrawable(drawable);
        cb.commit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strands_fit_the_screen_shape() {
        for ((w, h), n) in [
            ((3024.0, 1964.0), 1), // MacBook Pro 14"
            ((3840.0, 2160.0), 1), // 16:9
            ((1280.0, 1024.0), 1),
            ((3440.0, 1440.0), 2), // 21:9
            ((5120.0, 1440.0), 3), // 32:9
            ((7680.0, 2160.0), 3),
            ((1080.0, 1920.0), 1), // portrait
            ((0.0, 0.0), 1),
        ] {
            assert_eq!(auto_strands(w, h), n, "{w}x{h}");
        }
    }

    #[test]
    fn regions_tile_the_width() {
        assert_eq!(region(7680, 3, 0), (0, 2560));
        assert_eq!(region(7680, 3, 2), (5120, 2560));
        let widths: Vec<_> = (0..3).map(|i| region(100, 3, i)).collect();
        assert_eq!(widths, [(0, 33), (33, 33), (66, 34)]);
        assert_eq!(region(640, 1, 0), (0, 640));
    }
}

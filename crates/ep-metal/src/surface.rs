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

use crate::remastered::Remastered;
use crate::show::{Picture, Show};
use crate::{Renderer, RendererError};

/// How the shows are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    /// As ep drew them on an SGI RGB visual.
    #[default]
    Classic,
    /// Iridescent chrome in high dynamic range ([`Remastered`]).
    Remastered,
}

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
    style: Style,
    /// For `Style::Classic`, one per strand; the first made the others
    /// (`Renderer::sibling`). Empty until that style is first used.
    renderers: Vec<Renderer>,
    /// For `Style::Remastered`, once that style is first used.
    remastered: Option<Remastered>,
    layer: Retained<CAMetalLayer>,
    /// How far above SDR white the display can show (1 without EDR).
    headroom: f64,
    /// The classic renderers missed frames while another style drew, and
    /// must pick the shows up again.
    resync: bool,
}

impl Surface {
    /// A layer set up for a renderer in `style` on the system's default
    /// Metal device.
    pub fn new(style: Style) -> Result<Self, RendererError> {
        let mut surface = Surface {
            style,
            renderers: Vec::new(),
            remastered: None,
            layer: CAMetalLayer::new(),
            headroom: 1.0,
            resync: false,
        };
        surface.prepare(style)?;
        Ok(surface)
    }

    /// Draws in `style` from now on. The shows carry on as they were.
    ///
    /// # Errors
    ///
    /// If Metal can't set up that style's renderer; the style is unchanged.
    pub fn set_style(&mut self, style: Style) -> Result<(), RendererError> {
        if style != self.style {
            self.prepare(style)?;
            // The style left behind frees its buffers.
            match self.style {
                Style::Classic => self.renderers.iter_mut().for_each(Renderer::release),
                Style::Remastered => self.remastered.iter_mut().for_each(Remastered::release),
            }
            self.resync = style == Style::Classic;
            self.style = style;
        }
        Ok(())
    }

    /// Makes `style`'s renderer if there isn't one and sets the layer up for it.
    fn prepare(&mut self, style: Style) -> Result<(), RendererError> {
        match style {
            Style::Classic => {
                if self.renderers.is_empty() {
                    self.renderers.push(Renderer::system_default()?);
                }
                self.layer.setWantsExtendedDynamicRangeContent(false);
                self.layer.setColorspace(None);
                self.renderers[0].configure_layer(&self.layer);
            }
            Style::Remastered => {
                if self.remastered.is_none() {
                    self.remastered = Some(Remastered::system_default()?);
                }
                if let Some(r) = &self.remastered {
                    r.configure_layer(&self.layer);
                }
            }
        }
        Ok(())
    }

    /// How far above SDR white the display can currently show, as AppKit's
    /// `maximumExtendedDynamicRangeColorComponentValue` reports it.
    pub fn set_headroom(&mut self, headroom: f64) {
        self.headroom = headroom;
    }

    /// The layer, for the hosting view to adopt.
    pub fn layer(&self) -> &CAMetalLayer {
        &self.layer
    }

    /// Keeps the drawable at `points` (the view's size) times `scale`, in
    /// device pixels.
    pub fn fit(&self, points: NSSize, scale: f64) {
        let size = NSSize::new(
            (points.width * scale).round(),
            (points.height * scale).round(),
        );
        if self.layer.drawableSize() != size {
            self.layer.setContentsScale(scale);
            self.layer.setDrawableSize(size);
        }
    }

    /// Advances each show to `time` seconds, draws what it has into its
    /// strand, and presents the strands side by side, in order.
    pub fn render<'a>(&mut self, shows: impl IntoIterator<Item = &'a mut Show>, time: f64) {
        if let (Style::Remastered, Some(r)) = (self.style, &mut self.remastered) {
            r.render(
                &self.layer,
                shows.into_iter().collect(),
                time,
                self.headroom,
            );
            return;
        }
        let size = self.layer.drawableSize();
        let (width, height) = (size.width as usize, size.height as usize);
        // No more strands than pixels across (until the drawable has a size).
        let most = if width == 0 { usize::MAX } else { width };
        let shows: Vec<&mut Show> = shows.into_iter().take(most).collect();
        let count = shows.len().max(1);
        while self.renderers.len() < count {
            let sibling = self.renderers[0].sibling();
            self.renderers.push(sibling);
        }
        if std::mem::take(&mut self.resync) {
            for (show, renderer) in shows.iter().zip(&mut self.renderers) {
                let (draw_buffer, projection) = show.sim().draw_state();
                renderer.resync(draw_buffer, projection);
            }
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

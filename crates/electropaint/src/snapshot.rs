//! `--snapshot` and `--panel-snapshot`: pictures drawn offscreen, then exit.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ep_metal::Renderer;
use ep_metal::remastered::Remastered;
use ep_metal::show::Show;
use ep_metal::surface::{Style, region};
use objc2::MainThreadMarker;

use crate::args::{Options, Snapshot};
use crate::panel;
use crate::view::NORMAL_LEVEL;

/// Runs the shows offscreen for `snap.frames` retraces and saves what is
/// displayed, the strands side by side.
pub(crate) fn write_snapshot(
    snap: &Snapshot,
    opts: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    // No more strands than pixels across.
    let count = opts
        .strands()
        .count(snap.width as f64, snap.height as f64)
        .min(snap.width);
    let base = opts.seed_base();
    let frames = |index: usize| {
        let mut show = opts.show_options(base.wrapping_add(index as i64));
        // Only remastered draws from scenes; a snapshot can't switch style.
        show.scene = opts.style == Style::Remastered;
        snapshot_frames(snap, show)
    };
    let mut rgb = vec![0; snap.width * snap.height * 3];
    let mut paste =
        |bgra: &[u8], x: usize, width: usize| paste(&mut rgb, snap.width, bgra, x, width);
    match opts.style {
        // Each strand is a picture of its own.
        Style::Classic => {
            let first = Renderer::system_default()?;
            for index in 0..count {
                let (x, width) = region(snap.width, count, index);
                let mut renderer = first.sibling();
                paste(
                    &renderer.render_offscreen(width, snap.height, &frames(index))?,
                    x,
                    width,
                );
            }
        }
        // The strands share one space.
        Style::Remastered => {
            let strands: Vec<_> = (0..count).map(frames).collect();
            let mut renderer = Remastered::system_default()?;
            paste(
                &renderer.render_offscreen(snap.width, snap.height, &strands)?,
                0,
                snap.width,
            );
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(&snap.path)?);
    let mut encoder = png::Encoder::new(file, snap.width as u32, snap.height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

/// Copies `bgra` (BGRA8 rows `width` pixels wide) into `rgb` (RGB8 rows
/// `rgb_width` wide) with its left edge at `x`.
fn paste(rgb: &mut [u8], rgb_width: usize, bgra: &[u8], x: usize, width: usize) {
    for (y, row) in bgra.chunks_exact(width * 4).enumerate() {
        let out = &mut rgb[(y * rgb_width + x) * 3..][..width * 3];
        for (o, p) in out
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(row.as_chunks::<4>().0)
        {
            *o = [p[2], p[1], p[0]];
        }
    }
}

/// A snapshot's show: its frames up to `snap.frames` retraces.
fn snapshot_frames(snap: &Snapshot, opts: ep_core::Options) -> Vec<ep_core::Frame> {
    let mut sim = ep_core::Sim::new(opts);
    let (mut frames, mut retraces) = (Vec::new(), 0);
    while retraces < snap.frames && !sim.exited() {
        let frame = sim.tick();
        retraces += u64::from(frame.retraces());
        frames.push(frame);
    }
    frames
}

/// `--panel-snapshot`: draws the control panel as it is after five seconds
/// of the show, then exits.
pub(crate) fn write_panel_snapshot(mtm: MainThreadMarker, path: &str, opts: &Options) {
    let show = Rc::new(RefCell::new(Show::new(opts.show_options(opts.seed_base()))));
    for i in 0..300 {
        let _ = show.borrow_mut().picture(f64::from(i) / 60.0);
    }
    let style = Rc::new(Cell::new(opts.style));
    let panel = panel::PanelController::new(mtm, show, style, false, NORMAL_LEVEL);
    if let Err(e) = panel.snapshot(path) {
        eprintln!("electropaint: panel snapshot failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strands_paste_side_by_side_as_rgb() {
        // Two 2x2 strands (BGRA: blue and red) into a 4x2 RGB picture.
        let (blue, red) = ([255, 0, 0, 255].repeat(4), [0, 0, 255, 255].repeat(4));
        let mut rgb = vec![0; 4 * 2 * 3];
        paste(&mut rgb, 4, &blue, 0, 2);
        paste(&mut rgb, 4, &red, 2, 2);
        let row = [[0, 0, 255], [0, 0, 255], [255, 0, 0], [255, 0, 0]].concat();
        assert_eq!(rgb, [row.clone(), row].concat());
    }
}

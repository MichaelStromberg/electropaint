//! Renders small synthetic frames offscreen and checks pixels.

use ep_core::frame::{DrawBuffer, DrawState, Frame, Op, Projection, Vertex};
use ep_metal::Renderer;
use objc2_metal::MTLCreateSystemDefaultDevice;

const W: usize = 128;
const H: usize = 128;

fn renderer() -> Renderer {
    Renderer::new(MTLCreateSystemDefaultDevice().expect("Metal device"))
}

/// RGBA at (x, y), y counted from the top.
fn px(img: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * W + x) * 4;
    [img[i + 2], img[i + 1], img[i], img[i + 3]]
}

fn v(x: f32, y: f32, color: [f32; 4]) -> Vertex {
    Vertex { pos: [x, y, -10.0], color }
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Square viewport, 30 degree fovy, at z = -10: the visible half-height is
/// 10 * tan(15 deg) = 2.68.
fn projection() -> Op {
    Op::SetProjection(Projection { fovy_tenths: 300, aspect: 1.0, near: 0.01, far: 1e6 })
}

#[test]
fn clear_triangle_and_swap() {
    let mut r = renderer();
    let tri = Op::Triangles {
        state: DrawState::default(),
        verts: vec![v(-1.0, -1.0, GREEN), v(1.0, -1.0, GREEN), v(0.0, 1.0, GREEN)],
    };
    let frame = Frame { ops: vec![projection(), Op::Clear(RED), tri, Op::Swap] };
    let img = r.render_offscreen(W, H, &[frame]);
    assert_eq!(px(&img, 2, 2), [255, 0, 0, 255], "corner is the clear colour");
    assert_eq!(px(&img, W / 2, H / 2 + 10), [0, 255, 0, 255], "centre is the triangle");

    // Nothing drawn into the new back buffer: swapping again shows its old
    // contents (the never-written buffer is black).
    let img = r.render_offscreen(W, H, &[Frame { ops: vec![Op::Swap] }]);
    assert_eq!(px(&img, 2, 2), [0, 0, 0, 255]);
}

#[test]
fn blending_and_front_buffer() {
    let mut r = renderer();
    let half_green = [0.0, 1.0, 0.0, 0.5];
    let quad = |c| Op::Triangles {
        state: DrawState { depth_test: false, blend: true },
        verts: vec![
            v(-3.0, -3.0, c),
            v(3.0, -3.0, c),
            v(3.0, 3.0, c),
            v(-3.0, -3.0, c),
            v(3.0, 3.0, c),
            v(-3.0, 3.0, c),
        ],
    };
    let frames = [
        Frame { ops: vec![projection(), Op::Clear(RED), Op::Swap] },
        // Draw into the visible buffer directly, as smear mode does.
        Frame { ops: vec![Op::SetDrawBuffer(DrawBuffer::Front), quad(half_green)] },
    ];
    let img = r.render_offscreen(W, H, &frames);
    let [red, green, blue, _] = px(&img, W / 2, H / 2);
    assert!((127..=128).contains(&red) && (127..=128).contains(&green) && blue == 0, "{red} {green}");
}

#[test]
fn stipple_fill_hits_one_pixel_per_4x4_block() {
    let mut r = renderer();
    // Rows 0, 4, 8, 12 from the bottom, bit 15 (leftmost) of every nibble.
    let mut pattern = [0u16; 16];
    for row in [0, 4, 8, 12] {
        pattern[row] = 0x8888;
    }
    // A 1024-pixel-high target has a pixel scale of 1, so pattern bits map
    // to device pixels one to one.
    let (w, h) = (64, 1024);
    let frame = Frame {
        ops: vec![
            Op::Clear(BLACK),
            Op::StippleFill { pattern, color: [1.0, 1.0, 1.0, 1.0] },
            Op::Swap,
        ],
    };
    let img = r.render_offscreen(w, h, &[frame]);
    let at = |x: usize, y_from_bottom: usize| {
        let i = ((h - 1 - y_from_bottom) * w + x) * 4;
        img[i]
    };
    assert_eq!(at(0, 0), 255);
    assert_eq!(at(4, 4), 255);
    assert_eq!(at(1, 0), 0);
    assert_eq!(at(0, 1), 0);
    let lit = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| at(x, y) == 255).count();
    assert_eq!(lit, w * h / 16);
}

#[test]
fn wide_lines_cover_minor_axis() {
    let mut r = renderer();
    let (w, h) = (256, 1024); // pixel scale 1: widths are device pixels
    let line = Op::Lines {
        state: DrawState::default(),
        width: 3.0,
        verts: vec![v(-2.0, 0.0, GREEN), v(2.0, 0.0, GREEN)],
    };
    let frame = Frame {
        ops: vec![
            Op::SetProjection(Projection { fovy_tenths: 300, aspect: 0.25, near: 0.01, far: 1e6 }),
            Op::Clear(BLACK),
            line,
            Op::Swap,
        ],
    };
    let img = r.render_offscreen(w, h, &[frame]);
    // Multisampling softens the edges, so measure total coverage in pixels.
    let coverage: f32 = (0..h).map(|y| img[(y * w + w / 2) * 4 + 1] as f32 / 255.0).sum();
    assert!((coverage - 3.0).abs() < 0.05, "a width-3 horizontal line covers 3 pixels, got {coverage}");
}

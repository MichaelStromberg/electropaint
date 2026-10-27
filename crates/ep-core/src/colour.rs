//! ep's own colour conversion, with the original's single/double precision
//! mix (rendering.md §7). The arithmetic is spelled out step by step because
//! the trace comparison is bit-exact: every `as f32` marks a store to a
//! `float` in the original, every `f64` an intermediate kept in a double
//! register (K&R promotion of float arguments).

/// `hls_to_rgb(h, l, s)`: hue in turns (0 red, 1/3 green, 2/3 blue),
/// lightness and saturation in 0..1. The arguments are doubles in the
/// original; lightness and saturation are narrowed to `float` at once.
pub fn hls_to_rgb(h: f64, l: f64, s: f64) -> [f32; 3] {
    let lf = l as f32;
    let sf = s as f32;
    let m2 = if f64::from(lf) <= 0.5 {
        (1.0 + sf) * lf
    } else {
        (lf + sf) - lf * sf
    };
    if sf == 0.0 {
        return [lf, lf, lf];
    }
    let m1 = 2.0 * lf - m2;
    // The hue is scaled to degrees in double precision and stored as float.
    let hd = (f64::from(h as f32) * 360.0) as f32;
    let hd = f64::from(hd);
    [
        value(m1, m2, (hd + 120.0) as f32),
        value(m1, m2, hd as f32),
        value(m1, m2, (hd - 120.0) as f32),
    ]
}

/// One channel of the HLS trapezoid. Only a single ±360 wrap is applied, so
/// hues outside roughly (−1/3, 4/3) turns are not fully reduced (as in ep).
fn value(n1: f32, n2: f32, hue: f32) -> f32 {
    let mut x = hue;
    if 360.0 < x {
        x -= 360.0;
    }
    if x < 0.0 {
        x += 360.0;
    }
    let d = f64::from(n2 - n1);
    if x < 60.0 {
        (f64::from(n1) + d * (f64::from(x) / 60.0)) as f32
    } else if x < 180.0 {
        n2
    } else if x < 240.0 {
        (f64::from(n1) + d * (f64::from(240.0 - x) / 60.0)) as f32
    } else {
        n1
    }
}

/// `rgb_to_irgb`: 0..1 to 0..255 by truncation (the original forces
/// round-toward-zero for every float to int conversion).
pub fn rgb_to_irgb(c: f32) -> i16 {
    (f64::from(c) * 255.0) as i32 as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries() {
        assert_eq!(hls_to_rgb(0.0, 0.5, 1.0), [1.0, 0.0, 0.0]);
        assert_eq!(hls_to_rgb(1.0 / 3.0, 0.5, 1.0)[1], 1.0);
        assert_eq!(hls_to_rgb(2.0 / 3.0, 0.5, 1.0)[2], 1.0);
        assert_eq!(hls_to_rgb(0.3, 0.25, 0.0), [0.25, 0.25, 0.25]);
        assert_eq!(hls_to_rgb(0.0, 0.0, 1.0), [0.0, 0.0, 0.0]);
        assert_eq!(hls_to_rgb(0.5, 1.0, 1.0), [1.0, 1.0, 1.0]);
    }

    /// Values taken from the IRIX traces (`c4f` of a fill and its outline).
    #[test]
    fn trace_values() {
        // hue 0.01, lightness 0.5: the first channel swatch, mapcolor(768, 255, 15, 0).
        let c = hls_to_rgb(f64::from(0.01f32), 0.5, 1.0);
        assert_eq!(c.map(rgb_to_irgb), [255, 15, 0]);
        // The complementary outline of a fill is its per-channel complement.
        let h = 0.6f32;
        let l = 0.4f32;
        let fill = hls_to_rgb(f64::from(h), f64::from(l), 1.0);
        let h2 = (f64::from(h) + 0.5) as f32;
        let h2 = if 1.0 < f64::from(h2) {
            (f64::from(h2) - 1.0) as f32
        } else {
            h2
        };
        let out = hls_to_rgb(f64::from(h2), 1.0 - f64::from(l), 1.0);
        for i in 0..3 {
            assert!((fill[i] + out[i] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn irgb_truncates() {
        assert_eq!(rgb_to_irgb(1.0), 255);
        assert_eq!(rgb_to_irgb(0.999), 254);
        assert_eq!(rgb_to_irgb(0.0), 0);
    }
}

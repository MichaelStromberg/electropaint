//! Matrices and vectors, in IrisGL's conventions (see the parent module).

/// Column-major 4x4 matrix (same memory order as an IrisGL `Matrix`).
pub(crate) type Mat4 = [f32; 16];

pub(crate) const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// `a · b`.
pub(crate) fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

/// The rotation IrisGL's `rotate(a, axis)` applies: `a` tenths of a degree,
/// right-handed (counter-clockwise looking down the axis).
pub(crate) fn rotation(tenths: i16, axis: char) -> Mat4 {
    let rad = f64::from(tenths) / 10.0 * std::f64::consts::PI / 180.0;
    let (s, c) = (rad.sin() as f32, rad.cos() as f32);
    let mut m = IDENTITY;
    let (i, j) = match axis {
        'x' | 'X' => (1, 2),
        'y' | 'Y' => (2, 0),
        _ => (0, 1),
    };
    m[i * 4 + i] = c;
    m[i * 4 + j] = s;
    m[j * 4 + i] = -s;
    m[j * 4 + j] = c;
    m
}

pub(crate) fn translation(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

pub(crate) fn scaling(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

pub(super) fn transform_point(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r])
}

pub(super) fn transform_dir(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2])
}

/// Normal transform: the inverse transpose of the upper 3x3, applied to
/// `n` (up to a positive scale factor; callers normalise).
pub(super) fn transform_normal(m: &Mat4, n: [f32; 3]) -> [f32; 3] {
    let a = |r: usize, c: usize| f64::from(m[c * 4 + r]);
    // Cofactor matrix of the 3x3 = det · inverse transpose.
    let cof = [
        [
            a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1),
            a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2),
            a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0),
        ],
        [
            a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2),
            a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0),
            a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1),
        ],
        [
            a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1),
            a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2),
            a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0),
        ],
    ];
    let det = a(0, 0) * cof[0][0] + a(0, 1) * cof[0][1] + a(0, 2) * cof[0][2];
    let sign = if det < 0.0 { -1.0 } else { 1.0 };
    std::array::from_fn(|r| {
        (sign
            * (cof[r][0] * f64::from(n[0])
                + cof[r][1] * f64::from(n[1])
                + cof[r][2] * f64::from(n[2]))) as f32
    })
}

pub(super) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { v.map(|x| x / l) } else { v }
}

pub(super) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

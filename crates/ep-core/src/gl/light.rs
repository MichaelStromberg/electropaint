//! IrisGL lighting: `lmdef` definitions, `lmbind` bindings and the
//! per-vertex lighting model.

use super::math::{dot, normalize, transform_dir, transform_normal, transform_point};
use super::{Arg, Gl};

/// IrisGL `lmdef` tokens.
#[allow(
    dead_code,
    reason = "the complete IrisGL token table, not only the ones ep passes"
)]
pub(crate) mod lm {
    pub(crate) const DEFMATERIAL: i16 = 0;
    pub(crate) const DEFLIGHT: i16 = 100;
    pub(crate) const DEFLMODEL: i16 = 200;
    pub(crate) const LMNULL: f32 = 0.0;
    pub(crate) const EMISSION: f32 = 1.0;
    pub(crate) const AMBIENT: f32 = 2.0;
    pub(crate) const DIFFUSE: f32 = 3.0;
    pub(crate) const SPECULAR: f32 = 4.0;
    pub(crate) const SHININESS: f32 = 5.0;
    pub(crate) const COLORINDEXES: f32 = 6.0;
    pub(crate) const ALPHA: f32 = 7.0;
    pub(crate) const LCOLOR: f32 = 101.0;
    pub(crate) const POSITION: f32 = 102.0;
    pub(crate) const SPOTDIRECTION: f32 = 103.0;
    pub(crate) const SPOTLIGHT: f32 = 104.0;
    pub(crate) const LOCALVIEWER: f32 = 201.0;
    pub(crate) const ATTENUATION: f32 = 202.0;
    pub(crate) const ATTENUATION2: f32 = 203.0;
    pub(crate) const TWOSIDE: f32 = 204.0;
    /// `lmbind` targets.
    pub(crate) const MATERIAL: i16 = 1000;
    pub(crate) const BACKMATERIAL: i16 = 1001;
    pub(crate) const LIGHT0: i16 = 1100;
    pub(crate) const LMODEL: i16 = 1200;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Material {
    emission: [f32; 3],
    ambient: [f32; 3],
    diffuse: [f32; 3],
    specular: [f32; 3],
    shininess: f32,
    alpha: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            emission: [0.0; 3],
            ambient: [0.2; 3],
            diffuse: [0.8; 3],
            specular: [0.0; 3],
            shininess: 0.0,
            alpha: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Light {
    ambient: [f32; 3],
    color: [f32; 3],
    position: [f32; 4],
}

impl Default for Light {
    fn default() -> Self {
        Light {
            ambient: [0.0; 3],
            color: [1.0; 3],
            position: [0.0, 0.0, 1.0, 0.0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LModel {
    ambient: [f32; 3],
    local_viewer: bool,
}

impl Default for LModel {
    fn default() -> Self {
        LModel {
            ambient: [0.2; 3],
            local_viewer: false,
        }
    }
}

/// A light bound to a slot: its definition with the position already in
/// eye space (IrisGL transforms it by the modelview at `lmbind` time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BoundLight {
    light: Light,
    eye_pos: [f32; 4],
}

impl Gl {
    /// `lmdef(deftype, index, np, props)`. Properties not given keep their
    /// previous values (or the IrisGL defaults for a new index).
    pub(crate) fn lmdef(&mut self, deftype: i16, index: i16, props: &[f32]) {
        self.log("lmdef", || {
            vec![
                Arg::Int(deftype.into()),
                Arg::Int(index.into()),
                Arg::Int(props.len() as i64),
                Arg::Floats(props.to_vec()),
            ]
        });
        // NaN marks uninitialised memory in the original (see Arg::Floats);
        // use 0 for the state.
        let p: Vec<f32> = props
            .iter()
            .map(|&v| if v.is_nan() { 0.0 } else { v })
            .collect();
        let mut i = 0;
        let mut take = |n: usize| -> Option<&[f32]> {
            let s = p.get(i..i + n)?;
            i += n;
            Some(s)
        };
        let rgb = |s: &[f32]| [s[0], s[1], s[2]];
        match deftype {
            lm::DEFMATERIAL => {
                let mut m = lookup(&self.materials, index).unwrap_or_default();
                while let Some(&[tok]) = take(1) {
                    match tok {
                        lm::EMISSION => m.emission = take(3).map(rgb).unwrap_or(m.emission),
                        lm::AMBIENT => m.ambient = take(3).map(rgb).unwrap_or(m.ambient),
                        lm::DIFFUSE => m.diffuse = take(3).map(rgb).unwrap_or(m.diffuse),
                        lm::SPECULAR => m.specular = take(3).map(rgb).unwrap_or(m.specular),
                        lm::SHININESS => m.shininess = take(1).map(|s| s[0]).unwrap_or(m.shininess),
                        lm::COLORINDEXES => {
                            take(3);
                        }
                        lm::ALPHA => m.alpha = take(1).map(|s| s[0]).unwrap_or(m.alpha),
                        _ => break,
                    }
                }
                store(&mut self.materials, index, m);
            }
            lm::DEFLIGHT => {
                let mut l = lookup(&self.lights, index).unwrap_or_default();
                while let Some(&[tok]) = take(1) {
                    match tok {
                        lm::AMBIENT => l.ambient = take(3).map(rgb).unwrap_or(l.ambient),
                        lm::LCOLOR => l.color = take(3).map(rgb).unwrap_or(l.color),
                        lm::POSITION => {
                            if let Some(s) = take(4) {
                                l.position = [s[0], s[1], s[2], s[3]];
                            }
                        }
                        lm::SPOTDIRECTION => {
                            take(3);
                        }
                        lm::SPOTLIGHT => {
                            take(2);
                        }
                        _ => break,
                    }
                }
                store(&mut self.lights, index, l);
            }
            lm::DEFLMODEL => {
                let mut l = lookup(&self.lmodels, index).unwrap_or_default();
                while let Some(&[tok]) = take(1) {
                    match tok {
                        lm::AMBIENT => l.ambient = take(3).map(rgb).unwrap_or(l.ambient),
                        lm::LOCALVIEWER => l.local_viewer = take(1).is_some_and(|s| s[0] != 0.0),
                        lm::ATTENUATION => {
                            take(2);
                        }
                        lm::ATTENUATION2 | lm::TWOSIDE => {
                            take(1);
                        }
                        _ => break,
                    }
                }
                store(&mut self.lmodels, index, l);
            }
            _ => {}
        }
    }

    /// `lmbind(target, index)`. Index 0 unbinds. A light's position is
    /// transformed by the modelview in force now.
    pub(crate) fn lmbind(&mut self, target: i16, index: i16) {
        self.log("lmbind", || {
            vec![Arg::Int(target.into()), Arg::Int(index.into())]
        });
        match target {
            lm::MATERIAL => {
                self.material =
                    (index != 0).then(|| lookup(&self.materials, index).unwrap_or_default())
            }
            lm::LMODEL => {
                self.lmodel = (index != 0).then(|| lookup(&self.lmodels, index).unwrap_or_default())
            }
            t if (lm::LIGHT0..lm::LIGHT0 + 8).contains(&t) => {
                let slot = usize::from((t - lm::LIGHT0) as u8);
                self.bound_lights[slot] = (index != 0).then(|| {
                    let light = lookup(&self.lights, index).unwrap_or_default();
                    let p = light.position;
                    let mv = self.modelview();
                    let eye_pos = if p[3] == 0.0 {
                        let d = transform_dir(mv, [p[0], p[1], p[2]]);
                        [d[0], d[1], d[2], 0.0]
                    } else {
                        let q = transform_point(mv, [p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
                        [q[0], q[1], q[2], 1.0]
                    };
                    BoundLight { light, eye_pos }
                });
            }
            _ => {}
        }
    }

    /// The colour of a vertex at eye-space position `pos`: lit if lighting
    /// is enabled and the last colour/normal command was a normal
    /// (`lmcolor(LMC_COLOR)`), else the current colour.
    pub(super) fn vertex_color(&self, pos: [f32; 3]) -> [f32; 4] {
        let (Some(m), Some(lmodel)) = (self.material, self.lmodel) else {
            return self.color;
        };
        if !self.normal_last {
            return self.color;
        }
        let n = normalize(transform_normal(self.modelview(), self.normal));
        let mut c: [f32; 3] =
            std::array::from_fn(|i| m.emission[i] + lmodel.ambient[i] * m.ambient[i]);
        let view = if lmodel.local_viewer {
            normalize(pos.map(|x| -x))
        } else {
            [0.0, 0.0, 1.0]
        };
        for b in self.bound_lights.iter().flatten() {
            let l = if b.eye_pos[3] == 0.0 {
                normalize([b.eye_pos[0], b.eye_pos[1], b.eye_pos[2]])
            } else {
                normalize([
                    b.eye_pos[0] - pos[0],
                    b.eye_pos[1] - pos[1],
                    b.eye_pos[2] - pos[2],
                ])
            };
            for (i, ci) in c.iter_mut().enumerate() {
                *ci += b.light.ambient[i] * m.ambient[i];
            }
            let d = dot(n, l);
            if d > 0.0 {
                for (i, ci) in c.iter_mut().enumerate() {
                    *ci += d * b.light.color[i] * m.diffuse[i];
                }
                // Shininess 0 turns specular off in IrisGL.
                if m.shininess > 0.0 {
                    let h = normalize([l[0] + view[0], l[1] + view[1], l[2] + view[2]]);
                    let s = dot(n, h).max(0.0).powf(m.shininess);
                    for (i, ci) in c.iter_mut().enumerate() {
                        *ci += s * b.light.color[i] * m.specular[i];
                    }
                }
            }
        }
        [
            c[0].clamp(0.0, 1.0),
            c[1].clamp(0.0, 1.0),
            c[2].clamp(0.0, 1.0),
            m.alpha,
        ]
    }
}

fn lookup<T: Copy>(v: &[(i16, T)], index: i16) -> Option<T> {
    v.iter().find(|e| e.0 == index).map(|e| e.1)
}

fn store<T>(v: &mut Vec<(i16, T)>, index: i16, x: T) {
    match v.iter_mut().find(|e| e.0 == index) {
        Some(e) => e.1 = x,
        None => v.push((index, x)),
    }
}

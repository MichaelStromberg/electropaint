//! ep's lighting set-up: `deflight`, `uselight` and `ep_lmcolor`.

use super::{HAS_LIGHTING, Sim};
use crate::gl::{Gl, lm};

impl Sim {
    /// `uselight`: bind lights 5 and 6, the material and the lighting model.
    pub(crate) fn uselight(&mut self) {
        if HAS_LIGHTING {
            self.gl.lmbind(lm::LIGHT0 + 5, 5);
            self.gl.lmbind(lm::MATERIAL, 1);
            self.gl.lmbind(lm::LIGHT0 + 6, 6);
            self.gl.lmbind(lm::LMODEL, 1);
        }
    }

    /// `ep_lmcolor`: redefine material 1 from a colour and bind it.
    /// `amb` multiplies the colour for the ambient term; `None` stands for
    /// the uninitialised value the ribbon code passes (logged as NaN,
    /// treated as 0).
    pub(crate) fn lmcolor(&mut self, rgba: [f32; 4], amb: Option<f32>, shiny: f32) {
        if !HAS_LIGHTING {
            return;
        }
        let a = |c: f32| amb.map_or(f32::NAN, |m| c * m);
        let shininess = (f64::from(shiny) * 128.0).floor() as f32;
        let props = [
            2.0,
            a(rgba[0]),
            a(rgba[1]),
            a(rgba[2]),
            3.0,
            rgba[0],
            rgba[1],
            rgba[2],
            4.0,
            1.0,
            1.0,
            1.0,
            5.0,
            shininess,
            7.0,
            rgba[3],
            0.0,
        ];
        self.gl.lmdef(lm::DEFMATERIAL, 1, &props);
        self.gl.lmbind(lm::MATERIAL, 1);
    }
}

/// `deflight`: material 1, lights 1–7 and lighting model 1.
pub(super) fn deflight(gl: &mut Gl) {
    gl.lmdef(
        lm::DEFMATERIAL,
        1,
        &[
            2.0, 0.2, 0.2, 0.2, 3.0, 0.8, 0.8, 0.8, 4.0, 1.0, 1.0, 1.0, 5.0, 80.0, 0.0,
        ],
    );
    let positions: [[f32; 3]; 7] = [
        [1.0, 0.0, 0.5],
        [-1.0, 0.0, -0.5],
        [1.0, 0.0, -0.5],
        [-1.0, 0.0, 0.5],
        [0.0, 1.0, 0.5],
        [0.0, -1.0, -0.5],
        [0.0, 1.0, -0.5],
    ];
    for (i, p) in positions.iter().enumerate() {
        gl.lmdef(
            lm::DEFLIGHT,
            i as i16 + 1,
            &[101.0, 1.0, 1.0, 1.0, 102.0, p[0], p[1], p[2], 0.0, 0.0],
        );
    }
    gl.lmdef(
        lm::DEFLMODEL,
        1,
        &[2.0, 1.0, 1.0, 1.0, 201.0, 0.0, 202.0, 1.0, 1.0, 0.0],
    );
}

//! SVID `drand48`/`srand48`, bit-identical to IRIX libc.

const A: u64 = 0x5_DEEC_E66D;
const C: u64 = 0xB;
const MASK: u64 = (1 << 48) - 1;

/// The 48-bit linear congruential generator behind `drand48`.
#[derive(Clone, Debug)]
pub(crate) struct Rand48 {
    x: u64,
}

impl Rand48 {
    /// Equivalent to `srand48(seed)`: only the low 32 bits of the seed are used.
    pub(crate) fn new(seed: i64) -> Self {
        let mut r = Rand48 { x: 0 };
        r.srand48(seed);
        r
    }

    pub(crate) fn srand48(&mut self, seed: i64) {
        self.x = (((seed as u64) & 0xFFFF_FFFF) << 16) | 0x330E;
    }

    pub(crate) fn drand48(&mut self) -> f64 {
        self.x = A.wrapping_mul(self.x).wrapping_add(C) & MASK;
        self.x as f64 / (1u64 << 48) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        let mut r = Rand48::new(12345);
        assert_eq!(r.drand48().to_bits(), 0x3fcc_d790_90a8_8080);
        assert_eq!(r.drand48().to_bits(), 0x3fed_69f2_9c4c_6f00);
        assert_eq!(r.drand48().to_bits(), 0x3fca_79c6_3115_1180);
    }

    /// Compares against IRIX output in `re/fixtures` (git-ignored; skipped if absent).
    #[test]
    fn irix_fixtures() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/fixtures");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return crate::testing::re_missing(dir);
        };
        for e in entries.flatten() {
            let name = e.file_name().into_string().unwrap();
            let Some(seed) = name
                .strip_prefix("d48_")
                .and_then(|s| s.strip_suffix(".txt"))
            else {
                continue;
            };
            let mut r = Rand48::new(seed.parse().unwrap());
            for line in std::fs::read_to_string(e.path()).unwrap().lines() {
                let hex = line.split_whitespace().nth(2).unwrap();
                let want = u64::from_str_radix(hex, 16).unwrap();
                assert_eq!(r.drand48().to_bits(), want, "seed {seed}: {line}");
            }
        }
    }
}

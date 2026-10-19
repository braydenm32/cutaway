//! Colour lookup tables in the .cube format (Adobe/Resolve), as cameras
//! that shoot in log ship them: S-Log3 to Rec.709, V-Log to Rec.709, a
//! film look… Only 3D tables (`LUT_3D_SIZE`) are used for pictures.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// A 3D colour table: `size`³ entries, red changing fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub title: String,
    pub size: u32,
    /// Inputs that map to the first and last entries (usually 0 and 1).
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub data: Vec<[f32; 3]>,
}

impl Lut3d {
    pub fn load(path: &Path) -> Result<Lut3d> {
        let text = std::fs::read_to_string(path).with_context(|| format!("couldn't read {}", path.display()))?;
        Lut3d::parse(&text).with_context(|| format!("{} isn't a usable .cube LUT", path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()))
    }

    pub fn parse(text: &str) -> Result<Lut3d> {
        let mut lut = Lut3d { title: String::new(), size: 0, domain_min: [0.0; 3], domain_max: [1.0; 3], data: Vec::new() };
        let three = |rest: &str| -> Result<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().map(|x| x.parse::<f32>()).collect::<Result<_, _>>().context("a number that isn't one")?;
            if v.len() != 3 {
                bail!("expected three numbers, found {}", v.len());
            }
            Ok([v[0], v[1], v[2]])
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (word, rest) = line.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((line, ""));
            match word.to_ascii_uppercase().as_str() {
                "TITLE" => lut.title = rest.trim_matches('"').to_string(),
                "LUT_3D_SIZE" => {
                    let s: u32 = rest.parse().context("LUT_3D_SIZE")?;
                    if !(2..=128).contains(&s) {
                        bail!("a {s}-point table is too {}", if s < 2 { "small" } else { "big" });
                    }
                    lut.size = s;
                    lut.data.reserve((s * s * s) as usize);
                }
                "LUT_1D_SIZE" => bail!("it's a 1D LUT; pictures need a 3D one (LUT_3D_SIZE)"),
                "DOMAIN_MIN" => lut.domain_min = three(rest).with_context(|| format!("line {}", n + 1))?,
                "DOMAIN_MAX" => lut.domain_max = three(rest).with_context(|| format!("line {}", n + 1))?,
                // Resolve's older spelling: one range for all three.
                "LUT_3D_INPUT_RANGE" => {
                    let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                    if v.len() == 2 {
                        lut.domain_min = [v[0]; 3];
                        lut.domain_max = [v[1]; 3];
                    }
                }
                w if w.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.' || c == '+') => {
                    if lut.size == 0 {
                        bail!("values before LUT_3D_SIZE (line {})", n + 1);
                    }
                    lut.data.push(three(line).with_context(|| format!("line {}", n + 1))?);
                }
                // Other keywords (LUT_IN_VIDEO_RANGE…) don't change the table.
                _ => {}
            }
        }
        if lut.size == 0 {
            bail!("no LUT_3D_SIZE");
        }
        let want = (lut.size * lut.size * lut.size) as usize;
        if lut.data.len() != want {
            bail!("{} values for a {}-point table (needs {want})", lut.data.len(), lut.size);
        }
        for k in 0..3 {
            if lut.domain_max[k] <= lut.domain_min[k] {
                bail!("DOMAIN_MAX must be above DOMAIN_MIN");
            }
        }
        Ok(lut)
    }

    /// One line for the settings: "33-point · S-Log3 to Rec.709".
    pub fn describe(&self) -> String {
        if self.title.is_empty() {
            format!("{}-point table", self.size)
        } else {
            format!("{}-point table · {}", self.size, self.title)
        }
    }

    /// Look a colour up (trilinear), for tests.
    #[cfg(test)]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size as usize;
        let pos: Vec<f32> = (0..3).map(|k| ((rgb[k] - self.domain_min[k]) / (self.domain_max[k] - self.domain_min[k])).clamp(0.0, 1.0) * (n - 1) as f32).collect();
        let i0: Vec<usize> = pos.iter().map(|p| (p.floor() as usize).min(n - 1)).collect();
        let i1: Vec<usize> = i0.iter().map(|i| (i + 1).min(n - 1)).collect();
        let f: Vec<f32> = pos.iter().zip(&i0).map(|(p, i)| p - *i as f32).collect();
        let at = |r: usize, g: usize, b: usize| self.data[r + g * n + b * n * n];
        let mut out = [0.0f32; 3];
        for (dr, wr) in [(i0[0], 1.0 - f[0]), (i1[0], f[0])] {
            for (dg, wg) in [(i0[1], 1.0 - f[1]), (i1[1], f[1])] {
                for (db, wb) in [(i0[2], 1.0 - f[2]), (i1[2], f[2])] {
                    let v = at(dr, dg, db);
                    let w = wr * wg * wb;
                    for k in 0..3 {
                        out[k] += v[k] * w;
                    }
                }
            }
        }
        out
    }

    /// The table as RGBA half floats (red fastest), for a 3D texture.
    pub fn rgba_f16(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for v in &self.data {
            out.extend([f16_bits(v[0]), f16_bits(v[1]), f16_bits(v[2]), f16_bits(1.0)]);
        }
        out
    }
}

/// An f32 as IEEE half-float bits (rounded to nearest).
pub fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - e);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    let h = sign as u32 | ((e as u32) << 10) | (mant >> 13);
    // Round to nearest (a carry into the exponent is still correct).
    (h + ((mant >> 12) & 1)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An identity table of `n` points, with a title and comments.
    fn identity(n: usize) -> String {
        let mut s = String::from("# made for a test\nTITLE \"Identity\"\nLUT_3D_SIZE ");
        s.push_str(&format!("{n}\n"));
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |i: usize| i as f32 / (n - 1) as f32;
                    s.push_str(&format!("{:.6} {:.6} {:.6}\n", f(r), f(g), f(b)));
                }
            }
        }
        s
    }

    #[test]
    fn reads_cube_files() {
        let lut = Lut3d::parse(&identity(5)).unwrap();
        assert_eq!(lut.size, 5);
        assert_eq!(lut.title, "Identity");
        assert_eq!(lut.describe(), "5-point table · Identity");
        // An identity table gives back what goes in, between points too.
        let c = lut.apply([0.3, 0.71, 0.05]);
        for (a, b) in c.iter().zip([0.3, 0.71, 0.05]) {
            assert!((a - b).abs() < 1e-4, "{c:?}");
        }
        // Red changes fastest.
        assert_eq!(lut.data[1], [0.25, 0.0, 0.0]);
        assert!(Lut3d::parse("LUT_1D_SIZE 1024\n0 0 0\n").unwrap_err().to_string().contains("1D"));
        assert!(Lut3d::parse("LUT_3D_SIZE 2\n0 0 0\n1 1 1\n").unwrap_err().to_string().contains("needs 8"));
        assert!(Lut3d::parse("0 0 0\n").is_err());
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e9), 0x7c00);
        // A small (subnormal) value.
        assert_eq!(f16_bits(2f32.powi(-20)), 0x0010);
    }
}

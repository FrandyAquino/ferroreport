//! Page pictures compared pixel by pixel, for `migrate --jasper`: the same measure as
//! `tools/compare.py` (and `tools/samples.py`, which uses it), so a share means the same
//! thing everywhere.
//!
//! Both PDFs are rasterized by `pdftoppm` (poppler) at 100 DPI in gray. A pixel differs when
//! the two grays are more than 64 apart; a pixel is ink when either is darker than 200. A
//! page's share is its differing pixels over its ink pixels, and a page matches when that
//! share is at most [`MATCH_THRESHOLD`]. A page only one engine has counts as wholly
//! different (1.0).

use std::path::{Path, PathBuf};

/// Share of a page's ink allowed to differ (anti-aliasing noise): `MATCH_THRESHOLD` of
/// tools/compare.py.
pub const MATCH_THRESHOLD: f64 = 0.0015;
/// Resolution the pages are rasterized at: the default `--dpi` of tools/compare.py.
pub const DPI: u32 = 100;
/// Two grays further apart than this differ.
const DIFFERENT: i16 = 64;
/// A gray darker than this is ink.
const INK: u8 = 200;

/// An 8-bit gray picture of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Gray {
    fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x]
        } else {
            // The smaller page is padded with white, as compare.py does.
            255
        }
    }
}

/// An 8-bit RGB picture.
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Compares two pages: the share of their ink that differs, and the diff picture (grey
/// content, red where only `a` (JasperReports) has ink, blue where only `b` (ferroreport)
/// does), as `diff_page` of tools/compare.py.
pub fn diff_page(a: &Gray, b: &Gray) -> (f64, Rgb) {
    let (w, h) = (a.width.max(b.width), a.height.max(b.height));
    let mut picture = Vec::with_capacity(w * h * 3);
    let (mut differing, mut ink) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let (pa, pb) = (a.at(x, y), b.at(x, y));
            let delta = (pa as i16 - pb as i16).abs() > DIFFERENT;
            differing += delta as u64;
            ink += (pa < INK || pb < INK) as u64;
            let color = if delta && pa < pb {
                [220, 30, 30]
            } else if delta && pb < pa {
                [30, 80, 230]
            } else {
                // numpy: (uint8 * 0.35 + 165).astype(uint8), which truncates.
                let g = (pa as f64 * 0.35 + 165.0) as u8;
                [g, g, g]
            };
            picture.extend_from_slice(&color);
        }
    }
    let share = if ink == 0 { 0.0 } else { differing as f64 / ink as f64 };
    (share, Rgb { width: w, height: h, pixels: picture })
}

/// Reads a PNG as 8-bit gray (RGB is converted with the weights of PIL's `convert("L")`).
pub fn decode_png(bytes: &[u8]) -> Result<Gray, String> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("PNG: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
    let buf = &buf[..info.buffer_size()];
    let luma = |r: u8, g: u8, b: u8| ((r as u32 * 19595 + g as u32 * 38470 + b as u32 * 7471 + 0x8000) >> 16) as u8;
    let pixels: Vec<u8> = match info.color_type {
        png::ColorType::Grayscale => buf.to_vec(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).map(|p| p[0]).collect(),
        png::ColorType::Rgb => buf.chunks_exact(3).map(|p| luma(p[0], p[1], p[2])).collect(),
        png::ColorType::Rgba => buf.chunks_exact(4).map(|p| luma(p[0], p[1], p[2])).collect(),
        png::ColorType::Indexed => return Err("PNG: unexpanded palette".into()),
    };
    Ok(Gray { width: info.width as usize, height: info.height as usize, pixels })
}

/// Writes an RGB picture as PNG.
pub fn write_png(picture: &Rgb, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), picture.width as u32, picture.height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| format!("{}: {e}", path.display()))?;
    writer.write_image_data(&picture.pixels).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether `pdftoppm` can be run.
pub fn pdftoppm_available() -> bool {
    std::process::Command::new("pdftoppm")
        .arg("-v")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Rasterizes every page of `pdf` as `<prefix>-<n>.png`, in page order.
fn rasterize(pdf: &Path, prefix: &Path) -> Result<Vec<PathBuf>, String> {
    let out = std::process::Command::new("pdftoppm")
        .args(["-r", &DPI.to_string(), "-gray", "-png"])
        .arg(pdf)
        .arg(prefix)
        .output()
        .map_err(|e| format!("pdftoppm: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("pdftoppm could not read {}: {}", pdf.display(), why.trim()));
    }
    let dir = prefix.parent().unwrap_or(Path::new("."));
    let stem = prefix.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            name.strip_prefix(&stem).is_some_and(|rest| rest.starts_with('-') && rest.ends_with(".png"))
        })
        .collect();
    // pdftoppm pads the page numbers to the same width, so names sort in page order.
    pages.sort();
    Ok(pages)
}

/// How the pictures of two renderings of a template compare.
#[derive(Debug, Clone, PartialEq)]
pub struct Pictures {
    /// The page whose share is the largest (1-based), when there is a page at all.
    pub worst_page: Option<usize>,
    /// The share of that page's ink that differs (1.0 for a page only one engine has).
    pub worst_share: f64,
    /// Pages over the threshold, or that only one engine has.
    pub pages_differing: usize,
    /// Diff pictures written, one per differing page both engines have.
    pub diff_files: Vec<String>,
}

impl Pictures {
    /// Every page is the same picture.
    pub fn same(&self) -> bool {
        self.pages_differing == 0
    }

    /// Shares of the pages of the two engines, in page order.
    fn from_shares(shares: &[f64]) -> Self {
        let mut worst: Option<(usize, f64)> = None;
        for (i, &s) in shares.iter().enumerate() {
            if worst.is_none_or(|(_, w)| s > w) {
                worst = Some((i + 1, s));
            }
        }
        Pictures {
            worst_page: worst.map(|w| w.0),
            worst_share: worst.map_or(0.0, |w| w.1),
            pages_differing: shares.iter().filter(|&&s| s > MATCH_THRESHOLD).count(),
            diff_files: Vec::new(),
        }
    }
}

/// Rasterizes both PDFs in `work` and compares them page by page. With `keep`, writes the
/// diff picture of each page that differs as `<keep>.diff-<page>.png`.
pub fn compare_pdfs(jasper: &Path, ours: &Path, work: &Path, keep: Option<&Path>) -> Result<Pictures, String> {
    let pa = rasterize(jasper, &work.join("jasper-page"))?;
    let pb = rasterize(ours, &work.join("ferroreport-page"))?;
    let load = |p: &PathBuf| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display())).and_then(|b| decode_png(&b));
    let mut shares = Vec::new();
    let mut pictures = Vec::new();
    for i in 0..pa.len().max(pb.len()) {
        match (pa.get(i), pb.get(i)) {
            (Some(a), Some(b)) => {
                let (share, picture) = diff_page(&load(a)?, &load(b)?);
                if share > MATCH_THRESHOLD {
                    if let Some(keep) = keep {
                        let mut name = keep.as_os_str().to_os_string();
                        name.push(format!(".diff-{}.png", i + 1));
                        let path = PathBuf::from(name);
                        write_png(&picture, &path)?;
                        pictures.push(path.to_string_lossy().into_owned());
                    }
                }
                shares.push(share);
            }
            _ => shares.push(1.0),
        }
    }
    for p in pa.iter().chain(&pb) {
        let _ = std::fs::remove_file(p);
    }
    let mut result = Pictures::from_shares(&shares);
    result.diff_files = pictures;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width: usize, height: usize, gray: u8) -> Gray {
        Gray { width, height, pixels: vec![gray; width * height] }
    }

    fn fill(g: &mut Gray, x0: usize, y0: usize, x1: usize, y1: usize, value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                g.pixels[y * g.width + x] = value;
            }
        }
    }

    #[test]
    fn identical_pages_do_not_differ() {
        let mut a = page(50, 40, 255);
        fill(&mut a, 5, 5, 30, 20, 0);
        let (share, picture) = diff_page(&a, &a.clone());
        assert_eq!(share, 0.0);
        assert_eq!((picture.width, picture.height), (50, 40));
        // Grey content: white becomes 254, black 165, and nothing red or blue.
        assert_eq!(&picture.pixels[..3], &[254, 254, 254]);
        let black = (5 * 50 + 5) * 3;
        assert_eq!(&picture.pixels[black..black + 3], &[165, 165, 165]);
    }

    #[test]
    fn blank_pages_do_not_differ() {
        assert_eq!(diff_page(&page(10, 10, 255), &page(10, 10, 255)).0, 0.0);
    }

    #[test]
    fn a_known_region_gives_a_known_share() {
        // Both have a 10×10 black square; only Jasper has another 10×5 one, and only
        // ferroreport a 5×4 one: 100 + 50 + 20 ink pixels, 70 of them differ.
        let mut a = page(100, 100, 255);
        let mut b = page(100, 100, 255);
        fill(&mut a, 0, 0, 10, 10, 0);
        fill(&mut b, 0, 0, 10, 10, 0);
        fill(&mut a, 50, 50, 60, 55, 0);
        fill(&mut b, 80, 80, 85, 84, 0);
        let (share, picture) = diff_page(&a, &b);
        assert!((share - 70.0 / 170.0).abs() < 1e-12, "{share}");
        let px = |x: usize, y: usize| &picture.pixels[(y * 100 + x) * 3..(y * 100 + x) * 3 + 3];
        assert_eq!(px(52, 52), &[220, 30, 30]);
        assert_eq!(px(82, 82), &[30, 80, 230]);
        assert_eq!(px(5, 5), &[165, 165, 165]);
    }

    #[test]
    fn small_grey_changes_are_not_differences() {
        // 64 apart is still the same; 65 apart differs. Both are ink (under 200).
        let (a, b, c) = (page(4, 4, 100), page(4, 4, 164), page(4, 4, 165));
        assert_eq!(diff_page(&a, &b).0, 0.0);
        assert_eq!(diff_page(&a, &c).0, 1.0);
        // Light greys (200 and up) are not ink, so a page of them has no share at all.
        assert_eq!(diff_page(&page(4, 4, 200), &page(4, 4, 255)).0, 0.0);
    }

    #[test]
    fn a_smaller_page_is_padded_with_white() {
        // Jasper's page is 10×10 and black; ferroreport's is 10×20 with the same black top
        // and a black bottom half Jasper has as white padding.
        let a = page(10, 10, 0);
        let mut b = page(10, 20, 255);
        fill(&mut b, 0, 0, 10, 20, 0);
        let (share, picture) = diff_page(&a, &b);
        assert_eq!((picture.width, picture.height), (10, 20));
        assert!((share - 0.5).abs() < 1e-12, "{share}");
        // The padding only ferroreport fills is blue.
        assert_eq!(&picture.pixels[(15 * 10) * 3..(15 * 10) * 3 + 3], &[30, 80, 230]);
        // White padding against white is no difference and no ink.
        let (share, _) = diff_page(&page(10, 10, 255), &page(12, 12, 255));
        assert_eq!(share, 0.0);
    }

    #[test]
    fn the_worst_page_and_missing_pages() {
        let p = Pictures::from_shares(&[0.0, 0.001, 0.2, 0.0015]);
        assert_eq!(p.worst_page, Some(3));
        assert_eq!(p.worst_share, 0.2);
        // 0.0015 is at the threshold, so it still matches.
        assert_eq!(p.pages_differing, 1);
        assert!(!p.same());
        let p = Pictures::from_shares(&[0.0, 1.0]);
        assert_eq!((p.worst_page, p.pages_differing), (Some(2), 1));
        let p = Pictures::from_shares(&[]);
        assert_eq!((p.worst_page, p.worst_share, p.same()), (None, 0.0, true));
    }

    #[test]
    fn png_round_trip() {
        let picture =
            Rgb { width: 3, height: 2, pixels: vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 128, 128, 128] };
        let path = std::env::temp_dir().join(format!("ferroreport-pixels-{}.png", std::process::id()));
        write_png(&picture, &path).unwrap();
        let gray = decode_png(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!((gray.width, gray.height), (3, 2));
        // PIL's convert("L"): red 76, green 150, blue 29.
        assert_eq!(gray.pixels, vec![76, 150, 29, 0, 255, 128]);
    }
}

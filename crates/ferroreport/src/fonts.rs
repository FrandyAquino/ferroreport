//! Font registry and metrics.
//!
//! The DejaVu families that ship with JasperReports (`jasperreports-fonts`) are embedded, so
//! templates designed against them lay out identically. Fonts are parsed once per process
//! and shared between renders. Additional TrueType fonts can be registered per render.

use crate::error::{Error, Result};
use crate::model::FontSpec;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Metrics and glyph tables of one font face.
#[derive(Debug)]
pub struct FaceData {
    pub family: String,
    pub postscript_name: String,
    pub data: Arc<[u8]>,
    pub units_per_em: f32,
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    /// OS/2 typographic ascender and descender (both positive), used by iText for backgrounds.
    pub typo_ascender: f32,
    pub typo_descender: f32,
    pub cap_height: f32,
    pub italic_angle: f32,
    pub bbox: [f32; 4],
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikeout_position: f32,
    pub strikeout_thickness: f32,
    pub bold: bool,
    pub italic: bool,
    /// Glyph ids of the Basic Multilingual Plane (0 = none), the hot path of text layout.
    bmp: Vec<u16>,
    /// Glyph ids beyond the BMP.
    cmap: HashMap<char, u16>,
    advances: Vec<u16>,
    shaping: crate::shape::ShapingCache,
    /// A style the face's family has no face for, which PDF output simulates as JasperReports
    /// (iText) does: bold by also stroking the outlines, italic by slanting the text.
    pub(crate) simulated: Simulated,
}

/// Styles simulated on a face that lacks them (see [`FaceData`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Simulated {
    pub bold: bool,
    pub italic: bool,
}

/// How far iText slants simulated italic text (`PdfChunk.ITALIC_ANGLE`).
pub(crate) const SIMULATED_ITALIC_SKEW: f32 = 0.21256;

impl FaceData {
    pub(crate) fn shaping_cache(&self) -> &crate::shape::ShapingCache {
        &self.shaping
    }

    pub fn parse(family: &str, data: Arc<[u8]>, bold: bool, italic: bool) -> Result<FaceData> {
        let face = ttf_parser::Face::parse(&data, 0).map_err(|e| Error::Font(format!("{family}: {e}")))?;
        let mut cmap = HashMap::new();
        let mut bmp = vec![0u16; 0x10000];
        if let Some(table) = face.tables().cmap {
            for sub in table.subtables {
                if !sub.is_unicode() {
                    continue;
                }
                sub.codepoints(|cp| {
                    if let (Some(c), Some(g)) = (char::from_u32(cp), sub.glyph_index(cp)) {
                        if cp < 0x10000 {
                            if bmp[cp as usize] == 0 {
                                bmp[cp as usize] = g.0;
                            }
                        } else {
                            cmap.entry(c).or_insert(g.0);
                        }
                    }
                });
            }
        }
        let advances =
            (0..face.number_of_glyphs()).map(|g| face.glyph_hor_advance(ttf_parser::GlyphId(g)).unwrap_or(0)).collect();
        let upem = face.units_per_em() as f32;
        let bb = face.global_bounding_box();
        let ps_name = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME && n.is_unicode())
            .and_then(|n| n.to_string())
            .unwrap_or_else(|| family.replace(' ', ""));
        let (ul_pos, ul_th) =
            face.underline_metrics().map(|m| (m.position as f32, m.thickness as f32)).unwrap_or((-0.1 * upem, 0.05 * upem));
        let (st_pos, st_th) =
            face.strikeout_metrics().map(|m| (m.position as f32, m.thickness as f32)).unwrap_or((0.3 * upem, 0.05 * upem));
        Ok(FaceData {
            family: family.to_string(),
            postscript_name: ps_name,
            units_per_em: upem,
            ascender: face.ascender() as f32,
            descender: -(face.descender() as f32),
            line_gap: face.line_gap() as f32,
            typo_ascender: face.typographic_ascender().unwrap_or(face.ascender()) as f32,
            typo_descender: -(face.typographic_descender().unwrap_or(face.descender()) as f32),
            cap_height: face.capital_height().map(|c| c as f32).unwrap_or(0.7 * upem),
            italic_angle: face.italic_angle(),
            bbox: [bb.x_min as f32, bb.y_min as f32, bb.x_max as f32, bb.y_max as f32],
            underline_position: ul_pos,
            underline_thickness: ul_th,
            strikeout_position: st_pos,
            strikeout_thickness: st_th,
            bold,
            italic,
            bmp,
            cmap,
            advances,
            data,
            shaping: Default::default(),
            simulated: Simulated::default(),
        })
    }

    /// This face drawn in a style it does not have: `bold` and `italic` simulated.
    fn simulating(&self, bold: bool, italic: bool) -> FaceData {
        FaceData {
            family: self.family.clone(),
            postscript_name: self.postscript_name.clone(),
            data: self.data.clone(),
            units_per_em: self.units_per_em,
            ascender: self.ascender,
            descender: self.descender,
            line_gap: self.line_gap,
            typo_ascender: self.typo_ascender,
            typo_descender: self.typo_descender,
            cap_height: self.cap_height,
            italic_angle: self.italic_angle,
            bbox: self.bbox,
            underline_position: self.underline_position,
            underline_thickness: self.underline_thickness,
            strikeout_position: self.strikeout_position,
            strikeout_thickness: self.strikeout_thickness,
            bold: self.bold || bold,
            italic: self.italic || italic,
            bmp: self.bmp.clone(),
            cmap: self.cmap.clone(),
            advances: self.advances.clone(),
            shaping: Default::default(),
            simulated: Simulated { bold, italic },
        }
    }

    /// Appends the outline of `c` at `size` units per em, with its origin at `(x, y)` and y
    /// growing downwards.
    pub fn glyph_outline(&self, c: char, size: f64, x: f64, y: f64, out: &mut Vec<crate::print::PathOp>) {
        if c.is_control() {
            return;
        }
        self.glyph_id_outline(self.glyph(c), size, x, y, out);
    }

    /// Appends the outline of glyph `glyph`, like [`FaceData::glyph_outline`].
    pub fn glyph_id_outline(&self, glyph: u16, size: f64, x: f64, y: f64, out: &mut Vec<crate::print::PathOp>) {
        use crate::print::PathOp;
        struct Builder<'a> {
            out: &'a mut Vec<PathOp>,
            k: f64,
            x: f64,
            y: f64,
        }
        impl Builder<'_> {
            fn p(&self, px: f32, py: f32) -> (f64, f64) {
                (self.x + px as f64 * self.k, self.y - py as f64 * self.k)
            }
        }
        impl ttf_parser::OutlineBuilder for Builder<'_> {
            fn move_to(&mut self, x: f32, y: f32) {
                let (a, b) = self.p(x, y);
                self.out.push(PathOp::Move(a, b));
            }
            fn line_to(&mut self, x: f32, y: f32) {
                let (a, b) = self.p(x, y);
                self.out.push(PathOp::Line(a, b));
            }
            fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                let (a, b) = self.p(x1, y1);
                let (c, d) = self.p(x, y);
                self.out.push(PathOp::Quad(a, b, c, d));
            }
            fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                let (a, b) = self.p(x1, y1);
                let (c, d) = self.p(x2, y2);
                let (e, f) = self.p(x, y);
                self.out.push(PathOp::Cubic(a, b, c, d, e, f));
            }
            fn close(&mut self) {
                self.out.push(PathOp::Close);
            }
        }
        let Ok(face) = ttf_parser::Face::parse(&self.data, 0) else { return };
        let mut b = Builder { out, k: size / self.units_per_em as f64, x, y };
        face.outline_glyph(ttf_parser::GlyphId(glyph), &mut b);
    }

    /// Right edge of the outline of `c`'s glyph, in font units (0 for empty glyphs).
    pub fn glyph_x_max(&self, c: char) -> f64 {
        let Ok(face) = ttf_parser::Face::parse(&self.data, 0) else { return 0.0 };
        face.glyph_bounding_box(ttf_parser::GlyphId(self.glyph(c))).map(|b| b.x_max as f64).unwrap_or(0.0)
    }

    /// Glyph for `c`, or the `.notdef` glyph.
    pub fn glyph(&self, c: char) -> u16 {
        match self.bmp.get(c as usize) {
            Some(&g) => g,
            None => self.cmap.get(&c).copied().unwrap_or(0),
        }
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.glyph(c) != 0
    }

    /// Advance of glyph `g` in font units.
    pub fn advance(&self, g: u16) -> f32 {
        self.advances.get(g as usize).copied().unwrap_or(0) as f32
    }

    /// Width of `text` as JasperReports measures it while filling (AWT, fractional metrics,
    /// no kerning). Decides how tall a stretching text gets and where it is cut.
    pub fn fill_width(&self, text: &str, size: f32) -> f32 {
        text.chars().map(|c| self.fill_char_width(c, size)).sum()
    }

    pub fn fill_char_width(&self, c: char, size: f32) -> f32 {
        if c.is_control() {
            return 0.0;
        }
        (self.advance(self.glyph(c)) as f64 * size as f64 / self.units_per_em as f64) as f32
    }

    /// Width of `text` as JasperReports' PDF exporter measures it when drawing: each glyph
    /// advance truncated to whole thousandths of an em (iText font metrics). Decides the
    /// drawn line breaks and the alignment offsets.
    pub fn text_width(&self, text: &str, size: f32) -> f32 {
        let units: i64 = text.chars().map(|c| self.advance_millis(c)).sum();
        (units as f64 * size as f64 / 1000.0) as f32
    }

    pub fn char_width(&self, c: char, size: f32) -> f32 {
        (self.advance_millis(c) as f64 * size as f64 / 1000.0) as f32
    }

    /// Glyph advance in thousandths of an em, truncated.
    fn advance_millis(&self, c: char) -> i64 {
        // Control characters (U+0000-001F, U+007F-009F) are neither drawn nor measured, as in
        // Java; nor, when exporting, are characters the font lacks: JasperReports' PDF exporter
        // (iText) gives them no width and leaves them out, though filling (AWT) measured them
        // with the font's missing-glyph box.
        let glyph = self.glyph(c);
        if c.is_control() || glyph == 0 {
            return 0;
        }
        (self.advance(glyph) as f64 * 1000.0 / self.units_per_em as f64) as i64
    }

    /// Distance from the top of a line to its baseline.
    pub fn ascent(&self, size: f32) -> f32 {
        self.ascender * size / self.units_per_em
    }

    pub fn descent(&self, size: f32) -> f32 {
        self.descender * size / self.units_per_em
    }

    /// Typographic ascent and descent at `size` (iText's ASCENT and DESCENT).
    pub fn typo_metrics(&self, size: f32) -> (f32, f32) {
        (self.typo_ascender * size / self.units_per_em, self.typo_descender * size / self.units_per_em)
    }

    /// Single-spaced line height (ascent + descent + leading).
    pub fn line_height(&self, size: f32) -> f32 {
        (self.ascender + self.descender + self.line_gap) * size / self.units_per_em
    }
}

/// Identifies a face inside a [`FontSet`].
pub type FaceId = usize;

macro_rules! dejavu {
    ($file:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/", $file))
    };
}

/// (family, bold, italic, bytes)
const BUILTIN: &[(&str, bool, bool, &[u8])] = &[
    ("DejaVu Sans", false, false, dejavu!("DejaVuSans.ttf")),
    ("DejaVu Sans", true, false, dejavu!("DejaVuSans-Bold.ttf")),
    ("DejaVu Sans", false, true, dejavu!("DejaVuSans-Oblique.ttf")),
    ("DejaVu Sans", true, true, dejavu!("DejaVuSans-BoldOblique.ttf")),
    ("DejaVu Serif", false, false, dejavu!("DejaVuSerif.ttf")),
    ("DejaVu Serif", true, false, dejavu!("DejaVuSerif-Bold.ttf")),
    ("DejaVu Serif", false, true, dejavu!("DejaVuSerif-Italic.ttf")),
    ("DejaVu Serif", true, true, dejavu!("DejaVuSerif-BoldItalic.ttf")),
    ("DejaVu Sans Mono", false, false, dejavu!("DejaVuSansMono.ttf")),
    ("DejaVu Sans Mono", true, false, dejavu!("DejaVuSansMono-Bold.ttf")),
    ("DejaVu Sans Mono", false, true, dejavu!("DejaVuSansMono-Oblique.ttf")),
    ("DejaVu Sans Mono", true, true, dejavu!("DejaVuSansMono-BoldOblique.ttf")),
];

/// The files of the bundled fonts.
pub(crate) fn builtin_files() -> impl Iterator<Item = &'static [u8]> {
    BUILTIN.iter().map(|(_, _, _, bytes)| *bytes)
}

fn builtin_face(i: usize) -> &'static Arc<FaceData> {
    static CACHE: OnceLock<Vec<OnceLock<Arc<FaceData>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| (0..BUILTIN.len()).map(|_| OnceLock::new()).collect());
    cache[i].get_or_init(|| {
        let (family, bold, italic, bytes) = BUILTIN[i];
        Arc::new(FaceData::parse(family, Arc::from(bytes), bold, italic).expect("bundled font is valid"))
    })
}

/// Maps common family names onto the bundled DejaVu families.
fn builtin_family(name: &str) -> Option<&'static str> {
    let n = name.trim().to_ascii_lowercase();
    Some(match n.as_str() {
        "dejavu sans" | "sansserif" | "sans-serif" | "sans" | "dialog" | "arial" | "helvetica" | "verdana" | "tahoma"
        | "calibri" | "segoe ui" | "liberation sans" | "open sans" | "roboto" => "DejaVu Sans",
        "dejavu serif" | "serif" | "times new roman" | "times" | "times-roman" | "georgia" | "liberation serif" | "cambria" => {
            "DejaVu Serif"
        }
        "dejavu sans mono" | "monospaced" | "monospace" | "courier" | "courier new" | "consolas" | "liberation mono"
        | "dialoginput" => "DejaVu Sans Mono",
        _ => return None,
    })
}

/// Fonts available to one render: the bundled families plus any user-supplied faces.
#[derive(Default)]
pub struct FontSet {
    faces: Vec<Arc<FaceData>>,
    by_key: HashMap<(String, bool, bool), FaceId>,
    /// Families registered by the user (lowercase).
    user_families: std::collections::HashSet<String>,
    resolved: HashMap<(String, bool, bool), FaceId>,
    /// Families that had to be substituted, for diagnostics.
    pub substitutions: Vec<(String, String)>,
    /// User families in the order they were registered (as written), the first fonts tried
    /// for a character the text's own font lacks.
    user_order: Vec<String>,
    /// Font sets by lowercase name: their families in search order, the first one primary.
    sets: HashMap<String, Vec<String>>,
    /// Faces that simulate a style, with the face they are drawn from (whose font the PDF
    /// embeds for both).
    simulated_from: HashMap<FaceId, FaceId>,
    /// Paragraph settings (indents, tab stops) of the text elements of the reports being
    /// filled that have any, by [`crate::paragraph::fingerprint`].
    paragraphs: HashMap<u64, Arc<crate::paragraph::Paragraph>>,
    /// Whether every font falls back to the other fonts for characters it lacks.
    fallback: bool,
    /// The language of the render's locale (`en`), for texts hyphenated in it.
    language: String,
    /// The hyphenation the report being filled asks for: `None` off, `Some("")` in the
    /// locale's language, `Some("es")` in that language.
    hyphenation: Option<String>,
}

/// Characters every font "has", as Java's `Font.canDisplay` sees them: controls and the
/// invisible formatting characters are never drawn, so they never call for another font.
fn always_displayable(c: char) -> bool {
    c.is_control() || matches!(c, '\u{200C}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{206A}'..='\u{206F}')
}

impl FontSet {
    pub fn new() -> FontSet {
        FontSet::default()
    }

    /// Registers a TrueType/OpenType face under `family`.
    pub fn register(&mut self, family: &str, bold: bool, italic: bool, data: Vec<u8>) -> Result<()> {
        let face = FaceData::parse(family, Arc::from(data), bold, italic)?;
        let id = self.faces.len();
        self.faces.push(Arc::new(face));
        self.by_key.insert((family.to_ascii_lowercase(), bold, italic), id);
        if self.user_families.insert(family.to_ascii_lowercase()) {
            self.user_order.push(family.to_string());
        }
        self.resolved.clear();
        Ok(())
    }

    /// Whether `family` names a font this set draws with: a registered family or one of the
    /// bundled DejaVu families (common names such as `SansSerif` included).
    pub fn has_family(&self, family: &str) -> bool {
        self.user_families.contains(&family.to_ascii_lowercase()) || builtin_family(family).is_some()
    }

    /// Registers a font set, as a JasperReports font extension declares one (`<fontSet>`):
    /// a text whose font name is `name` is drawn in `families`, each run of characters in the
    /// first of them that has them all; characters none of them has stay in the first family.
    /// The families must be registered already (or be bundled ones).
    pub fn register_set(&mut self, name: &str, families: &[&str]) -> Result<()> {
        if families.is_empty() {
            return Err(Error::Font(format!("font set {name}: no families")));
        }
        if let Some(f) = families.iter().find(|f| !self.has_family(f)) {
            return Err(Error::Font(format!("font set {name}: unknown family {f}")));
        }
        self.sets.insert(name.trim().to_ascii_lowercase(), families.iter().map(|f| f.trim().to_string()).collect());
        Ok(())
    }

    /// Sets the language texts are hyphenated in when they do not name one (the locale's).
    pub(crate) fn set_language(&mut self, locale: &str) {
        self.language = locale.to_string();
    }

    /// Sets the hyphenation of the report being filled from the value of its
    /// [`crate::text::HYPHENATE`] parameter (`true`, `false`, a language), returning the one
    /// it replaces.
    pub(crate) fn set_hyphenation(&mut self, value: Option<&crate::value::Value>) -> Option<String> {
        let setting = value.filter(|v| !v.is_null()).map(|v| v.to_java_string()).and_then(|v| {
            let v = v.trim();
            match v.to_ascii_lowercase().as_str() {
                "" | "false" | "no" => None,
                "true" | "yes" => Some(String::new()),
                _ => Some(v.to_string()),
            }
        });
        std::mem::replace(&mut self.hyphenation, setting)
    }

    /// Puts back a hyphenation [`FontSet::set_hyphenation`] replaced.
    pub(crate) fn restore_hyphenation(&mut self, setting: Option<String>) {
        self.hyphenation = setting;
    }

    /// How the plain texts of the report being filled are hyphenated, if at all.
    pub(crate) fn hyphenator(&self) -> Option<crate::text::Hyphenator> {
        let language = self.hyphenation.as_deref()?;
        crate::text::Hyphenator::for_language(if language.is_empty() { &self.language } else { language })
    }

    /// Whether some texts may be drawn in more than one font (font sets or fallback).
    pub fn resolves_fonts(&self) -> bool {
        !self.sets.is_empty() || self.fallback
    }

    /// Turns font fallback on or off: with it on, characters a text's font lacks are drawn in
    /// the first font that has them, looking at the registered families (in the order they
    /// were registered) and then at the bundled DejaVu Sans, Serif and Sans Mono, as if every
    /// family were a font set of itself followed by those.
    pub fn set_fallback(&mut self, on: bool) {
        self.fallback = on;
    }

    /// The face of `family` in exactly the style asked for, if it has one.
    fn exact_face(&mut self, family: &str, bold: bool, italic: bool) -> Option<FaceId> {
        let key = family.to_ascii_lowercase();
        if self.user_families.contains(&key) {
            return self.by_key.get(&(key, bold, italic)).copied();
        }
        let builtin = builtin_family(family)?;
        Some(self.builtin(builtin, bold, italic))
    }

    /// Splits `text`, written in `spec`, into runs by the family that draws them when the text
    /// needs more than its own font: `(from, to, family)` with byte offsets into `text`.
    /// `None` when the text is drawn in `spec`'s face as usual (no font set, no fallback, or
    /// nothing the face lacks that another font has).
    ///
    /// This is how JasperReports resolves a font set (`JRStyledTextUtil.resolveFonts`): from
    /// each position, the longest run of characters that one family has, in the first family
    /// (in search order) that has them all; a character no family has goes in the primary
    /// family. A bold or italic text looks at the faces of that style first, then at the other
    /// styles down to the regular faces.
    pub fn fallback_runs(&mut self, spec: &FontSpec, text: &str) -> Option<Vec<(usize, usize, String)>> {
        if !self.resolves_fonts() {
            return None;
        }
        // Each family with its faces, in search order, by style: [regular, bold, italic, both].
        let mut lists: [Vec<(usize, FaceId)>; 4] = Default::default();
        let families: Vec<String>;
        if let Some(set) = self.sets.get(&spec.name.trim().to_ascii_lowercase()) {
            families = set.clone();
            for (k, f) in families.iter().enumerate() {
                for (s, (b, i)) in [(false, false), (true, false), (false, true), (true, true)].into_iter().enumerate() {
                    if let Some(id) = self.exact_face(f, b, i) {
                        lists[s].push((k, id));
                    }
                }
            }
        } else if self.fallback {
            let primary = self.resolve(spec);
            if text.chars().all(|c| always_displayable(c) || self.faces[primary].has_glyph(c)) {
                return None;
            }
            let mut names = vec![spec.name.clone()];
            names.extend(self.user_order.iter().cloned());
            names.extend(["DejaVu Sans", "DejaVu Serif", "DejaVu Sans Mono"].map(String::from));
            families = names;
            // The text's own face comes first in every style, as it is drawn without fallback.
            for list in lists.iter_mut() {
                list.push((0, primary));
            }
            for (k, f) in families.iter().enumerate().skip(1) {
                for (s, (b, i)) in [(false, false), (true, false), (false, true), (true, true)].into_iter().enumerate() {
                    if let Some(id) = self.exact_face(f, b, i) {
                        if !lists[s].iter().any(|&(_, seen)| seen == id) {
                            lists[s].push((k, id));
                        }
                    }
                }
            }
        } else {
            return None;
        }
        let order: &[usize] = match (spec.bold, spec.italic) {
            (true, true) => &[3, 1, 2, 0],
            (true, false) => &[1, 0],
            (false, true) => &[2, 0],
            (false, false) => &[0],
        };
        let supports = |fonts: &FontSet, id: FaceId, c: char| always_displayable(c) || fonts.faces[id].has_glyph(c);
        // The longest run from `from` that one face of `list` has, and that face's family.
        let match_run = |fonts: &FontSet, list: &[(usize, FaceId)], from: usize| -> Option<(usize, usize)> {
            let mut valid: Vec<(usize, FaceId)> = list.to_vec();
            let mut last = None;
            let mut end = from;
            for c in text[from..].chars() {
                valid.retain(|&(_, id)| supports(fonts, id, c));
                let Some(&(family, _)) = valid.first() else { break };
                last = Some(family);
                end += c.len_utf8();
            }
            last.map(|family| (end, family))
        };
        let mut runs: Vec<(usize, usize, usize)> = Vec::new();
        let push = |runs: &mut Vec<(usize, usize, usize)>, from: usize, to: usize, family: usize| match runs.last_mut() {
            Some(last) if last.2 == family && last.1 == from => last.1 = to,
            _ => runs.push((from, to, family)),
        };
        let mut at = 0;
        while at < text.len() {
            match order.iter().find_map(|&s| match_run(self, &lists[s], at)) {
                Some((end, family)) => {
                    push(&mut runs, at, end, family);
                    at = end;
                }
                None => {
                    // No family has it: the primary family draws it (as a missing glyph).
                    let end = at + text[at..].chars().next().map_or(1, char::len_utf8);
                    push(&mut runs, at, end, 0);
                    at = end;
                }
            }
        }
        if runs.is_empty() {
            runs.push((0, 0, 0));
        }
        if self.fallback && !self.sets.contains_key(&spec.name.trim().to_ascii_lowercase()) && runs.iter().all(|r| r.2 == 0) {
            return None;
        }
        Some(runs.into_iter().map(|(a, b, k)| (a, b, families[k].clone())).collect())
    }

    /// Forgets the paragraph settings of the reports filled before (a new render starts).
    pub(crate) fn clear_paragraphs(&mut self) {
        self.paragraphs.clear();
    }

    /// Takes the paragraph settings a report keeps in its hidden parameter
    /// ([`crate::text::PARAGRAPHS`]).
    pub(crate) fn add_paragraphs(&mut self, value: Option<&crate::value::Value>) {
        let Some(crate::value::Value::Json(table)) = value else { return };
        let Some(table) = table.as_object() else { return };
        for (key, spec) in table {
            if let (Ok(key), Some(p)) = (u64::from_str_radix(key, 16), crate::paragraph::Paragraph::from_json(spec)) {
                self.paragraphs.insert(key, Arc::new(p));
            }
        }
    }

    /// The paragraph settings of a text element, when it has any.
    pub(crate) fn paragraph(&self, te: &crate::model::TextElement) -> Option<Arc<crate::paragraph::Paragraph>> {
        if self.paragraphs.is_empty() {
            return None;
        }
        self.paragraphs.get(&crate::paragraph::fingerprint(te)).cloned()
    }

    /// `to`, made from `from` while filling (the rest of a text continuing on the next page),
    /// has its paragraph settings.
    pub(crate) fn carry_paragraph(&mut self, from: &crate::model::TextElement, to: &crate::model::TextElement) {
        if let Some(p) = self.paragraph(from) {
            self.paragraphs.insert(crate::paragraph::fingerprint(to), p);
        }
    }

    /// The face whose font draws `id`: itself, or the face a simulated style is drawn from.
    pub(crate) fn drawn_from(&self, id: FaceId) -> FaceId {
        self.simulated_from.get(&id).copied().unwrap_or(id)
    }

    pub fn face(&self, id: FaceId) -> &FaceData {
        &self.faces[id]
    }

    pub fn faces(&self) -> &[Arc<FaceData>] {
        &self.faces
    }

    fn builtin(&mut self, family: &str, bold: bool, italic: bool) -> FaceId {
        let key = (family.to_ascii_lowercase(), bold, italic);
        if let Some(&id) = self.by_key.get(&key) {
            return id;
        }
        let idx = BUILTIN.iter().position(|(f, b, i, _)| *f == family && *b == bold && *i == italic).unwrap_or(0);
        let id = self.faces.len();
        self.faces.push(builtin_face(idx).clone());
        self.by_key.insert(key, id);
        id
    }

    /// The PDF standard font JasperReports writes text of `spec` in, if any. JasperReports
    /// draws a family it has no font extension for (anything but DejaVu, unless registered)
    /// in the font its `pdfFontName` names, Helvetica by default, not embedded, while laying
    /// it out with the Java runtime's font (DejaVu on Linux). Text with a character that the
    /// standard fonts lack, or a `pdfFontName` that is a font file, stays in the embedded face.
    pub fn standard(&mut self, spec: &FontSpec, text: &str) -> Option<crate::standard::Standard> {
        let id = self.resolve(spec);
        if self.faces[id].family.eq_ignore_ascii_case(spec.name.trim()) {
            return None;
        }
        let font = crate::standard::pick(spec.pdf_font.as_deref().unwrap_or("Helvetica"), spec.bold, spec.italic)?;
        text.chars().all(|c| c.is_control() || crate::standard::encode(c).is_some()).then_some(font)
    }

    /// Picks the face used for `spec`, substituting a bundled family when needed.
    pub fn resolve(&mut self, spec: &FontSpec) -> FaceId {
        let key = (spec.name.to_ascii_lowercase(), spec.bold, spec.italic);
        if let Some(&id) = self.resolved.get(&key) {
            return id;
        }
        let user_regular =
            if self.user_families.contains(&key.0) { self.by_key.get(&(key.0.clone(), false, false)).copied() } else { None };
        let id = if let Some(&id) = self.by_key.get(&key) {
            id
        } else if let Some(id) = user_regular {
            // A registered family without the face asked for: its regular face, in the style
            // asked for simulated, as JasperReports draws a font extension's family.
            if spec.bold || spec.italic {
                let face = self.faces[id].simulating(spec.bold, spec.italic);
                let simulated = self.faces.len();
                self.faces.push(Arc::new(face));
                self.simulated_from.insert(simulated, id);
                simulated
            } else {
                id
            }
        } else {
            let family = match builtin_family(&spec.name) {
                Some(f) => f,
                None => {
                    if !self.substitutions.iter().any(|(from, _)| from == &spec.name) {
                        self.substitutions.push((spec.name.clone(), "DejaVu Sans".into()));
                    }
                    "DejaVu Sans"
                }
            };
            self.builtin(family, spec.bold, spec.italic)
        };
        self.resolved.insert(key, id);
        id
    }
}

/// The file a JasperReports extension is declared in, at the root of the class path.
pub(crate) const EXTENSION_PROPERTIES: &str = "jasperreports_extension.properties";

/// The properties of a `jasperreports_extension.properties` that name font extension files.
const FONT_FAMILIES_PROPERTY: &str = "net.sf.jasperreports.extension.simple.font.families.";

/// Registers the font families and font sets of the JasperReports font extensions that
/// `properties` (a `jasperreports_extension.properties`) declares, the way JasperReports finds
/// them on its class path. `read` reads a file by its class path location (the `fonts.xml`
/// files and the fonts they name). Families come first, then the sets that use them, so a set
/// may name a family of another extension. Returns what could not be registered.
pub(crate) fn load_extensions(
    fonts: &mut FontSet,
    properties: &[Vec<u8>],
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for bytes in properties {
        let mut entries: Vec<(String, String)> = crate::format::message::parse_properties(bytes)
            .into_iter()
            .filter(|(k, _)| k.starts_with(FONT_FAMILIES_PROPERTY))
            .collect();
        entries.sort();
        for (_, file) in entries {
            let file = file.trim().trim_start_matches('/').to_string();
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    let mut sets = Vec::new();
    for file in &files {
        let Some(bytes) = read(file) else {
            problems.push(format!("font extension `{file}` not found"));
            continue;
        };
        let root = match std::str::from_utf8(&bytes)
            .map_err(|e| e.to_string())
            .and_then(|s| crate::xml::parse(s).map_err(|e| e.to_string()))
        {
            Ok(root) => root,
            Err(e) => {
                problems.push(format!("font extension `{file}`: {e}"));
                continue;
            }
        };
        for family in root.children_named("fontFamily") {
            let Some(name) = family.attr("name").map(str::trim).filter(|n| !n.is_empty()) else { continue };
            for (tag, bold, italic) in
                [("normal", false, false), ("bold", true, false), ("italic", false, true), ("boldItalic", true, true)]
            {
                let Some(face) = family.child(tag) else { continue };
                // `<normal>fonts/a.ttf</normal>`, or the TrueType one of several formats:
                // `<normal><ttf>fonts/a.ttf</ttf><eot>…</eot></normal>`.
                let location = face.child("ttf").map_or(face.text.trim(), |t| t.text.trim()).trim_start_matches('/');
                if location.is_empty() {
                    continue;
                }
                let registered = read(location)
                    .ok_or_else(|| "not found".to_string())
                    .and_then(|data| fonts.register(name, bold, italic, data).map_err(|e| e.to_string()));
                if let Err(e) = registered {
                    problems.push(format!("font family `{name}` ({tag}), `{location}`: {e}"));
                }
            }
        }
        for set in root.children_named("fontSet") {
            let Some(name) = set.attr("name").map(str::trim).filter(|n| !n.is_empty()) else { continue };
            // In the order written, which is the order JasperReports searches them in. (The
            // set's primary family, which draws what no family has, is taken to be the first.)
            let families: Vec<String> =
                set.children_named("family").filter_map(|f| Some(f.attr("familyName")?.trim().to_string())).collect();
            sets.push((name.to_string(), families));
        }
    }
    for (name, families) in sets {
        let families: Vec<&str> = families.iter().map(String::as_str).collect();
        if let Err(e) = fonts.register_set(&name, &families) {
            problems.push(e.to_string());
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dejavu_metrics_match_awt() {
        let mut fonts = FontSet::new();
        let spec = FontSpec {
            name: "DejaVu Sans".into(),
            size: 10.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            pdf_font: None,
        };
        let id = fonts.resolve(&spec);
        let f = fonts.face(id);
        // Values observed in PDFs produced by JasperReports 6.20.
        assert!((f.line_height(10.0) - 11.640625).abs() < 1e-4);
        assert!((f.ascent(10.0) - 9.282_227).abs() < 1e-4);
    }

    #[test]
    fn styles_resolve_after_regular_is_loaded() {
        let mut fonts = FontSet::new();
        let mut spec = FontSpec {
            name: "DejaVu Sans".into(),
            size: 10.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            pdf_font: None,
        };
        fonts.resolve(&spec);
        spec.bold = true;
        spec.italic = true;
        let id = fonts.resolve(&spec);
        assert!(fonts.face(id).bold && fonts.face(id).italic);
    }

    #[test]
    fn substitutes_unknown_families() {
        let mut fonts = FontSet::new();
        let spec = FontSpec {
            name: "Comic Sans".into(),
            size: 10.0,
            bold: true,
            italic: false,
            underline: false,
            strike: false,
            pdf_font: None,
        };
        let id = fonts.resolve(&spec);
        assert_eq!(fonts.face(id).family, "DejaVu Sans");
        assert!(fonts.face(id).bold);
        assert_eq!(fonts.substitutions.len(), 1);
    }

    fn spec(name: &str, bold: bool, italic: bool) -> FontSpec {
        FontSpec { name: name.into(), size: 10.0, bold, italic, underline: false, strike: false, pdf_font: None }
    }

    fn families(runs: Option<Vec<(usize, usize, String)>>, text: &str) -> Vec<(String, String)> {
        runs.unwrap_or_default().into_iter().map(|(a, b, f)| (text[a..b].to_string(), f)).collect()
    }

    #[test]
    fn missing_characters_are_measured_like_jasperreports() {
        let mut fonts = FontSet::new();
        let id = fonts.resolve(&spec("DejaVu Serif", false, false));
        let f = fonts.face(id);
        // Filling (AWT) counts the missing-glyph box; exporting (iText) gives it no width.
        assert!(f.fill_width("★", 10.0) > 5.0);
        assert_eq!(f.text_width("a★b", 10.0), f.text_width("ab", 10.0));
    }

    #[test]
    fn no_fallback_unless_asked() {
        let mut fonts = FontSet::new();
        assert!(fonts.fallback_runs(&spec("DejaVu Serif", false, false), "rating ★").is_none());
    }

    #[test]
    fn font_sets_resolve_like_jasperreports() {
        let mut fonts = FontSet::new();
        fonts.register_set("Serif Fallback", &["DejaVu Serif", "DejaVu Sans"]).unwrap();
        assert!(fonts.register_set("Broken", &["DejaVu Serif", "No Such Font"]).is_err());
        // The longest run one family has, in the first family that has it all: DejaVu Serif
        // lacks the star, DejaVu Sans the arrow; CJK is in neither and stays in the primary.
        let text = "arrow ⤴ then ★ star 漢 end";
        let runs = families(fonts.fallback_runs(&spec("serif fallback", false, false), text), text);
        let want = [("arrow ⤴ then ", "DejaVu Serif"), ("★ star ", "DejaVu Sans"), ("漢 end", "DejaVu Serif")];
        assert_eq!(runs, want.map(|(t, f)| (t.to_string(), f.to_string())));
        // A text the primary family has whole is one run in it.
        let runs = families(fonts.fallback_runs(&spec("Serif Fallback", true, false), "plain"), "plain");
        assert_eq!(runs, [("plain".to_string(), "DejaVu Serif".to_string())]);
    }

    #[test]
    fn font_extensions_register_families_and_sets() {
        let properties = b"net.sf.jasperreports.extension.registry.factory.x=net.sf.jasperreports.engine.fonts.SimpleFontExtensionsRegistryFactory\n\
            net.sf.jasperreports.extension.simple.font.families.x=fonts/fonts.xml\n\
            net.sf.jasperreports.extension.simple.font.families.y=/fonts/missing.xml\n";
        let xml = r#"<fontFamilies>
            <fontFamily name="Ext Mono"><normal>fonts/mono.ttf</normal><bold><ttf>fonts/mono-bold.ttf</ttf></bold></fontFamily>
            <fontFamily name="Ext Broken"><normal>fonts/nowhere.ttf</normal></fontFamily>
            <fontSet name="Ext Set"><family familyName="Ext Mono" primary="true"/><family familyName="DejaVu Sans"/></fontSet>
        </fontFamilies>"#;
        let read = |location: &str| -> Option<Vec<u8>> {
            match location {
                "fonts/fonts.xml" => Some(xml.as_bytes().to_vec()),
                "fonts/mono.ttf" => Some(builtin_files().nth(8).unwrap().to_vec()),
                "fonts/mono-bold.ttf" => Some(builtin_files().nth(9).unwrap().to_vec()),
                _ => None,
            }
        };
        let mut fonts = FontSet::new();
        let problems = load_extensions(&mut fonts, &[properties.to_vec()], &read);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("fonts/missing.xml")) && problems.iter().any(|p| p.contains("Ext Broken")));
        let regular = fonts.resolve(&spec("ext mono", false, false));
        let bold = fonts.resolve(&spec("Ext Mono", true, false));
        assert_eq!(fonts.face(regular).family, "Ext Mono");
        assert!(!fonts.face(regular).bold && fonts.face(bold).bold);
        // The set's first family draws the text it has.
        let text = "mono ★";
        let runs = families(fonts.fallback_runs(&spec("Ext Set", false, false), text), text);
        assert_eq!(runs.first().map(|r| r.1.as_str()), Some("Ext Mono"));
    }

    #[test]
    fn fallback_looks_at_registered_fonts_first() {
        let mut fonts = FontSet::new();
        fonts.set_fallback(true);
        // Nothing missing: drawn as before.
        assert!(fonts.fallback_runs(&spec("DejaVu Serif", false, false), "all there").is_none());
        // Nobody has it: drawn as before, as a missing glyph.
        assert!(fonts.fallback_runs(&spec("DejaVu Serif", false, false), "漢").is_none());
        let text = "rating ★★ ok";
        let runs = families(fonts.fallback_runs(&spec("DejaVu Serif", false, true), text), text);
        assert_eq!(runs, [(text.to_string(), "DejaVu Sans".to_string())]);
        // A registered family comes before the bundled ones.
        fonts.register("Mono Copy", false, false, builtin_files().nth(8).unwrap().to_vec()).unwrap();
        let runs = families(fonts.fallback_runs(&spec("DejaVu Serif", false, false), text), text);
        assert_eq!(runs, [(text.to_string(), "Mono Copy".to_string())]);
    }
}

//! PDF exporter.
//!
//! Fonts are embedded as subsetted CID fonts (Identity-H) with a ToUnicode map, so text stays
//! selectable and searchable. Content streams are Flate-compressed.
//!
//! With [`PdfOptions::tagged`] the document also says what its content is, for screen readers
//! and for text extraction: every text element is a paragraph and every image, chart and
//! barcode a figure in the document's structure tree, in reading order, and everything else
//! (lines, borders, backgrounds) is marked as decoration.

use crate::error::{Error, Result};
use crate::fonts::{FaceId, FontSet};
use crate::image::{ImageData, Pixels};
use crate::model::{Color, LineStyle, Pen};
use crate::print::{LinkInfo, LinkTarget, PathOp, PrintDocument, PrintItem};
use pdf_writer::types::{CidFontType, FontFlags, FunctionShadingType, LineCapStyle, SystemInfo};
use pdf_writer::{Chunk, Content, Filter, Finish, Name, Rect, Ref, Str, TextStr};
use std::collections::{BTreeMap, BTreeSet};
use subsetter::GlyphRemapper;

/// Document metadata written to the PDF info dictionary.
#[derive(Debug, Clone, Default)]
pub struct PdfOptions {
    pub title: Option<String>,
    pub compress: bool,
    /// Write a tagged PDF: a structure tree over the content (see the module documentation).
    pub tagged: bool,
    /// The language of the text (`en-US`, `es-DO`), for tagged PDFs.
    pub lang: Option<String>,
    /// Where pages are moved to, odd pages then even ones (x right, y down, in points).
    pub page_offsets: [[f32; 2]; 2],
    /// Write a PDF/A document (for archiving): fonts embedded, an sRGB output intent, the
    /// conformance declared in the metadata.
    pub pdfa: Option<PdfA>,
    /// Files carried inside the document (PDF/A-3 allows them; an e-invoice's XML, say).
    pub attachments: Vec<Attachment>,
    /// Encrypt the document: a password to open it, permissions an owner password lifts.
    pub encryption: Option<crate::crypt::Encryption>,
    /// Sign the document (PAdES B-B). The writer leaves room for the signature;
    /// [`crate::sign::apply`] puts it in once the whole file is written.
    pub signature: Option<crate::sign::Signature>,
    /// When it is signed (the signature's `/M`).
    pub signed_at: Option<chrono::NaiveDateTime>,
}

/// A PDF/A conformance: part 2 or 3, level A (tagged), B (basic) or U (text extractable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfA {
    pub part: u8,
    pub level: char,
}

impl PdfA {
    /// `2b`, `pdfa3u`, `PDF/A-2a`... (`1a` and `1b` are written as part 2, which keeps their
    /// rules and allows transparency).
    pub fn parse(s: &str) -> Option<PdfA> {
        let t: String = s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let t = t.strip_prefix("pdfa").unwrap_or(&t);
        let mut chars = t.chars();
        let part = chars.next()?.to_digit(10)? as u8;
        let level = chars.next()?.to_ascii_uppercase();
        if chars.next().is_some() || !matches!(level, 'A' | 'B' | 'U') {
            return None;
        }
        match part {
            1 | 2 => Some(PdfA { part: 2, level }),
            3 => Some(PdfA { part: 3, level }),
            _ => None,
        }
    }
}

/// A file carried inside a PDF.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub data: std::sync::Arc<Vec<u8>>,
    /// Its MIME type (`text/xml`).
    pub mime: String,
    pub description: Option<String>,
    /// How it relates to the document: `Data`, `Source`, `Alternative`, `Supplement` or
    /// `Unspecified` (PDF/A-3's `AFRelationship`).
    pub relationship: String,
}

/// The sRGB profile of PDF/A output intents (CC0, see assets/icc/README.md).
const SRGB_ICC: &[u8] = include_bytes!("../assets/icc/sRGB-v4.icc");

/// Compresses a stream. Level 3 is about twice as fast as the default level 6 and only
/// ~8% larger (still a quarter of the size of JasperReports' output).
#[cfg(not(target_arch = "wasm32"))]
fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 3 + 64), flate2::Compression::new(3));
    // Writing to a Vec cannot fail.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 3)
}

/// iText (and so JasperReports) writes coordinates with two decimals; rounding the same way
/// keeps glyphs on the same pixel grid when rasterized.
fn r2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn rgb(c: Color) -> (f32, f32, f32) {
    (c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0)
}

/// Sets stroke color, width and dash pattern. Lines and borders use JasperReports' line
/// patterns; rectangles and ellipses (`shape`) use its graphic-element patterns.
fn apply_pen(content: &mut Content, pen: &Pen, shape: bool) {
    let (r, g, b) = rgb(pen.color);
    content.set_stroke_rgb(r, g, b);
    content.set_line_width(pen.width);
    let w = pen.width.max(0.1);
    content.set_line_cap(LineCapStyle::ButtCap);
    match (pen.style, shape) {
        (LineStyle::Dashed, false) => content.set_dash_pattern([5.0 * w, 3.0 * w], 0.0),
        (LineStyle::Dotted, false) => content.set_dash_pattern([w, w], 0.0),
        (LineStyle::Dashed, true) => content.set_dash_pattern([4.0 * w, 4.0 * w], 0.0),
        (LineStyle::Dotted, true) => {
            content.set_line_cap(LineCapStyle::RoundCap);
            content.set_dash_pattern([0.0, 2.0 * w], 0.0)
        }
        _ => content.set_dash_pattern([], 0.0),
    };
}

/// Appends a rounded-rectangle path (PDF coordinates, y up).
fn rounded_rect(content: &mut Content, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * r;
    content.move_to(x + r, y);
    content.line_to(x + w - r, y);
    content.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    content.line_to(x + w, y + h - r);
    content.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    content.line_to(x + r, y + h);
    content.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    content.line_to(x, y + r);
    content.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    content.close_path();
}

/// A rectangle as iText's `roundRectangle` writes it with radius 0: lines with zero-length
/// curves at the corners, not closed.
fn itext_rect(content: &mut Content, x: f32, y: f32, w: f32, h: f32) {
    content.move_to(x, y);
    content.line_to(x + w, y);
    content.cubic_to(x + w, y, x + w, y, x + w, y);
    content.line_to(x + w, y + h);
    content.cubic_to(x + w, y + h, x + w, y + h, x + w, y + h);
    content.line_to(x, y + h);
    content.cubic_to(x, y + h, x, y + h, x, y + h);
    content.line_to(x, y);
    content.cubic_to(x, y, x, y, x, y);
}

fn ellipse(content: &mut Content, x: f32, y: f32, w: f32, h: f32) {
    let (rx, ry) = (w / 2.0, h / 2.0);
    let (cx, cy) = (x + rx, y + ry);
    let (kx, ky) = (0.552_284_8 * rx, 0.552_284_8 * ry);
    content.move_to(cx + rx, cy);
    content.cubic_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry);
    content.cubic_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy);
    content.cubic_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry);
    content.cubic_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy);
    content.close_path();
}

/// Six-letter subset tag derived from the glyph set, as required for subset font names.
fn subset_tag(face: FaceId, glyphs: &BTreeSet<u16>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ face as u64;
    for g in glyphs {
        h = (h ^ *g as u64).wrapping_mul(0x0100_0000_01b3);
    }
    (0..6).map(|i| (b'A' + ((h >> (i * 5)) % 26) as u8) as char).collect()
}

/// One drawing step: an item, or entering/leaving a transformed coordinate system.
enum Draw<'a> {
    Item(&'a PrintItem),
    /// A link, anchor or bookmark where an element is.
    Link(&'a LinkInfo),
    /// A text element written as a form field.
    Field(&'a crate::print::CellInfo),
    Push([f32; 6]),
    Pop,
    /// What follows, up to `End`, is one piece of content of the document (tagged PDFs).
    Begin(Mark),
    End,
    /// What follows, up to `Close`, is in a table, one of its cells or a list; where it is on
    /// the page (left, top, width, height).
    Open(Role, [f32; 4]),
    Close,
}

/// What a piece of a tagged PDF's structure is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Paragraph,
    Heading(u8),
    Figure,
    Table,
    Row,
    HeaderCell,
    RowHeaderCell,
    DataCell,
    /// A list, numbered or not; one of its items, the item's bullet or number, and the rest.
    List(bool),
    ListItem,
    ListLabel,
    ListBody,
    /// A link: it holds the link's annotation.
    Link,
    /// The page itself, while its structure is gathered.
    Group,
}

impl Role {
    fn of(tag: crate::model::Tag) -> Role {
        match tag {
            crate::model::Tag::Heading(level) => Role::Heading(level.clamp(1, 6)),
            crate::model::Tag::Table => Role::Table,
            crate::model::Tag::HeaderCell => Role::HeaderCell,
            crate::model::Tag::RowHeaderCell => Role::RowHeaderCell,
            crate::model::Tag::DataCell => Role::DataCell,
        }
    }

    /// The name of the role in a PDF.
    fn name(self) -> &'static str {
        match self {
            Role::Paragraph => "P",
            Role::Heading(1) => "H1",
            Role::Heading(2) => "H2",
            Role::Heading(3) => "H3",
            Role::Heading(4) => "H4",
            Role::Heading(5) => "H5",
            Role::Heading(_) => "H6",
            Role::Figure => "Figure",
            Role::Table => "Table",
            Role::Row => "TR",
            Role::HeaderCell | Role::RowHeaderCell => "TH",
            Role::DataCell => "TD",
            Role::List(_) => "L",
            Role::ListItem => "LI",
            Role::ListLabel => "Lbl",
            Role::ListBody => "LBody",
            Role::Link => "Link",
            Role::Group => "Div",
        }
    }

    fn is_cell(self) -> bool {
        matches!(self, Role::HeaderCell | Role::RowHeaderCell | Role::DataCell)
    }
}

/// A piece of content in the content stream of a tagged PDF: a text or a picture.
#[derive(Debug, Clone)]
struct Mark {
    role: Role,
    /// What a figure shows, for those who cannot see it.
    alt: Option<String>,
}

/// A node of the structure of one page: a piece of content (it has a number in the page's
/// content stream), or a table, a row or a cell holding others.
#[derive(Debug)]
struct Node {
    role: Role,
    alt: Option<String>,
    content: Option<usize>,
    /// Top of a cell on the page: cells at the same height are a row.
    y: f32,
    /// Left, width and height of a cell, for the columns and rows it spans.
    x: f32,
    w: f32,
    h: f32,
    /// How many columns and rows of its table a cell spans.
    span: (u16, u16),
    /// For a link, its annotation among those of the page.
    annot: Option<usize>,
    kids: Vec<Node>,
}

impl Node {
    fn new(role: Role, [x, y, w, h]: [f32; 4], kids: Vec<Node>) -> Node {
        Node { role, alt: None, content: None, y, x, w, h, span: (1, 1), annot: None, kids }
    }
}

/// How many columns and rows each cell of a table spans: the columns are where its cells
/// start, the rows where its rows start (a header over two columns spans both).
fn spans(rows: &mut [Node]) {
    let mut xs: Vec<f32> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for row in rows.iter() {
        if row.y.is_finite() {
            ys.push(row.y);
        }
        xs.extend(row.kids.iter().map(|c| c.x).filter(|x| x.is_finite()));
    }
    let distinct = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    };
    distinct(&mut xs);
    distinct(&mut ys);
    let count = |v: &[f32], from: f32, len: f32| v.iter().filter(|&&p| p > from - 0.5 && p < from + len - 0.5).count();
    for row in rows.iter_mut() {
        for cell in row.kids.iter_mut().filter(|c| c.x.is_finite() && c.y.is_finite()) {
            cell.span = (count(&xs, cell.x, cell.w).clamp(1, 1000) as u16, count(&ys, cell.y, cell.h).clamp(1, 1000) as u16);
        }
    }
    // Every row accounts for every column: where no cell covers one (nothing printed
    // there), an empty cell stands in, as PDF/UA asks.
    if xs.is_empty() || rows.iter().any(|r| !r.y.is_finite() || r.kids.iter().any(|c| !c.x.is_finite())) {
        return;
    }
    let at = |v: &[f32], p: f32| v.iter().position(|q| (q - p).abs() < 0.5);
    let mut taken = vec![vec![false; xs.len()]; ys.len()];
    for row in rows.iter() {
        let Some(r0) = at(&ys, row.y) else { return };
        for cell in &row.kids {
            let Some(c0) = at(&xs, cell.x) else { return };
            for line in taken.iter_mut().skip(r0).take(cell.span.1 as usize) {
                for slot in line.iter_mut().skip(c0).take(cell.span.0 as usize) {
                    *slot = true;
                }
            }
        }
    }
    for row in rows.iter_mut() {
        let Some(r0) = at(&ys, row.y) else { continue };
        let missing: Vec<usize> = (0..xs.len()).filter(|&c| !taken[r0][c]).collect();
        if missing.is_empty() {
            continue;
        }
        for c in missing {
            row.kids.push(Node::new(Role::DataCell, [xs[c], row.y, 0.0, 0.0], Vec::new()));
        }
        row.kids.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
}

/// The cells of a table gathered into rows (cells at the same height), in order. Anything
/// in a table that is not in a cell gets a row and a cell of its own.
fn rows(kids: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for kid in kids {
        let cell = if kid.role.is_cell() {
            kid
        } else if kid.role == Role::Row {
            out.push(kid);
            continue;
        } else {
            Node::new(Role::DataCell, [f32::NAN; 4], vec![kid])
        };
        match out.last_mut() {
            Some(row) if row.role == Role::Row && (row.y - cell.y).abs() < 0.5 => row.kids.push(cell),
            _ => out.push(Node::new(Role::Row, [f32::NAN, cell.y, f32::NAN, f32::NAN], vec![cell])),
        }
    }
    spans(&mut out);
    out
}

/// Cells that are not in a table become one: a table that goes on to another page is
/// drawn there as its cells alone, and so are cells tagged by hand without a table.
fn adopt(kids: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    let mut cells: Vec<Node> = Vec::new();
    let table = |cells: &mut Vec<Node>, out: &mut Vec<Node>| {
        if !cells.is_empty() {
            let y = cells[0].y;
            out.push(Node::new(Role::Table, [f32::NAN, y, f32::NAN, f32::NAN], rows(std::mem::take(cells))));
        }
    };
    for kid in kids {
        if kid.role.is_cell() {
            cells.push(kid);
        } else {
            table(&mut cells, &mut out);
            out.push(kid);
        }
    }
    table(&mut cells, &mut out);
    out
}

/// Items in drawing order with groups expanded. With `tagged`, text elements and pictures
/// are bracketed by `Begin` and `End`, and tables and their cells by `Open` and `Close`.
fn flatten<'a>(items: &'a [PrintItem], tagged: bool, out: &mut Vec<Draw<'a>>) {
    use crate::print::Shape;
    // Where the elements being drawn end (the items after an element's mark are its own),
    // and whether each is a picture: what a picture is made of is not content of its own.
    let mut open: Vec<(usize, bool)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let tagging = tagged && !open.iter().any(|&(_, picture)| picture);
        match item {
            PrintItem::Group(inner) => flatten(inner, tagging, out),
            PrintItem::Link(info) => out.push(Draw::Link(info)),
            PrintItem::Cell(info, _) if info.field.is_some() => out.push(Draw::Field(info)),
            PrintItem::Cell(info, inner) if tagging && matches!(**inner, PrintItem::Paragraphs(..)) => {
                if let PrintItem::Paragraphs(kinds, paras) = &**inner {
                    lists(kinds, paras, info, out);
                }
            }
            PrintItem::Paragraphs(_, inner) => flatten(inner, false, out),
            PrintItem::Cell(info, inner) => {
                let mark = tagging && !info.text.trim().is_empty();
                if mark {
                    let role = match info.tag {
                        Some(crate::model::Tag::Heading(level)) => Role::Heading(level.clamp(1, 6)),
                        _ => Role::Paragraph,
                    };
                    out.push(Draw::Begin(Mark { role, alt: None }));
                }
                flatten(std::slice::from_ref(inner), false, out);
                if mark {
                    out.push(Draw::End);
                }
            }
            PrintItem::Transform(m, inner) => {
                out.push(Draw::Push(*m));
                flatten(inner, tagging, out);
                out.push(Draw::Pop);
            }
            PrintItem::Element(info) if tagging && info.len > 0 => {
                let picture = match &info.shape {
                    Shape::Image { data: Some(_), .. } => Some("Image"),
                    Shape::Picture { symbol: Some(_) } => Some("Barcode"),
                    Shape::Picture { symbol: None } => Some("Chart"),
                    _ => None,
                };
                if let Some(what) = picture {
                    let alt = info.alt.clone().unwrap_or_else(|| what.to_string());
                    out.push(Draw::Begin(Mark { role: Role::Figure, alt: Some(alt) }));
                    open.push((i + info.len, true));
                } else if let Some(tag) = info.tag.filter(|t| !matches!(t, crate::model::Tag::Heading(_))) {
                    out.push(Draw::Open(Role::of(tag), [info.x, info.y, info.w, info.h]));
                    open.push((i + info.len, false));
                }
            }
            other => out.push(Draw::Item(other)),
        }
        while open.last().is_some_and(|&(end, _)| end <= i) {
            let (_, picture) = open.pop().unwrap_or((0, false));
            out.push(if picture { Draw::End } else { Draw::Close });
        }
    }
    for (_, picture) in open.into_iter().rev() {
        out.push(if picture { Draw::End } else { Draw::Close });
    }
}

/// The paragraphs of a text with HTML lists as a tagged PDF has them: lists of items, each
/// with its bullet or number and its body, nested as deep as the text's lists are.
fn lists<'a>(kinds: &[crate::print::ParaKind], paras: &'a [PrintItem], info: &crate::print::CellInfo, out: &mut Vec<Draw<'a>>) {
    let role = match info.tag {
        Some(crate::model::Tag::Heading(level)) => Role::Heading(level.clamp(1, 6)),
        _ => Role::Paragraph,
    };
    let y = [info.x, info.y, info.w, info.h];
    let content = |items: &'a [PrintItem], role: Role, out: &mut Vec<Draw<'a>>| {
        if items.is_empty() {
            return;
        }
        out.push(Draw::Begin(Mark { role, alt: None }));
        flatten(items, false, out);
        out.push(Draw::End);
    };
    // Each open level has a list, an item and the item's body open.
    let mut depth = 0u8;
    let close = |out: &mut Vec<Draw<'a>>, n: usize| out.extend((0..n).map(|_| Draw::Close));
    for (kind, para) in kinds.iter().zip(paras) {
        let parts: &'a [PrintItem] = match para {
            PrintItem::Group(v) => v,
            other => std::slice::from_ref(other),
        };
        if !kind.item {
            while depth > kind.level {
                close(out, 3);
                depth -= 1;
            }
            content(parts, if depth > 0 { Role::Paragraph } else { role }, out);
            continue;
        }
        let level = kind.level.max(1);
        while depth > level {
            close(out, 3);
            depth -= 1;
        }
        // The item's bullet or number is drawn first.
        let (label, body) = parts.split_first().map(|(l, b)| (std::slice::from_ref(l), b)).unwrap_or((&[], &[]));
        let ordered = match label.first() {
            Some(PrintItem::Text(t)) => t.lines.first().is_some_and(|l| l.text.starts_with(|c: char| c.is_ascii_digit())),
            _ => false,
        };
        if depth == level {
            close(out, 2);
        } else {
            while depth < level {
                out.push(Draw::Open(Role::List(ordered), y));
                depth += 1;
                if depth < level {
                    out.push(Draw::Open(Role::ListItem, y));
                    out.push(Draw::Open(Role::ListBody, y));
                }
            }
        }
        out.push(Draw::Open(Role::ListItem, y));
        content(label, Role::ListLabel, out);
        out.push(Draw::Open(Role::ListBody, y));
        content(body, Role::Paragraph, out);
    }
    close(out, 3 * depth as usize);
}

/// What a document has used of one font face so far.
struct FaceUse {
    remapper: GlyphRemapper,
    /// Glyph of the face -> its number in the subset plus one (0: not used yet).
    codes: Vec<u16>,
    /// The glyphs used and a character each stands for, in order of first use.
    chars: Vec<(u16, char)>,
    /// Glyphs of shaped text that stand for something other than one character (ligatures,
    /// conjuncts), and what they stand for.
    strings: Vec<(u16, String)>,
    resource: String,
}

impl FaceUse {
    fn new(number: usize) -> FaceUse {
        FaceUse {
            remapper: GlyphRemapper::new(),
            codes: Vec::new(),
            chars: Vec::new(),
            strings: Vec::new(),
            resource: format!("F{number}"),
        }
    }

    /// The code that draws `glyph` in the subset font, and whether it is new.
    #[inline]
    fn remap(&mut self, glyph: u16) -> (u16, bool) {
        let i = glyph as usize;
        if i >= self.codes.len() {
            self.codes.resize(i + 1, 0);
        }
        let new = self.codes[i] == 0;
        if new {
            self.codes[i] = self.remapper.remap(glyph).wrapping_add(1);
        }
        (self.codes[i].wrapping_sub(1), new)
    }

    /// The code that draws `glyph` in the subset font.
    fn code(&mut self, glyph: u16, ch: char) -> u16 {
        let (code, new) = self.remap(glyph);
        if new {
            self.chars.push((glyph, ch));
        }
        code
    }

    /// The code that draws `glyph`, a glyph of shaped text standing for `text`.
    fn code_for(&mut self, glyph: u16, text: &str) -> u16 {
        let (code, new) = self.remap(glyph);
        if new {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => self.chars.push((glyph, c)),
                _ => self.strings.push((glyph, text.to_string())),
            }
        }
        code
    }
}

/// Writes one shaped line: its glyphs in visual order, each placed where shaping put it
/// (the PDF advances by the font's own widths, adjusted to the shaped advances; marks raised
/// with the text rise).
///
/// Copying gives the characters as written: a glyph maps to the characters of its cluster.
/// Left-to-right runs whose glyphs are not one per character in reading order (Devanagari
/// vowel signs drawn before their consonant, conjuncts, Thai marks) are also wrapped in a span
/// whose actual text is the run as written. Right-to-left runs are left to the reader's own
/// reordering, as for any PDF of right-to-left text, so a ligature there maps to its
/// characters from left to right as drawn (lam-alef: alef, lam), which readers reverse.
fn show_shaped(
    c: &mut Content,
    used: &mut FaceUse,
    face: &crate::fonts::FaceData,
    size: f32,
    line: &crate::print::PrintLine,
    shaped: &crate::shape::ShapedLine,
    ph: f32,
) {
    enum Op {
        Show(Vec<u8>),
        Adjust(f32),
    }
    fn flush(c: &mut Content, ops: &mut Vec<Op>) {
        if ops.is_empty() {
            return;
        }
        let mut positioned = c.show_positioned();
        let mut items = positioned.items();
        for op in ops.drain(..) {
            match op {
                Op::Show(bytes) => {
                    items.show(Str(&bytes));
                }
                Op::Adjust(a) => {
                    items.adjust(a);
                }
            }
        }
    }
    let k = size as f64 / 1000.0;
    let millis = 1000.0 / face.units_per_em;
    let clusters: Vec<(&str, &[crate::shape::Glyph])> = shaped.clusters(&line.text).collect();
    let x0 = r2(line.x) as f64;
    let skew = if face.simulated.italic { crate::fonts::SIMULATED_ITALIC_SKEW } else { 0.0 };
    c.set_text_matrix([1.0, 0.0, skew, 1.0, x0 as f32, r2(ph - line.baseline)]);
    let (mut pen, mut cur, mut rise) = (x0, x0, 0.0f32);
    let mut ops: Vec<Op> = Vec::new();
    // Runs of clusters of one direction.
    for run in clusters.chunk_by(|a, b| a.1[0].rtl == b.1[0].rtl) {
        let rtl = run[0].1[0].rtl;
        let reordered = !rtl && run.iter().any(|(t, gs)| gs.len() > 1 || t.chars().nth(1).is_some());
        if reordered {
            flush(c, &mut ops);
            let actual: String = run.iter().map(|(t, _)| *t).collect();
            c.begin_marked_content_with_properties(Name(b"Span")).properties().actual_text(TextStr(&actual));
        }
        for &(text, glyphs) in run {
            let mut left: Vec<char> = text.chars().collect();
            let mut meaning: Vec<Option<String>> = glyphs
                .iter()
                .map(|g| {
                    if glyphs.len() == 1 {
                        return Some(if rtl { text.chars().rev().collect() } else { text.to_string() });
                    }
                    let i = left.iter().position(|&ch| face.glyph(ch) == g.id)?;
                    Some(left.remove(i).to_string())
                })
                .collect();
            for m in meaning.iter_mut().filter(|m| m.is_none()) {
                *m = Some(if left.is_empty() { text.to_string() } else { left.drain(..).collect() });
            }
            for (g, m) in glyphs.iter().zip(meaning) {
                let x = pen + g.dx as f64 * k;
                pen += g.advance as f64 * k;
                if text == " " {
                    pen += line.word_spacing as f64;
                }
                let code = used.code_for(g.id, m.as_deref().unwrap_or(text));
                let dy = r2((g.dy as f64 * k) as f32);
                if dy != rise {
                    flush(c, &mut ops);
                    c.set_rise(dy);
                    rise = dy;
                }
                let adjust = r2(((cur - x) * 1000.0 / size as f64) as f32);
                if adjust != 0.0 {
                    ops.push(Op::Adjust(adjust));
                    cur -= adjust as f64 * size as f64 / 1000.0;
                }
                match ops.last_mut() {
                    Some(Op::Show(bytes)) => bytes.extend_from_slice(&code.to_be_bytes()),
                    _ => ops.push(Op::Show(code.to_be_bytes().to_vec())),
                }
                cur += (face.advance(g.id) * millis).trunc() as f64 * size as f64 / 1000.0;
            }
        }
        if reordered {
            flush(c, &mut ops);
            c.end_marked_content();
        }
    }
    flush(c, &mut ops);
    if rise != 0.0 {
        c.set_rise(0.0);
    }
}

/// A text element written as a PDF form field.
struct Widget {
    rect: Rect,
    field: std::sync::Arc<crate::model::FormField>,
    value: String,
    size: f32,
    color: Color,
    back: Option<Color>,
    border: Option<Color>,
}

impl Widget {
    fn of(info: &crate::print::CellInfo, ph: f32, offset: [f32; 2]) -> Widget {
        let (x, y) = (info.x + offset[0], ph - info.y - info.h - offset[1]);
        let pen = info.boxed.top;
        Widget {
            rect: Rect::new(x, y, x + info.w, y + info.h),
            field: info.field.clone().unwrap_or_else(|| unreachable!("a field cell")),
            value: info.text.clone(),
            size: info.font.size,
            color: info.color,
            back: info.back,
            border: (pen.width > 0.0).then_some(pen.color),
        }
    }
}

/// Draws text in a standard font the way JasperReports does: its lines broken where the
/// face put them, each line aligned with the standard font's own widths.
fn draw_standard(c: &mut Content, t: &crate::print::PrintText, style: crate::standard::Standard, ph: f32) {
    use crate::model::HAlign;
    let font = style.font;
    let (r, g, b) = rgb(t.color);
    let resource = format!("S{font}");
    let mut placed = Vec::with_capacity(t.lines.len());
    // iText's simulated italic slants by this much.
    let skew = if style.italic { 0.21256 } else { 0.0 };
    c.set_fill_rgb(r, g, b);
    c.begin_text();
    c.set_font(Name(resource.as_bytes()), t.size);
    if style.bold {
        // iText's simulated bold: the outline stroked as well, a thirtieth of the size wide.
        c.set_text_rendering_mode(pdf_writer::types::TextRenderingMode::FillStroke);
        c.set_line_width(t.size / 30.0);
        c.set_stroke_rgb(r, g, b);
    }
    for line in &t.lines {
        if line.text.is_empty() {
            continue;
        }
        let natural = crate::standard::width(font, &line.text, t.size);
        let spaces = line.text.matches(' ').count() as f32;
        let (x, spacing, width) = if line.word_spacing.abs() > 1e-4 && spaces > 0.0 {
            // Justified: the words spread over the same width.
            (line.x, (line.width - natural) / spaces, line.width)
        } else {
            let x = match t.halign {
                HAlign::Center => line.x + (line.width - natural) / 2.0,
                HAlign::Right => line.x + line.width - natural,
                _ => line.x,
            };
            (x, 0.0, natural)
        };
        let bytes: Vec<u8> = line.text.chars().filter_map(crate::standard::encode).collect();
        if spacing != 0.0 {
            c.set_word_spacing(r2(spacing));
        }
        c.set_text_matrix([1.0, 0.0, skew, 1.0, r2(x), r2(ph - line.baseline)]);
        c.show(Str(&bytes));
        if spacing != 0.0 {
            c.set_word_spacing(0.0);
        }
        placed.push((x, line.baseline, width));
    }
    if style.bold {
        c.set_text_rendering_mode(pdf_writer::types::TextRenderingMode::Fill);
    }
    c.end_text();
    if t.underline || t.strike {
        let thick = t.size / 18.0;
        for (x, baseline, width) in placed {
            for (on, offset) in [(t.underline, -t.size / 12.0), (t.strike, t.size / 3.0)] {
                if on && width > 0.0 {
                    let y = ph - baseline + offset;
                    c.set_stroke_rgb(r, g, b);
                    c.set_line_width(thick);
                    c.set_dash_pattern([], 0.0);
                    c.move_to(x, y);
                    c.line_to(x + width, y);
                    c.stroke();
                }
            }
        }
    }
}

/// Writes a PDF page by page: each page's content goes to `out` as soon as it arrives, and
/// fonts (subsetted to the glyphs used), the page tree and the cross-reference table follow
/// at the end. Memory use does not grow with the number of pages.
pub struct PdfStream<W: std::io::Write> {
    out: W,
    pos: u64,
    /// Byte offset of each object, by object number minus one.
    offsets: Vec<u64>,
    options: PdfOptions,
    page_width: f32,
    page_height: f32,
    faces: BTreeMap<FaceId, FaceUse>,
    /// The standard fonts used (index into `standard::STANDARD`).
    standard: std::collections::BTreeSet<u8>,
    images: BTreeMap<usize, (String, Ref)>,
    /// The content stream of each page; pages drawn on again later ("page X of Y") have more,
    /// in `more_contents`. Kept this small because it is the one thing kept for every page.
    pages: Vec<Ref>,
    more_contents: BTreeMap<usize, Vec<Ref>>,
    /// The form fields of the pages that have some, and those of the page being drawn.
    fields: BTreeMap<usize, Vec<Widget>>,
    pending_fields: Vec<Widget>,
    /// The links of the pages that have some, and those of the page being drawn.
    links: BTreeMap<usize, Vec<PageLink>>,
    pending_links: Vec<PageLink>,
    /// Anchors (named destinations): name, page and top, in the order they are met.
    dests: Vec<(String, usize, f32)>,
    /// Bookmarks: level, title, page and top.
    bookmarks: Vec<(u32, String, usize, f32)>,
    /// In a tagged PDF, the parent tree key of each link annotation and its structure element.
    link_parents: Vec<(i32, Ref)>,
    tags: Option<Tags>,
    /// Encrypts every object as it is written.
    crypt: Option<crate::crypt::Crypt>,
}

/// Named destinations: name and object.
type NamedDests = Vec<(String, Ref)>;

/// Annotations of the pages that have some, by page number.
type PageAnnots = BTreeMap<usize, Vec<Ref>>;

/// Parent tree keys of link annotations start here, after any page's.
const LINK_KEYS: i32 = 1 << 24;

/// A link annotation on a page.
struct PageLink {
    rect: Rect,
    target: LinkTarget,
    /// What the link is, in words (its tooltip, or where it leads).
    contents: String,
    /// Its object and parent tree key, given when it is tagged.
    id: Option<Ref>,
    key: Option<i32>,
}

/// The structure tree of a tagged PDF, as far as it has been written.
struct Tags {
    root: Ref,
    document: Ref,
    /// Per page: its object, its section in the tree, and the array that leads from its
    /// marked content back to the tree (none for a page without content).
    pages: Vec<(Ref, Option<(Ref, Ref)>)>,
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

impl<W: std::io::Write> PdfStream<W> {
    pub fn new(mut out: W, page_width: f32, page_height: f32, options: PdfOptions) -> Result<PdfStream<W>> {
        let header = b"%PDF-1.7\n%\x80\x80\x80\x80\n\n";
        out.write_all(header).map_err(io)?;
        // Objects 1 to 3 are the catalog, the page tree and the document information.
        let mut pdf = PdfStream {
            out,
            pos: header.len() as u64,
            offsets: vec![0; 3],
            options,
            page_width,
            page_height,
            faces: BTreeMap::new(),
            standard: Default::default(),
            fields: BTreeMap::new(),
            pending_fields: Vec::new(),
            links: BTreeMap::new(),
            pending_links: Vec::new(),
            dests: Vec::new(),
            bookmarks: Vec::new(),
            link_parents: Vec::new(),
            images: BTreeMap::new(),
            pages: Vec::new(),
            more_contents: BTreeMap::new(),
            tags: None,
            crypt: None,
        };
        if let Some(a) = pdf.options.pdfa {
            // Level A is about the structure: the document is tagged.
            if a.level == 'A' {
                pdf.options.tagged = true;
            }
            if a.part == 2 && !pdf.options.attachments.is_empty() {
                return Err(Error::Unsupported(
                    "PDF/A-2 allows only PDF/A documents as attachments: ask for PDF/A-3 (3b) to attach other files".into(),
                ));
            }
        }
        if let Some(e) = &pdf.options.encryption {
            if pdf.options.pdfa.is_some() {
                return Err(Error::Unsupported("PDF/A does not allow encryption: ask for one or the other".into()));
            }
            let mut e = e.clone();
            // Screen readers may read a tagged document (PDF/UA asks for it).
            if pdf.options.tagged {
                e.permissions.0 |= crate::crypt::Permissions::SCREENREADERS;
            }
            pdf.crypt = Some(crate::crypt::Crypt::new(&e)?);
        }
        if pdf.options.tagged {
            let (root, document) = (pdf.next_id(), pdf.next_id());
            pdf.tags = Some(Tags { root, document, pages: Vec::new() });
        }
        Ok(pdf)
    }

    fn next_id(&mut self) -> Ref {
        self.offsets.push(0);
        Ref::new(self.offsets.len() as i32)
    }

    /// Writes a chunk holding the single object `id`.
    fn put(&mut self, id: Ref, chunk: &Chunk) -> Result<()> {
        self.offsets[id.get() as usize - 1] = self.pos;
        if let Some(crypt) = &mut self.crypt {
            let bytes = crypt.object(chunk.as_bytes());
            self.out.write_all(&bytes).map_err(io)?;
            self.pos += bytes.len() as u64;
            return Ok(());
        }
        self.out.write_all(chunk.as_bytes()).map_err(io)?;
        self.pos += chunk.len() as u64;
        Ok(())
    }

    /// An object written as it is: the signature's, which must not be encrypted.
    fn put_raw(&mut self, id: Ref, bytes: &[u8]) -> Result<()> {
        self.offsets[id.get() as usize - 1] = self.pos;
        self.out.write_all(bytes).map_err(io)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// The signature's value, with room for the signature and its byte range.
    fn write_signature(&mut self) -> Result<Option<(Ref, Ref)>> {
        let Some(sig) = self.options.signature.clone() else { return Ok(None) };
        if self.crypt.is_some() {
            return Err(Error::Unsupported("signing an encrypted PDF (sign it, or encrypt it, not both)".into()));
        }
        let (value, field) = (self.next_id(), self.next_id());
        let when = self.options.signed_at.unwrap_or_else(|| chrono::Utc::now().naive_utc());
        let mut text = format!(
            "{} 0 obj\n<<\n  /Type /Sig\n  /Filter /Adobe.PPKLite\n  /SubFilter /ETSI.CAdES.detached\n  {}\n  /Contents <{}>\n  /M (D:{}Z)\n",
            value.get(),
            crate::sign::BYTE_RANGE_PLACEHOLDER,
            "0".repeat(crate::sign::RESERVED * 2),
            when.format("%Y%m%d%H%M%S"),
        );
        for (key, v) in [("Name", &sig.name), ("Reason", &sig.reason), ("Location", &sig.location), ("ContactInfo", &sig.contact)]
        {
            if let Some(v) = v {
                text.push_str(&format!("  /{key} {}\n", crate::sign::pdf_text(v)));
            }
        }
        text.push_str(">>\nendobj\n\n");
        self.put_raw(value, text.as_bytes())?;
        Ok(Some((field, value)))
    }

    fn content_stream(&mut self, raw: &[u8]) -> Result<Ref> {
        let id = self.next_id();
        let mut chunk = Chunk::new();
        if self.options.compress {
            chunk.stream(id, &deflate(raw)).filter(Filter::FlateDecode);
        } else {
            chunk.stream(id, raw);
        }
        self.put(id, &chunk)?;
        Ok(id)
    }

    /// The resource name of an image, writing the image the first time it is used.
    fn image_name(&mut self, image: &ImageData) -> Result<String> {
        if let Some((name, _)) = self.images.get(&image.id) {
            return Ok(name.clone());
        }
        let r = self.next_id();
        let mut chunk = Chunk::new();
        match &image.pixels {
            Pixels::Jpeg { data, components } => {
                let mut x = chunk.image_xobject(r, data);
                x.filter(Filter::DctDecode);
                x.width(image.width as i32).height(image.height as i32).bits_per_component(8);
                match components {
                    1 => x.color_space().device_gray(),
                    4 => x.color_space().device_cmyk(),
                    _ => x.color_space().device_rgb(),
                };
            }
            Pixels::Svg(svg) if crate::svg::needs_raster(&svg.tree) => {
                // Filters, masks and patterns: a sharp picture of the drawing.
                let size = svg.tree.size();
                let k = (4.0f32).min(4096.0 / size.width().max(size.height()).max(1.0));
                let (w, h) = (((size.width() * k).round() as u32).max(1), ((size.height() * k).round() as u32).max(1));
                let pixmap =
                    crate::svg::rasterize(&svg.tree, w, h).ok_or_else(|| Error::Data("SVG: too large to draw".into()))?;
                let mut samples = Vec::with_capacity((w * h * 3) as usize);
                let mut alpha = Vec::with_capacity((w * h) as usize);
                for px in pixmap.pixels() {
                    let c = px.demultiply();
                    samples.extend_from_slice(&[c.red(), c.green(), c.blue()]);
                    alpha.push(c.alpha());
                }
                self.raw_image(&mut chunk, r, w, h, &samples, false, Some(&alpha))?;
            }
            Pixels::Svg(svg) => {
                let form = crate::svg::form(&svg.tree);
                let mut pictures = Vec::with_capacity(form.images.len());
                for img in &form.images {
                    self.image_name(img)?;
                    pictures.push(self.images[&img.id].1);
                }
                let mut states = Vec::with_capacity(form.alphas.len());
                for (fill, stroke) in &form.alphas {
                    let id = self.next_id();
                    let mut c = Chunk::new();
                    c.ext_graphics(id).non_stroking_alpha(*fill).stroking_alpha(*stroke);
                    self.put(id, &c)?;
                    states.push(id);
                }
                let mut patterns = Vec::with_capacity(form.gradients.len());
                for g in &form.gradients {
                    patterns.push(self.gradient(g)?);
                }
                let (w, h) = (form.width.max(1e-3), form.height.max(1e-3));
                let content = if self.options.compress { deflate(&form.content) } else { form.content.clone() };
                let mut x = chunk.form_xobject(r, &content);
                if self.options.compress {
                    x.filter(Filter::FlateDecode);
                }
                x.bbox(pdf_writer::Rect::new(0.0, 0.0, w, h));
                // Drawn into a unit square like a picture, the SVG's top at its top.
                x.matrix([1.0 / w, 0.0, 0.0, -1.0 / h, 0.0, 1.0]);
                let mut res = x.resources();
                if !pictures.is_empty() {
                    let mut d = res.x_objects();
                    for (i, p) in pictures.iter().enumerate() {
                        d.pair(Name(format!("X{}", i + 1).as_bytes()), *p);
                    }
                }
                if !states.is_empty() {
                    let mut d = res.ext_g_states();
                    for (i, p) in states.iter().enumerate() {
                        d.pair(Name(format!("G{}", i + 1).as_bytes()), *p);
                    }
                }
                if !patterns.is_empty() {
                    let mut d = res.patterns();
                    for (i, p) in patterns.iter().enumerate() {
                        d.pair(Name(format!("P{}", i + 1).as_bytes()), *p);
                    }
                }
            }
            Pixels::Raw { samples, gray: false, alpha: Some(alpha) } if keyed(image.id, samples, alpha) => {
                // A chart bitmap: transparent pixels are black, and no other pixel is.
                let compressed = deflate(samples);
                let mut x = chunk.image_xobject(r, &compressed);
                x.filter(Filter::FlateDecode);
                x.width(image.width as i32).height(image.height as i32).bits_per_component(8);
                x.color_space().device_rgb();
                x.insert(Name(b"Mask")).array().items([0, 0, 0, 0, 0, 0]);
            }
            Pixels::Raw { samples, gray, alpha } => {
                self.raw_image(&mut chunk, r, image.width, image.height, samples, *gray, alpha.as_deref())?;
            }
        }
        self.put(r, &chunk)?;
        let name = format!("Im{}", self.images.len() + 1);
        self.images.insert(image.id, (name.clone(), r));
        Ok(name)
    }

    /// A gradient of an SVG as a shading pattern.
    fn gradient(&mut self, g: &crate::svg::Gradient) -> Result<Ref> {
        let mut stops = g.stops.clone();
        if stops.first().is_some_and(|s| s.0 > 0.0) {
            stops.insert(0, (0.0, stops[0].1));
        }
        if stops.last().is_some_and(|s| s.0 < 1.0) {
            let last = stops[stops.len() - 1].1;
            stops.push((1.0, last));
        }
        if stops.len() == 1 {
            stops.push((1.0, stops[0].1));
        }
        let mut pieces = Vec::with_capacity(stops.len() - 1);
        for pair in stops.windows(2) {
            let id = self.next_id();
            let mut chunk = Chunk::new();
            chunk.exponential_function(id).domain([0.0, 1.0]).c0(pair[0].1).c1(pair[1].1).n(1.0);
            self.put(id, &chunk)?;
            pieces.push(id);
        }
        let function = if pieces.len() == 1 {
            pieces[0]
        } else {
            let id = self.next_id();
            let bounds: Vec<f32> = stops[1..stops.len() - 1].iter().map(|s| s.0).collect();
            let encode: Vec<f32> = pieces.iter().flat_map(|_| [0.0, 1.0]).collect();
            let mut chunk = Chunk::new();
            chunk.stitching_function(id).domain([0.0, 1.0]).functions(pieces.iter().copied()).bounds(bounds).encode(encode);
            self.put(id, &chunk)?;
            id
        };
        let id = self.next_id();
        let mut chunk = Chunk::new();
        let mut pattern = chunk.shading_pattern(id);
        let mut shading = pattern.function_shading();
        shading.shading_type(if g.radial { FunctionShadingType::Radial } else { FunctionShadingType::Axial });
        shading.color_space().device_rgb();
        shading.function(function).coords(g.coords.iter().copied()).extend([true, true]);
        shading.finish();
        pattern.matrix(g.matrix);
        pattern.finish();
        self.put(id, &chunk)?;
        Ok(id)
    }

    /// Writes an image XObject of 8-bit samples, with a soft mask for the alpha channel.
    #[allow(clippy::too_many_arguments)]
    fn raw_image(
        &mut self,
        chunk: &mut Chunk,
        r: Ref,
        width: u32,
        height: u32,
        samples: &[u8],
        gray: bool,
        alpha: Option<&[u8]>,
    ) -> Result<()> {
        let mask = match alpha {
            Some(a) => {
                let mr = self.next_id();
                let compressed = deflate(a);
                let mut mc = Chunk::new();
                let mut m = mc.image_xobject(mr, &compressed);
                m.filter(Filter::FlateDecode);
                m.width(width as i32).height(height as i32).bits_per_component(8);
                m.color_space().device_gray();
                m.finish();
                self.put(mr, &mc)?;
                Some(mr)
            }
            None => None,
        };
        let compressed = deflate(samples);
        let mut x = chunk.image_xobject(r, &compressed);
        x.filter(Filter::FlateDecode);
        x.width(width as i32).height(height as i32).bits_per_component(8);
        if gray {
            x.color_space().device_gray();
        } else {
            x.color_space().device_rgb();
        }
        if let Some(mr) = mask {
            x.s_mask(mr);
        }
        Ok(())
    }

    /// The content stream that draws `items`.
    /// The content stream of `items`, and with tagging the pieces of content in it, in the
    /// order of their marked-content numbers.
    /// `page` is the page's number from 0: the first page is an odd one.
    fn draw(&mut self, items: &[PrintItem], fonts: &FontSet, page: usize) -> Result<(Vec<u8>, Vec<Node>, usize)> {
        let tagged = self.tags.is_some();
        let mut flat = Vec::with_capacity(items.len());
        flatten(items, tagged, &mut flat);
        let ph = self.page_height;
        let mut c = Content::new();
        let [dx, dy] = self.options.page_offsets[page % 2];
        if dx != 0.0 || dy != 0.0 {
            c.transform([1.0, 0.0, 0.0, 1.0, dx, -dy]);
        }
        c.set_line_cap(LineCapStyle::ButtCap);
        // The structure of the page as it is drawn: the pieces of content, numbered in
        // order, inside the tables and cells that are open.
        let mut numbered = 0usize;
        let mut nodes: Vec<Node> = vec![Node::new(Role::Group, [0.0; 4], Vec::new())];
        // In a tagged PDF everything drawn is either a piece of content or decoration.
        let (mut inside, mut decoration) = (false, false);
        // How many coordinate systems deep the drawing is, and was when decoration began.
        let (mut depth, mut decoration_depth) = (0usize, 0usize);
        for draw in flat.iter() {
            let item = match draw {
                Draw::Field(info) => {
                    self.pending_fields.push(Widget::of(info, ph, self.options.page_offsets[page % 2]));
                    continue;
                }
                Draw::Link(info) => {
                    let [ox, oy] = self.options.page_offsets[page % 2];
                    let top = ph - info.y - oy;
                    if let Some(name) = &info.anchor {
                        if !self.dests.iter().any(|(n, _, _)| n == name) {
                            self.dests.push((name.clone(), page, top));
                        }
                        if info.bookmark > 0 {
                            self.bookmarks.push((info.bookmark, name.clone(), page, top));
                        }
                    }
                    if let Some(target) = &info.target {
                        let rect = Rect::new(info.x + ox, top - info.h, info.x + info.w + ox, top);
                        let contents = info.tooltip.clone().unwrap_or_else(|| match target {
                            LinkTarget::Url(u) => u.clone(),
                            LinkTarget::Anchor(a) => a.clone(),
                            LinkTarget::Page(p) => format!("Page {p}"),
                            LinkTarget::RemoteAnchor { file, anchor } => format!("{file}#{anchor}"),
                            LinkTarget::RemotePage { file, page } => format!("{file}, page {page}"),
                        });
                        if tagged {
                            if let Some(parent) = nodes.last_mut() {
                                parent.kids.push(Node {
                                    annot: Some(self.pending_links.len()),
                                    ..Node::new(Role::Link, [info.x, info.y, info.w, info.h], Vec::new())
                                });
                            }
                        }
                        self.pending_links.push(PageLink { rect, target: target.clone(), contents, id: None, key: None });
                    }
                    continue;
                }
                Draw::Item(item) => {
                    if tagged && !inside && !decoration && !matches!(item, PrintItem::Element(_)) {
                        c.begin_marked_content(Name(b"Artifact"));
                        decoration = true;
                        decoration_depth = depth;
                    }
                    *item
                }
                Draw::Begin(mark) => {
                    if decoration {
                        c.end_marked_content();
                        decoration = false;
                    }
                    c.begin_marked_content_with_properties(Name(mark.role.name().as_bytes()))
                        .properties()
                        .pair(Name(b"MCID"), numbered as i32);
                    if let Some(parent) = nodes.last_mut() {
                        parent.kids.push(Node {
                            alt: mark.alt.clone(),
                            content: Some(numbered),
                            ..Node::new(mark.role, [0.0; 4], Vec::new())
                        });
                    }
                    numbered += 1;
                    inside = true;
                    continue;
                }
                Draw::Open(role, rect) => {
                    nodes.push(Node::new(*role, *rect, Vec::new()));
                    continue;
                }
                Draw::Close => {
                    if nodes.len() > 1 {
                        let mut node = nodes.pop().unwrap_or_else(|| unreachable!());
                        node.kids = if node.role == Role::Table { rows(node.kids) } else { adopt(node.kids) };
                        // An empty table or cell says nothing.
                        if let (false, Some(parent)) = (node.kids.is_empty(), nodes.last_mut()) {
                            parent.kids.push(node);
                        }
                    }
                    continue;
                }
                Draw::End => {
                    c.end_marked_content();
                    inside = false;
                    continue;
                }
                Draw::Push([a, b, cc, d, e, f]) => {
                    // Local items are written as if on the page; map them into place.
                    depth += 1;
                    c.save_state();
                    c.transform([*a, -*b, -*cc, *d, *cc * ph + *e, ph - *d * ph - *f]);
                    continue;
                }
                Draw::Pop => {
                    // Decoration that began inside this coordinate system ends with it.
                    if decoration && decoration_depth >= depth {
                        c.end_marked_content();
                        decoration = false;
                    }
                    depth = depth.saturating_sub(1);
                    c.restore_state();
                    continue;
                }
            };
            match item {
                PrintItem::Group(_)
                | PrintItem::Paragraphs(..)
                | PrintItem::Transform(..)
                | PrintItem::Cell(..)
                | PrintItem::Link(_)
                | PrintItem::Element(_) => {}
                PrintItem::Path { ops, fill, stroke, dash } => {
                    // A path that neither fills nor strokes clips what follows it (up to the
                    // end of the coordinate system it is in, charts' data areas).
                    let clip = fill.is_none() && stroke.is_none();
                    if ops.is_empty() || (clip && depth == 0) {
                        continue;
                    }
                    let y = |v: f64| ph - v as f32;
                    let mut cur = (0.0f64, 0.0f64);
                    for op in ops.iter() {
                        match *op {
                            PathOp::Move(a, b) => {
                                c.move_to(a as f32, y(b));
                                cur = (a, b);
                            }
                            PathOp::Line(a, b) => {
                                c.line_to(a as f32, y(b));
                                cur = (a, b);
                            }
                            PathOp::Quad(qx, qy, ex, ey) => {
                                // Exact cubic form of the quadratic.
                                let (c1x, c1y) = (cur.0 + 2.0 / 3.0 * (qx - cur.0), cur.1 + 2.0 / 3.0 * (qy - cur.1));
                                let (c2x, c2y) = (ex + 2.0 / 3.0 * (qx - ex), ey + 2.0 / 3.0 * (qy - ey));
                                c.cubic_to(c1x as f32, y(c1y), c2x as f32, y(c2y), ex as f32, y(ey));
                                cur = (ex, ey);
                            }
                            PathOp::Cubic(a, b, cc, d, e, f) => {
                                c.cubic_to(a as f32, y(b), cc as f32, y(d), e as f32, y(f));
                                cur = (e, f);
                            }
                            PathOp::Close => {
                                c.close_path();
                            }
                        }
                    }
                    if let Some(color) = fill {
                        let (r, g, b) = rgb(*color);
                        c.set_fill_rgb(r, g, b);
                    }
                    if let Some((color, width)) = stroke {
                        let (r, g, b) = rgb(*color);
                        c.set_stroke_rgb(r, g, b);
                        c.set_line_width(*width);
                        c.set_line_cap(LineCapStyle::ButtCap);
                        match dash {
                            Some(d) => c.set_dash_pattern(*d, 0.0),
                            None => c.set_dash_pattern([], 0.0),
                        };
                    }
                    match (fill.is_some(), stroke.is_some()) {
                        (true, true) => c.fill_nonzero_and_stroke(),
                        (true, false) => c.fill_nonzero(),
                        (false, true) => c.stroke(),
                        (false, false) => c.clip_nonzero().end_path(),
                    };
                }
                PrintItem::Rect { x, y, w, h, fill, stroke, radius, path } => {
                    let (px, py) = (*x, ph - y - h);
                    let shape = |c: &mut Content| {
                        if *radius > 0.0 {
                            rounded_rect(c, px, py, *w, *h, *radius);
                        } else if *path {
                            itext_rect(c, px, py, *w, *h);
                        } else {
                            c.rect(px, py, *w, *h);
                        }
                    };
                    if let Some(color) = fill {
                        let (r, g, b) = rgb(*color);
                        c.set_fill_rgb(r, g, b);
                        shape(&mut c);
                        c.fill_nonzero();
                    }
                    if let Some(pen) = stroke {
                        apply_pen(&mut c, pen, true);
                        shape(&mut c);
                        c.stroke();
                    }
                }
                PrintItem::Shapes { x, y, scale, color, rects, path } => {
                    let (r, g, b) = rgb(*color);
                    c.save_state();
                    c.set_fill_rgb(r, g, b);
                    let s = *scale as f32;
                    c.transform([s, 0.0, 0.0, -s, *x, ph - y]);
                    for [rx, ry, rw, rh] in rects.iter() {
                        c.rect(*rx as f32, *ry as f32, *rw as f32, *rh as f32);
                        c.fill_nonzero();
                    }
                    if !path.is_empty() {
                        for op in path.iter() {
                            match *op {
                                PathOp::Move(a, b) => c.move_to(a as f32, b as f32),
                                PathOp::Line(a, b) => c.line_to(a as f32, b as f32),
                                PathOp::Quad(a, b, x3, y3) => c.cubic_to_initial(a as f32, b as f32, x3 as f32, y3 as f32),
                                PathOp::Cubic(a, b, x2, y2, x3, y3) => {
                                    c.cubic_to(a as f32, b as f32, x2 as f32, y2 as f32, x3 as f32, y3 as f32)
                                }
                                PathOp::Close => c.close_path(),
                            };
                        }
                        c.fill_nonzero();
                    }
                    c.restore_state();
                }
                PrintItem::Ellipse { x, y, w, h, fill, stroke } => {
                    let (px, py) = (*x, ph - y - h);
                    if let Some(color) = fill {
                        let (r, g, b) = rgb(*color);
                        c.set_fill_rgb(r, g, b);
                        ellipse(&mut c, px, py, *w, *h);
                        c.fill_nonzero();
                    }
                    if let Some(pen) = stroke {
                        apply_pen(&mut c, pen, true);
                        ellipse(&mut c, px, py, *w, *h);
                        c.stroke();
                    }
                }
                PrintItem::Image { x, y, w, h, clip, image } => {
                    let name = self.image_name(image)?;
                    c.save_state();
                    if let Some([cx, cy, cw, ch]) = clip {
                        c.rect(*cx, ph - cy - ch, *cw, *ch);
                        c.clip_nonzero();
                        c.end_path();
                    }
                    c.transform([*w, 0.0, 0.0, *h, *x, ph - y - h]);
                    c.x_object(Name(name.as_bytes()));
                    c.restore_state();
                }
                PrintItem::Line { x1, y1, x2, y2, pen } => {
                    apply_pen(&mut c, pen, false);
                    if pen.style == LineStyle::Double {
                        // Two thin strokes on either side of the center line.
                        let (dx, dy) = if (y1 - y2).abs() < 1e-3 { (0.0, pen.width / 3.0) } else { (pen.width / 3.0, 0.0) };
                        c.set_line_width(pen.width / 3.0);
                        for s in [-1.0f32, 1.0] {
                            c.move_to(x1 + s * dx, ph - y1 - s * dy);
                            c.line_to(x2 + s * dx, ph - y2 - s * dy);
                        }
                    } else {
                        c.move_to(*x1, ph - y1);
                        c.line_to(*x2, ph - y2);
                    }
                    c.stroke();
                }
                PrintItem::Text(t) => {
                    if t.lines.iter().all(|l| l.text.is_empty()) {
                        continue;
                    }
                    if let Some(font) = t.standard {
                        if self.options.pdfa.is_some() {
                            return Err(Error::Unsupported(format!(
                                "PDF/A needs every font inside the document, and the text \"{}\" is in {}, a PDF standard font that is not: give it a font that is available (fontName; DejaVu Sans, Serif and Mono are bundled, others with --font), as JasperReports also requires",
                                t.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ").chars().take(40).collect::<String>(),
                                crate::standard::STANDARD[font.font as usize].0
                            )));
                        }
                        self.standard.insert(font.font);
                        draw_standard(&mut c, t, font, ph);
                        continue;
                    }
                    let next = self.faces.len() + 1;
                    // A face that simulates a style is written with the font it is drawn from.
                    let used = self.faces.entry(fonts.drawn_from(t.face)).or_insert_with(|| FaceUse::new(next));
                    let resource = used.resource.clone();
                    let face = fonts.face(t.face);
                    let (r, g, b) = rgb(t.color);
                    c.set_fill_rgb(r, g, b);
                    c.begin_text();
                    c.set_font(Name(resource.as_bytes()), t.size);
                    let simulated = face.simulated;
                    let skew = if simulated.italic { crate::fonts::SIMULATED_ITALIC_SKEW } else { 0.0 };
                    if simulated.bold {
                        // iText's simulated bold: the outlines stroked as well, a thirtieth of the
                        // size wide, in the text's color.
                        c.set_text_rendering_mode(pdf_writer::types::TextRenderingMode::FillStroke);
                        c.set_line_width(t.size / 30.0);
                        c.set_stroke_rgb(r, g, b);
                    }
                    for line in &t.lines {
                        if line.text.is_empty() {
                            continue;
                        }
                        if let Some(shaped) = &line.shaped {
                            show_shaped(&mut c, used, face, t.size, line, shaped, ph);
                            continue;
                        }
                        let mut encode = |s: &str| -> Vec<u8> {
                            s.chars()
                                // Characters the font lacks are left out, as JasperReports does.
                                .filter(|&ch| !ch.is_control() && face.has_glyph(ch))
                                .flat_map(|ch| used.code(face.glyph(ch), ch).to_be_bytes())
                                .collect()
                        };
                        c.set_text_matrix([1.0, 0.0, skew, 1.0, r2(line.x), r2(ph - line.baseline)]);
                        if line.word_spacing.abs() > 1e-4 {
                            // Justified: move each word by the extra spacing after every space
                            // (Tw does not apply to two-byte fonts).
                            let adjust = -line.word_spacing * 1000.0 / t.size;
                            let segments: Vec<&str> = line.text.split_inclusive(' ').collect();
                            let mut positioned = c.show_positioned();
                            let mut items = positioned.items();
                            for (i, seg) in segments.iter().enumerate() {
                                items.show(Str(&encode(seg)));
                                if i + 1 < segments.len() && seg.ends_with(' ') {
                                    items.adjust(adjust);
                                }
                            }
                        } else {
                            c.show(Str(&encode(&line.text)));
                        }
                    }
                    if simulated.bold {
                        c.set_text_rendering_mode(pdf_writer::types::TextRenderingMode::Fill);
                    }
                    c.end_text();
                    if t.underline || t.strike {
                        // Same geometry as iText/JasperReports: thickness size/18, underline at
                        // size/12 below the baseline, strike-through at size/3 above it.
                        let thick = t.size / 18.0;
                        for line in &t.lines {
                            for (on, offset) in [(t.underline, -t.size / 12.0), (t.strike, t.size / 3.0)] {
                                if on && line.width > 0.0 {
                                    let y = ph - line.baseline + offset;
                                    c.set_stroke_rgb(r, g, b);
                                    c.set_line_width(thick);
                                    c.set_dash_pattern([], 0.0);
                                    c.move_to(line.x, y);
                                    c.line_to(line.x + line.width, y);
                                    c.stroke();
                                }
                            }
                        }
                    }
                }
            }
        }
        if decoration {
            c.end_marked_content();
        }
        // Anything left open ends with the page.
        while nodes.len() > 1 {
            let mut node = nodes.pop().unwrap_or_else(|| unreachable!());
            node.kids = if node.role == Role::Table { rows(node.kids) } else { adopt(node.kids) };
            if let (false, Some(parent)) = (node.kids.is_empty(), nodes.last_mut()) {
                parent.kids.push(node);
            }
        }
        let top = adopt(nodes.pop().map(|n| n.kids).unwrap_or_default());
        Ok((c.finish().to_vec(), top, numbered))
    }

    /// Adds the next page.
    pub fn page(&mut self, items: &[PrintItem], fonts: &FontSet) -> Result<()> {
        let (raw, nodes, numbered) = self.draw(items, fonts, self.pages.len())?;
        let id = self.content_stream(&raw)?;
        let number = self.pages.len();
        self.pages.push(id);
        if !self.pending_fields.is_empty() {
            self.fields.insert(number, std::mem::take(&mut self.pending_fields));
        }
        if !self.pending_links.is_empty() {
            self.links.insert(number, std::mem::take(&mut self.pending_links));
        }
        if self.tags.is_some() {
            self.tag_page(&nodes, numbered)?;
        }
        Ok(())
    }

    /// Writes the part of the structure tree that describes the page just drawn: a section
    /// holding its texts, pictures and tables, in reading order.
    fn tag_page(&mut self, nodes: &[Node], numbered: usize) -> Result<()> {
        use pdf_writer::types::StructRole;
        let page = self.next_id();
        let Some(document) = self.tags.as_ref().map(|t| t.document) else { return Ok(()) };
        if nodes.is_empty() {
            if let Some(tags) = &mut self.tags {
                tags.pages.push((page, None));
            }
            return Ok(());
        }
        let section = self.next_id();
        // From the numbers in the page's content back to the elements that hold them.
        let mut back = vec![section; numbered];
        let mut kids = Vec::with_capacity(nodes.len());
        for node in nodes {
            kids.push(self.tag_node(node, section, page, &mut back)?);
        }
        let mut chunk = Chunk::new();
        let mut e = chunk.struct_element(section);
        e.kind(StructRole::Sect).parent(document);
        let mut children = e.children();
        for id in &kids {
            children.struct_element(*id);
        }
        children.finish();
        e.finish();
        self.put(section, &chunk)?;
        let array = self.next_id();
        let mut chunk = Chunk::new();
        chunk.indirect(array).array().items(back.iter().copied());
        self.put(array, &chunk)?;
        if let Some(tags) = &mut self.tags {
            tags.pages.push((page, Some((section, array))));
        }
        Ok(())
    }

    /// Writes one element of the structure and what it holds; returns its object.
    fn tag_node(&mut self, node: &Node, parent: Ref, page: Ref, back: &mut [Ref]) -> Result<Ref> {
        use pdf_writer::types::{StructRole, TableHeaderScope};
        let id = self.next_id();
        if let Some(index) = node.annot {
            // A link holds its annotation, which leads back to it through the parent tree.
            let annot = self.next_id();
            let key = LINK_KEYS + self.link_parents.len() as i32;
            self.link_parents.push((key, id));
            let current = self.pages.len().saturating_sub(1);
            if let Some(link) = self.links.get_mut(&current).and_then(|l| l.get_mut(index)) {
                link.id = Some(annot);
                link.key = Some(key);
            }
            let mut chunk = Chunk::new();
            let mut e = chunk.struct_element(id);
            e.kind(StructRole::Link).parent(parent).page(page);
            e.children().object_ref().object(annot).page(page);
            e.finish();
            return self.put(id, &chunk).map(|_| id);
        }
        if let Some(number) = node.content {
            back[number] = id;
            if let Some(alt) = &node.alt {
                let mut chunk = Chunk::new();
                let mut e = chunk.struct_element(id);
                e.kind(StructRole::Figure).parent(parent).page(page).alt(TextStr(alt));
                e.children().marked_content_id(number as i32);
                e.finish();
                return self.put(id, &chunk).map(|_| id);
            }
            // Texts are most of a report: written as tightly as the format allows (with the
            // line breaks around obj and endobj that PDF/A asks for), they are a third of the
            // size.
            let line = format!(
                "{} 0 obj\n<</S/{}/P {} 0 R/Pg {} 0 R/K {number}>>\nendobj\n",
                id.get(),
                node.role.name(),
                parent.get(),
                page.get()
            );
            self.offsets[id.get() as usize - 1] = self.pos;
            self.out.write_all(line.as_bytes()).map_err(io)?;
            self.pos += line.len() as u64;
            return Ok(id);
        }
        let mut kids = Vec::with_capacity(node.kids.len());
        for kid in &node.kids {
            kids.push(self.tag_node(kid, id, page, back)?);
        }
        let mut chunk = Chunk::new();
        let mut e = chunk.struct_element(id);
        e.kind(match node.role {
            Role::Table => StructRole::Table,
            Role::Row => StructRole::TR,
            Role::HeaderCell | Role::RowHeaderCell => StructRole::TH,
            Role::DataCell => StructRole::TD,
            Role::List(_) => StructRole::L,
            Role::ListItem => StructRole::LI,
            Role::ListLabel => StructRole::Lbl,
            Role::ListBody => StructRole::LBody,
            _ => StructRole::Div,
        })
        .parent(parent)
        .page(page);
        if let Role::List(ordered) = node.role {
            use pdf_writer::types::ListNumbering;
            e.attributes().push().list().list_numbering(if ordered { ListNumbering::Decimal } else { ListNumbering::Disc });
        }
        // A header cell says what it names, its column or its row; a cell over several
        // columns or rows says how many.
        let scope = match node.role {
            Role::HeaderCell => Some(TableHeaderScope::Column),
            Role::RowHeaderCell => Some(TableHeaderScope::Row),
            _ => None,
        };
        if node.role.is_cell() && (scope.is_some() || node.span != (1, 1)) {
            let mut attributes = e.attributes();
            let mut table = attributes.push().table();
            if let Some(scope) = scope {
                table.scope(scope);
            }
            if node.span.0 > 1 {
                table.col_span(node.span.0 as i32);
            }
            if node.span.1 > 1 {
                table.row_span(node.span.1 as i32);
            }
        }
        let mut children = e.children();
        for kid in &kids {
            children.struct_element(*kid);
        }
        children.finish();
        e.finish();
        self.put(id, &chunk)?;
        Ok(id)
    }

    /// Draws `items` on top of a page added earlier (numbered from 0).
    pub fn late(&mut self, page: usize, items: &[PrintItem], fonts: &FontSet) -> Result<()> {
        if self.tags.is_some() {
            // A tagged page is complete when it is written: its structure goes with it.
            return Err(Error::Io("content cannot be added to a page of a tagged PDF once it is written".into()));
        }
        let (raw, _, _) = self.draw(items, fonts, page)?;
        let id = self.content_stream(&raw)?;
        if page < self.pages.len() {
            self.more_contents.entry(page).or_default().push(id);
            let late = std::mem::take(&mut self.pending_fields);
            if !late.is_empty() {
                self.fields.entry(page).or_default().extend(late);
            }
            let late = std::mem::take(&mut self.pending_links);
            if !late.is_empty() {
                self.links.entry(page).or_default().extend(late);
            }
        }
        Ok(())
    }

    /// Pages added so far.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The document's metadata packet: its title and producer, and what it conforms to
    /// (PDF/UA-1 when tagged, the PDF/A part and level when asked).
    fn xmp(&self, tagged: bool) -> String {
        let title = crate::xlsx::escape(self.options.title.as_deref().unwrap_or(""));
        let mut ids = String::new();
        let mut namespaces = String::new();
        if let Some(a) = self.options.pdfa {
            namespaces.push_str(" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"");
            ids.push_str(&format!(
                "<pdfaid:part>{}</pdfaid:part>\n<pdfaid:conformance>{}</pdfaid:conformance>\n",
                a.part, a.level
            ));
        }
        if tagged {
            namespaces.push_str(" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"");
            ids.push_str("<pdfuaid:part>1</pdfuaid:part>\n");
        }
        // PDF/A knows the properties of its own time; PDF/UA's is described for it.
        let extension = if tagged && self.options.pdfa.is_some() {
            "<rdf:Description rdf:about=\"\" xmlns:pdfaExtension=\"http://www.aiim.org/pdfa/ns/extension/\" xmlns:pdfaSchema=\"http://www.aiim.org/pdfa/ns/schema#\" xmlns:pdfaProperty=\"http://www.aiim.org/pdfa/ns/property#\">\n<pdfaExtension:schemas><rdf:Bag><rdf:li rdf:parseType=\"Resource\">\n<pdfaSchema:schema>PDF/UA Universal Accessibility Schema</pdfaSchema:schema>\n<pdfaSchema:namespaceURI>http://www.aiim.org/pdfua/ns/id/</pdfaSchema:namespaceURI>\n<pdfaSchema:prefix>pdfuaid</pdfaSchema:prefix>\n<pdfaSchema:property><rdf:Seq><rdf:li rdf:parseType=\"Resource\">\n<pdfaProperty:name>part</pdfaProperty:name>\n<pdfaProperty:valueType>Integer</pdfaProperty:valueType>\n<pdfaProperty:category>internal</pdfaProperty:category>\n<pdfaProperty:description>Indicates, which part of ISO 14289 standard is followed</pdfaProperty:description>\n</rdf:li></rdf:Seq></pdfaSchema:property>\n</rdf:li></rdf:Bag></pdfaExtension:schemas>\n</rdf:Description>\n"
        } else {
            ""
        };
        format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n{extension}<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"{namespaces}>\n{ids}<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li></rdf:Alt></dc:title>\n<pdf:Producer>ferroreport {}</pdf:Producer>\n</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
            env!("CARGO_PKG_VERSION")
        )
    }

    /// The file identifier of the trailer: a hash of what makes this document what it is.
    fn file_id(&self) -> String {
        let feed = |mut h: u64, bytes: &[u8]| {
            for b in bytes {
                h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
            }
            h
        };
        let mut h = feed(0xcbf2_9ce4_8422_2325, self.options.title.as_deref().unwrap_or("").as_bytes());
        h = feed(h, &self.pos.to_le_bytes());
        h = feed(h, &(self.offsets.len() as u64).to_le_bytes());
        let b = feed(h, b"ferroreport");
        format!("{h:016X}{b:016X}")
    }

    /// Writes the attached files; returns each one's name and file specification.
    fn write_attachments(&mut self) -> Result<Vec<(String, Ref)>> {
        let mut out = Vec::new();
        for file in std::mem::take(&mut self.options.attachments) {
            let (spec, data) = (self.next_id(), self.next_id());
            let compressed = deflate(&file.data);
            let mut chunk = Chunk::new();
            let mut ef = chunk.embedded_file(data, &compressed);
            ef.filter(Filter::FlateDecode);
            ef.subtype(Name(file.mime.as_bytes()));
            ef.params().size(file.data.len() as i32);
            ef.finish();
            self.put(data, &chunk)?;
            let mut chunk = Chunk::new();
            let mut fs = chunk.file_spec(spec);
            fs.path(Str(file.name.as_bytes())).unic_file(TextStr(&file.name));
            fs.insert(Name(b"EF")).dict().pair(Name(b"F"), data).pair(Name(b"UF"), data);
            if let Some(d) = &file.description {
                fs.description(TextStr(d));
            }
            fs.pair(Name(b"AFRelationship"), Name(file.relationship.as_bytes()));
            fs.finish();
            self.put(spec, &chunk)?;
            out.push((file.name, spec));
        }
        Ok(out)
    }

    /// Writes the link annotations of each page.
    fn write_links(&mut self, page_ids: &[Ref]) -> Result<PageAnnots> {
        use pdf_writer::types::ActionType;
        let mut out = BTreeMap::new();
        for (number, list) in std::mem::take(&mut self.links) {
            let mut refs = Vec::with_capacity(list.len());
            for link in list {
                let id = match link.id {
                    Some(id) => id,
                    None => self.next_id(),
                };
                let mut chunk = Chunk::new();
                let mut a = chunk.annotation(id);
                a.subtype(pdf_writer::types::AnnotationType::Link)
                    .rect(link.rect)
                    .border(0.0, 0.0, 0.0, None)
                    .contents(TextStr(&link.contents))
                    .flags(pdf_writer::types::AnnotationFlags::PRINT);
                if let Some(key) = link.key {
                    a.struct_parent(key);
                }
                match &link.target {
                    LinkTarget::Url(url) => {
                        a.action().action_type(ActionType::Uri).uri(Str(url.as_bytes()));
                    }
                    LinkTarget::Anchor(name) => {
                        a.pair(Name(b"Dest"), Str(name.as_bytes()));
                    }
                    LinkTarget::Page(p) => {
                        let page = page_ids.get(*p as usize - 1).or(page_ids.last()).copied();
                        if let Some(page) = page {
                            a.insert(Name(b"Dest")).start::<pdf_writer::writers::Destination>().page(page).xyz(
                                0.0,
                                self.page_height,
                                None,
                            );
                        }
                    }
                    LinkTarget::RemoteAnchor { file, anchor } => {
                        let mut act = a.action();
                        act.action_type(ActionType::RemoteGoTo);
                        act.pair(Name(b"F"), Str(file.as_bytes()));
                        act.pair(Name(b"D"), Str(anchor.as_bytes()));
                    }
                    LinkTarget::RemotePage { file, page } => {
                        let mut act = a.action();
                        act.action_type(ActionType::RemoteGoTo);
                        act.pair(Name(b"F"), Str(file.as_bytes()));
                        act.insert(Name(b"D")).array().item(page.saturating_sub(1) as i32).item(Name(b"Fit"));
                    }
                }
                a.finish();
                self.put(id, &chunk)?;
                refs.push(id);
            }
            out.insert(number, refs);
        }
        Ok(out)
    }

    /// Writes the anchors (as named destinations, sorted by name as the name tree wants them)
    /// and the bookmarks (as the document outline).
    fn write_destinations(&mut self, page_ids: &[Ref]) -> Result<(NamedDests, Option<Ref>)> {
        let mut dests = Vec::new();
        let mut list = std::mem::take(&mut self.dests);
        list.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        for (name, page, top) in list {
            let Some(page) = page_ids.get(page).copied() else { continue };
            let id = self.next_id();
            let mut chunk = Chunk::new();
            chunk.destination(id).page(page).xyz(0.0, top, None);
            self.put(id, &chunk)?;
            dests.push((name, id));
        }
        let marks = std::mem::take(&mut self.bookmarks);
        if marks.is_empty() {
            return Ok((dests, None));
        }
        // Each bookmark goes under the last one of a lower level before it.
        let root = self.next_id();
        let ids: Vec<Ref> = marks.iter().map(|_| self.next_id()).collect();
        let mut parent: Vec<Option<usize>> = Vec::with_capacity(marks.len());
        let mut open: Vec<usize> = Vec::new();
        for (i, (level, ..)) in marks.iter().enumerate() {
            while open.last().is_some_and(|&j| marks[j].0 >= *level) {
                open.pop();
            }
            parent.push(open.last().copied());
            open.push(i);
        }
        let kids = |p: Option<usize>| -> Vec<usize> { (0..marks.len()).filter(|&i| parent[i] == p).collect() };
        for (i, (_, title, page, top)) in marks.iter().enumerate() {
            let siblings = kids(parent[i]);
            let at = siblings.iter().position(|&s| s == i).unwrap_or(0);
            let children = kids(Some(i));
            let mut chunk = Chunk::new();
            let mut item = chunk.outline_item(ids[i]);
            item.title(TextStr(title)).parent(parent[i].map(|p| ids[p]).unwrap_or(root));
            if at > 0 {
                item.prev(ids[siblings[at - 1]]);
            }
            if let Some(&next) = siblings.get(at + 1) {
                item.next(ids[next]);
            }
            if let (Some(&first), Some(&last)) = (children.first(), children.last()) {
                item.first(ids[first]).last(ids[last]).count(-(children.len() as i32));
            }
            if let Some(page) = page_ids.get(*page).copied() {
                item.dest().page(page).xyz(0.0, *top, None);
            }
            item.finish();
            self.put(ids[i], &chunk)?;
        }
        let top_level = kids(None);
        let mut chunk = Chunk::new();
        let mut outline = chunk.outline(root);
        if let (Some(&first), Some(&last)) = (top_level.first(), top_level.last()) {
            outline.first(ids[first]).last(ids[last]).count(top_level.len() as i32);
        }
        outline.finish();
        self.put(root, &chunk)?;
        Ok((dests, Some(root)))
    }

    /// Writes the fonts, the page tree and the trailer, and returns the output.
    /// Writes the form fields: the annotations of each page and the form they make up.
    fn write_fields(&mut self, signature: Option<(Ref, Ref)>) -> Result<(PageAnnots, Option<Ref>)> {
        use crate::model::FieldKind;
        let fields = std::mem::take(&mut self.fields);
        let no_fields = fields.values().all(|f| f.is_empty());
        if no_fields && signature.is_none() {
            return Ok((BTreeMap::new(), None));
        }
        let helv = self.next_id();
        let zadb = self.next_id();
        let mut chunk = Chunk::new();
        chunk.type1_font(helv).base_font(Name(b"Helvetica")).encoding_predefined(Name(b"WinAnsiEncoding"));
        self.put(helv, &chunk)?;
        let mut chunk = Chunk::new();
        chunk.type1_font(zadb).base_font(Name(b"ZapfDingbats"));
        self.put(zadb, &chunk)?;
        let text_str = |s: &str| -> Vec<u8> {
            // PDF text strings: Latin-1 as it is, anything else as UTF-16.
            if s.chars().all(|c| (c as u32) < 256) {
                s.chars().map(|c| c as u8).collect()
            } else {
                let mut out = vec![0xFE, 0xFF];
                for u in s.encode_utf16() {
                    out.extend(u.to_be_bytes());
                }
                out
            }
        };
        let rgb3 = |c: Color| [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0];
        let mut top_level: Vec<Ref> = Vec::new();
        let mut annots: BTreeMap<usize, Vec<Ref>> = BTreeMap::new();
        // Radio buttons with the same name are one field.
        let mut radios: Vec<(String, Ref, Vec<Ref>, Option<String>)> = Vec::new();
        let mut unnamed = 0;
        for (&page, list) in fields.iter() {
            for w in list {
                let id = self.next_id();
                annots.entry(page).or_default().push(id);
                let f = &w.field;
                let name = f.name.clone().unwrap_or_else(|| {
                    unnamed += 1;
                    format!("field{unnamed}")
                });
                let [r, g, b] = rgb3(w.color);
                let (bw, bh) = (w.rect.x2 - w.rect.x1, w.rect.y2 - w.rect.y1);
                // The mark of a check box or radio button, drawn when it is on.
                let mark_stream = |pdf: &mut Self, mark: char| -> Result<(Ref, Ref)> {
                    let (on, off) = (pdf.next_id(), pdf.next_id());
                    let size = bw.min(bh) * 0.8;
                    let ops = format!(
                        "q BT /ZaDb {size:.2} Tf {r:.3} {g:.3} {b:.3} rg {:.2} {:.2} Td ({mark}) Tj ET Q",
                        (bw - size * 0.8) / 2.0,
                        (bh - size * 0.7) / 2.0
                    );
                    for (rf, body) in [(on, ops.as_bytes().to_vec()), (off, Vec::new())] {
                        let mut chunk = Chunk::new();
                        let mut x = chunk.form_xobject(rf, &body);
                        x.bbox(Rect::new(0.0, 0.0, bw, bh));
                        x.resources().fonts().pair(Name(b"ZaDb"), zadb);
                        x.finish();
                        pdf.put(rf, &chunk)?;
                    }
                    Ok((on, off))
                };
                let mut chunk = Chunk::new();
                let mut d = chunk.indirect(id).dict();
                d.pair(Name(b"Type"), Name(b"Annot")).pair(Name(b"Subtype"), Name(b"Widget"));
                d.insert(Name(b"Rect")).primitive(w.rect);
                d.pair(Name(b"F"), 4);
                {
                    let mut mk = d.insert(Name(b"MK")).dict();
                    if let Some(c) = w.border {
                        mk.insert(Name(b"BC")).array().items(rgb3(c));
                    }
                    if let Some(c) = w.back {
                        mk.insert(Name(b"BG")).array().items(rgb3(c));
                    }
                    if matches!(f.kind, FieldKind::Check | FieldKind::Radio) {
                        mk.pair(Name(b"CA"), Str(f.check_char.to_string().as_bytes()));
                    }
                }
                {
                    let mut bs = d.insert(Name(b"BS")).dict();
                    bs.pair(Name(b"W"), if w.border.is_some() { 1 } else { 0 });
                    bs.pair(Name(b"S"), Name(f.border.to_string().as_bytes()));
                }
                let mut flags = if f.read_only { 1 } else { 0 };
                match f.kind {
                    FieldKind::Text | FieldKind::Combo | FieldKind::List => {
                        let value = f.value.clone().unwrap_or_else(|| w.value.clone());
                        d.pair(Name(b"T"), Str(&text_str(&name)));
                        d.pair(Name(b"V"), Str(&text_str(&value)));
                        let da = format!("/Helv {:.1} Tf {r:.3} {g:.3} {b:.3} rg", w.size);
                        d.pair(Name(b"DA"), Str(da.as_bytes()));
                        if f.kind == FieldKind::Text {
                            d.pair(Name(b"FT"), Name(b"Tx"));
                            if f.multiline {
                                flags |= 1 << 12;
                            }
                        } else {
                            d.pair(Name(b"FT"), Name(b"Ch"));
                            if f.kind == FieldKind::Combo {
                                flags |= 1 << 17;
                                if f.editable {
                                    flags |= 1 << 18;
                                }
                            }
                            let mut opt = d.insert(Name(b"Opt")).array();
                            for c in &f.choices {
                                opt.item(Str(&text_str(c)));
                            }
                        }
                        d.pair(Name(b"Ff"), flags);
                        top_level.push(id);
                        d.finish();
                    }
                    FieldKind::Check => {
                        drop(d);
                        let (on, off) = mark_stream(self, f.check_char)?;
                        let mut chunk = Chunk::new();
                        let mut d = chunk.indirect(id).dict();
                        d.pair(Name(b"Type"), Name(b"Annot")).pair(Name(b"Subtype"), Name(b"Widget"));
                        d.insert(Name(b"Rect")).primitive(w.rect);
                        d.pair(Name(b"F"), 4);
                        {
                            let mut mk = d.insert(Name(b"MK")).dict();
                            if let Some(c) = w.border {
                                mk.insert(Name(b"BC")).array().items(rgb3(c));
                            }
                            if let Some(c) = w.back {
                                mk.insert(Name(b"BG")).array().items(rgb3(c));
                            }
                            mk.pair(Name(b"CA"), Str(f.check_char.to_string().as_bytes()));
                        }
                        d.pair(Name(b"T"), Str(&text_str(&name)));
                        d.pair(Name(b"FT"), Name(b"Btn"));
                        d.pair(Name(b"Ff"), flags);
                        let state: &[u8] = if f.checked { b"Yes" } else { b"Off" };
                        d.pair(Name(b"V"), Name(state)).pair(Name(b"AS"), Name(state));
                        let da = format!("/ZaDb 0 Tf {r:.3} {g:.3} {b:.3} rg");
                        d.pair(Name(b"DA"), Str(da.as_bytes()));
                        {
                            let mut ap = d.insert(Name(b"AP")).dict();
                            let mut n = ap.insert(Name(b"N")).dict();
                            n.pair(Name(b"Yes"), on).pair(Name(b"Off"), off);
                        }
                        d.finish();
                        top_level.push(id);
                        self.put(id, &chunk)?;
                        continue;
                    }
                    FieldKind::Radio => {
                        drop(d);
                        let (on, off) = mark_stream(self, f.check_char)?;
                        let group = match radios.iter().position(|(n, ..)| *n == name) {
                            Some(i) => i,
                            None => {
                                let parent = self.next_id();
                                radios.push((name.clone(), parent, Vec::new(), None));
                                radios.len() - 1
                            }
                        };
                        let state = format!("choice{}", radios[group].2.len() + 1);
                        radios[group].2.push(id);
                        if f.checked {
                            radios[group].3 = Some(state.clone());
                        }
                        let parent = radios[group].1;
                        let mut chunk = Chunk::new();
                        let mut d = chunk.indirect(id).dict();
                        d.pair(Name(b"Type"), Name(b"Annot")).pair(Name(b"Subtype"), Name(b"Widget"));
                        d.insert(Name(b"Rect")).primitive(w.rect);
                        d.pair(Name(b"F"), 4).pair(Name(b"Parent"), parent);
                        {
                            let mut mk = d.insert(Name(b"MK")).dict();
                            if let Some(c) = w.border {
                                mk.insert(Name(b"BC")).array().items(rgb3(c));
                            }
                            mk.pair(Name(b"CA"), Str(f.check_char.to_string().as_bytes()));
                        }
                        let current: &[u8] = if f.checked { state.as_bytes() } else { b"Off" };
                        d.pair(Name(b"AS"), Name(current));
                        {
                            let mut ap = d.insert(Name(b"AP")).dict();
                            let mut n = ap.insert(Name(b"N")).dict();
                            n.pair(Name(state.as_bytes()), on).pair(Name(b"Off"), off);
                        }
                        d.finish();
                        self.put(id, &chunk)?;
                        continue;
                    }
                }
                self.put(id, &chunk)?;
            }
        }
        for (name, parent, kids, selected) in radios {
            let mut chunk = Chunk::new();
            let mut d = chunk.indirect(parent).dict();
            d.pair(Name(b"FT"), Name(b"Btn")).pair(Name(b"T"), Str(&text_str(&name)));
            // Radio, and one is always on.
            d.pair(Name(b"Ff"), (1 << 15) | (1 << 14));
            d.pair(Name(b"V"), Name(selected.as_deref().unwrap_or("Off").as_bytes()));
            d.insert(Name(b"Kids")).array().items(kids);
            d.finish();
            self.put(parent, &chunk)?;
            top_level.push(parent);
        }
        // The signature: an invisible field on the first page.
        if let Some((field, value)) = signature {
            let mut chunk = Chunk::new();
            let mut d = chunk.indirect(field).dict();
            d.pair(Name(b"Type"), Name(b"Annot")).pair(Name(b"Subtype"), Name(b"Widget"));
            d.pair(Name(b"FT"), Name(b"Sig")).pair(Name(b"T"), Str(b"Signature1")).pair(Name(b"V"), value);
            d.insert(Name(b"Rect")).array().items([0.0f32, 0.0, 0.0, 0.0]);
            // Print and locked.
            d.pair(Name(b"F"), 132);
            d.finish();
            self.put(field, &chunk)?;
            top_level.push(field);
            annots.entry(0).or_default().push(field);
        }
        let form = self.next_id();
        let mut chunk = Chunk::new();
        let mut d = chunk.indirect(form).dict();
        d.insert(Name(b"Fields")).array().items(top_level);
        if signature.is_some() {
            // Signed, and only appended to after it (incremental updates keep the signature).
            d.pair(Name(b"SigFlags"), 3);
        }
        if !no_fields {
            d.pair(Name(b"NeedAppearances"), true);
        }
        d.pair(Name(b"DA"), Str(b"/Helv 0 Tf 0 g"));
        {
            let mut dr = d.insert(Name(b"DR")).dict();
            let mut f = dr.insert(Name(b"Font")).dict();
            f.pair(Name(b"Helv"), helv).pair(Name(b"ZaDb"), zadb);
        }
        d.finish();
        self.put(form, &chunk)?;
        Ok((annots, Some(form)))
    }

    pub fn finish(mut self, fonts: &FontSet) -> Result<W> {
        if self.pages.is_empty() {
            self.page(&[], fonts)?;
        }
        // Fonts, each cut down to the glyphs the document used.
        let mut font_refs: Vec<(String, Ref)> = Vec::new();
        for (face_id, used) in std::mem::take(&mut self.faces) {
            let face = fonts.face(face_id);
            let subset =
                subsetter::subset(&face.data, 0, &used.remapper).map_err(|e| Error::Font(format!("{}: {e}", face.family)))?;
            let glyph_set: BTreeSet<u16> =
                used.chars.iter().map(|&(g, _)| g).chain(used.strings.iter().map(|(g, _)| *g)).collect();
            let base_name = format!("{}+{}", subset_tag(face_id, &glyph_set), face.postscript_name);

            let type0 = self.next_id();
            let cid = self.next_id();
            let descriptor = self.next_id();
            let cmap_ref = self.next_id();
            let file = self.next_id();
            let scale = 1000.0 / face.units_per_em;

            let mut chunk = Chunk::new();
            chunk
                .type0_font(type0)
                .base_font(Name(base_name.as_bytes()))
                .encoding_predefined(Name(b"Identity-H"))
                .descendant_font(cid)
                .to_unicode(cmap_ref);
            self.put(type0, &chunk)?;

            let system_info = SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 };
            // iText (used by JasperReports) truncates glyph widths to whole units; viewers advance
            // glyphs by these widths, so we do the same to match its output.
            let widths: Vec<f32> = used.remapper.remapped_gids().map(|old| (face.advance(old) * scale).trunc()).collect();
            let mut chunk = Chunk::new();
            let mut cid_font = chunk.cid_font(cid);
            cid_font
                .subtype(CidFontType::Type2)
                .base_font(Name(base_name.as_bytes()))
                .system_info(system_info)
                .font_descriptor(descriptor)
                .default_width(0.0)
                .cid_to_gid_map_predefined(Name(b"Identity"));
            cid_font.widths().consecutive(0, widths);
            cid_font.finish();
            self.put(cid, &chunk)?;

            let mut flags = FontFlags::NON_SYMBOLIC;
            if face.family.contains("Serif") {
                flags |= FontFlags::SERIF;
            }
            if face.family.contains("Mono") {
                flags |= FontFlags::FIXED_PITCH;
            }
            if face.italic {
                flags |= FontFlags::ITALIC;
            }
            let b = face.bbox;
            let mut chunk = Chunk::new();
            chunk
                .font_descriptor(descriptor)
                .name(Name(base_name.as_bytes()))
                .flags(flags)
                .bbox(Rect::new(b[0] * scale, b[1] * scale, b[2] * scale, b[3] * scale))
                .italic_angle(face.italic_angle)
                .ascent(face.ascender * scale)
                .descent(-face.descender * scale)
                .cap_height(face.cap_height * scale)
                .stem_v(if face.bold { 120.0 } else { 80.0 })
                .font_file2(file);
            self.put(descriptor, &chunk)?;

            let mut cmap = pdf_writer::types::UnicodeCmap::new(Name(b"Custom"), system_info);
            let mut pairs: Vec<(u16, char)> =
                used.chars.iter().filter_map(|&(old, c)| used.remapper.get(old).map(|new| (new, c))).collect();
            let mut multiple: Vec<(u16, &str)> =
                used.strings.iter().filter_map(|(old, s)| used.remapper.get(*old).map(|new| (new, s.as_str()))).collect();
            pairs.sort_unstable();
            multiple.sort_unstable();
            let mut multiple = multiple.into_iter().peekable();
            for (new, c) in pairs {
                while let Some((m, s)) = multiple.next_if(|&(m, _)| m < new) {
                    cmap.pair_with_multiple(m, s.chars());
                }
                cmap.pair(new, c);
            }
            for (m, s) in multiple {
                cmap.pair_with_multiple(m, s.chars());
            }
            let mut chunk = Chunk::new();
            chunk.cmap(cmap_ref, &cmap.finish());
            self.put(cmap_ref, &chunk)?;

            let compressed = deflate(&subset);
            let mut chunk = Chunk::new();
            chunk.stream(file, &compressed).filter(Filter::FlateDecode).pair(Name(b"Length1"), subset.len() as i32);
            self.put(file, &chunk)?;

            font_refs.push((used.resource, type0));
        }

        // The standard fonts, not embedded, as iText writes them.
        for font in std::mem::take(&mut self.standard) {
            let id = self.next_id();
            let mut chunk = Chunk::new();
            chunk
                .type1_font(id)
                .base_font(Name(crate::standard::STANDARD[font as usize].0.as_bytes()))
                .encoding_predefined(Name(b"WinAnsiEncoding"));
            self.put(id, &chunk)?;
            font_refs.push((format!("S{font}"), id));
        }

        // One resource dictionary shared by every page.
        let resources = self.next_id();
        let mut chunk = Chunk::new();
        let mut res = chunk.indirect(resources).start::<pdf_writer::writers::Resources>();
        let mut fonts_dict = res.fonts();
        for (name, r) in &font_refs {
            fonts_dict.pair(Name(name.as_bytes()), *r);
        }
        fonts_dict.finish();
        if !self.images.is_empty() {
            let mut xobjects = res.x_objects();
            for (name, r) in self.images.values() {
                xobjects.pair(Name(name.as_bytes()), *r);
            }
            xobjects.finish();
        }
        res.finish();
        self.put(resources, &chunk)?;

        let (catalog_id, tree_id, info_id) = (Ref::new(1), Ref::new(2), Ref::new(3));
        let signature = self.write_signature()?;
        let (mut annots, acroform) = self.write_fields(signature)?;
        // Every page's object first: links and bookmarks lead to them.
        let mut page_ids = Vec::with_capacity(self.pages.len());
        for number in 0..self.pages.len() {
            let tagged = self.tags.as_ref().and_then(|t| t.pages.get(number).copied());
            page_ids.push(match tagged {
                Some((id, _)) => id,
                None => self.next_id(),
            });
        }
        let link_annots = self.write_links(&page_ids)?;
        for (number, list) in link_annots {
            annots.entry(number).or_default().extend(list);
        }
        let (dests, outline) = self.write_destinations(&page_ids)?;
        let mut more_contents = std::mem::take(&mut self.more_contents);
        for (number, first) in std::mem::take(&mut self.pages).into_iter().enumerate() {
            let tagged = self.tags.as_ref().and_then(|t| t.pages.get(number).copied());
            let page_id = page_ids[number];
            let mut chunk = Chunk::new();
            let mut p = chunk.page(page_id);
            p.parent(tree_id).media_box(Rect::new(0.0, 0.0, self.page_width, self.page_height));
            if let Some((_, Some(_))) = tagged {
                p.struct_parents(number as i32).tab_order(pdf_writer::types::TabOrder::StructureOrder);
            }
            match more_contents.remove(&number) {
                None => {
                    p.contents(first);
                }
                Some(more) => {
                    p.contents_array(std::iter::once(first).chain(more));
                }
            }
            p.pair(Name(b"Resources"), resources);
            if let Some(list) = annots.get(&number).filter(|l| !l.is_empty()) {
                p.annotations(list.iter().copied());
            }
            p.finish();
            self.put(page_id, &chunk)?;
        }

        let mut chunk = Chunk::new();
        chunk.pages(tree_id).kids(page_ids.iter().copied()).count(page_ids.len() as i32);
        self.put(tree_id, &chunk)?;
        // The structure tree of a tagged PDF: the document, a section per page, and the way
        // back from each page's content.
        let mut structure = None;
        if let Some(tags) = self.tags.take() {
            use pdf_writer::types::StructRole;
            let mut chunk = Chunk::new();
            let mut e = chunk.struct_element(tags.document);
            e.kind(StructRole::Document).parent(tags.root);
            let mut children = e.children();
            for (_, page) in &tags.pages {
                if let Some((section, _)) = page {
                    children.struct_element(*section);
                }
            }
            children.finish();
            e.finish();
            self.put(tags.document, &chunk)?;
            let mut chunk = Chunk::new();
            let mut root = chunk.indirect(tags.root).start::<pdf_writer::writers::StructTreeRoot>();
            root.child(tags.document);
            let mut tree = root.parent_tree();
            let mut numbers = tree.nums();
            for (number, (_, page)) in tags.pages.iter().enumerate() {
                if let Some((_, back)) = page {
                    numbers.insert(number as i32, *back);
                }
            }
            for (key, element) in &self.link_parents {
                numbers.insert(*key, *element);
            }
            numbers.finish();
            tree.finish();
            let next = self.link_parents.last().map(|(k, _)| k + 1).unwrap_or(0);
            root.parent_tree_next_key((tags.pages.len() as i32).max(next));
            root.finish();
            self.put(tags.root, &chunk)?;
            structure = Some(tags.root);
        }
        // The metadata: the title where assistive software looks for it, and what the
        // document conforms to.
        let metadata = if structure.is_some() || self.options.pdfa.is_some() {
            let id = self.next_id();
            let packet = self.xmp(structure.is_some());
            let mut chunk = Chunk::new();
            chunk.metadata(id, packet.as_bytes());
            self.put(id, &chunk)?;
            Some(id)
        } else {
            None
        };
        // PDF/A: the colors are sRGB.
        let intent = if self.options.pdfa.is_some() {
            let id = self.next_id();
            let mut chunk = Chunk::new();
            chunk.icc_profile(id, SRGB_ICC).n(3).range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
            self.put(id, &chunk)?;
            Some(id)
        } else {
            None
        };
        let attached = self.write_attachments()?;
        let mut chunk = Chunk::new();
        let mut catalog = chunk.indirect(catalog_id).start::<pdf_writer::writers::Catalog>();
        catalog.pages(tree_id);
        if let Some(xmp) = metadata {
            catalog.metadata(xmp);
        }
        if let Some(root) = structure {
            catalog.pair(Name(b"StructTreeRoot"), root);
            catalog.mark_info().marked(true);
            catalog.viewer_preferences().display_doc_title(true);
            catalog.lang(TextStr(self.options.lang.as_deref().unwrap_or("en-US")));
        }
        if let Some(profile) = intent {
            let mut intents = catalog.output_intents();
            intents
                .push()
                .subtype(pdf_writer::types::OutputIntentSubtype::PDFA)
                .output_condition_identifier(TextStr("sRGB"))
                .registry_name(TextStr("http://www.color.org"))
                .info(TextStr("sRGB IEC61966-2.1"))
                .dest_output_profile(profile);
        }
        if !attached.is_empty() {
            catalog.insert(Name(b"AF")).array().items(attached.iter().map(|(_, id)| *id));
        }
        if let Some(form) = acroform {
            catalog.pair(Name(b"AcroForm"), form);
        }
        if !dests.is_empty() || !attached.is_empty() {
            let mut names = catalog.names();
            if !dests.is_empty() {
                let mut tree = names.destinations();
                let mut entries = tree.names();
                for (name, id) in &dests {
                    entries.insert(Str(name.as_bytes()), *id);
                }
            }
            if !attached.is_empty() {
                let mut tree = names.embedded_files();
                let mut entries = tree.names();
                let mut sorted = attached.clone();
                sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                for (name, id) in &sorted {
                    entries.insert(Str(name.as_bytes()), *id);
                }
            }
        }
        if let Some(outline) = outline {
            catalog.outlines(outline);
            catalog.page_mode(pdf_writer::types::PageMode::UseOutlines);
        }
        catalog.finish();
        self.put(catalog_id, &chunk)?;
        let mut chunk = Chunk::new();
        let mut info = chunk.indirect(info_id).start::<pdf_writer::writers::DocumentInfo>();
        info.producer(TextStr(concat!("ferroreport ", env!("CARGO_PKG_VERSION"))));
        if let Some(t) = &self.options.title {
            info.title(TextStr(t));
        }
        info.finish();
        self.put(info_id, &chunk)?;

        // The encryption dictionary: itself not encrypted.
        let encrypt = match self.crypt.take() {
            Some(crypt) => {
                let id = self.next_id();
                let mut object = format!("{} 0 obj\n", id.get()).into_bytes();
                object.extend(crypt.dictionary());
                object.extend_from_slice(b"\nendobj\n");
                self.offsets[id.get() as usize - 1] = self.pos;
                self.out.write_all(&object).map_err(io)?;
                self.pos += object.len() as u64;
                format!("  /Encrypt {} 0 R\n", id.get())
            }
            None => String::new(),
        };

        // Cross-reference table and trailer.
        let mut end = Vec::with_capacity(self.offsets.len() * 20 + 128);
        end.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f\r\n", self.offsets.len() + 1).as_bytes());
        for offset in &self.offsets {
            end.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
        }
        end.extend_from_slice(
            format!(
                "trailer\n<<\n  /Size {}\n  /Root 1 0 R\n  /Info 3 0 R\n  /ID [<{id}> <{id}>]\n{encrypt}>>\nstartxref\n{}\n%%EOF",
                self.offsets.len() + 1,
                self.pos,
                id = self.file_id()
            )
            .as_bytes(),
        );
        self.out.write_all(&end).map_err(io)?;
        self.out.flush().map_err(io)?;
        Ok(self.out)
    }
}

impl<W: std::io::Write> crate::fill::PageSink for PdfStream<W> {
    /// Not in a tagged PDF: there a page is written with its structure, so it waits for
    /// the values it is missing.
    fn accepts_late(&self) -> bool {
        self.tags.is_none()
    }

    /// Tagging needs to know where each text element and picture begins and ends.
    fn wants_cells(&self) -> bool {
        self.tags.is_some()
    }

    fn page(&mut self, page: crate::print::PrintPage, fonts: &FontSet) -> Result<()> {
        PdfStream::page(self, &page.items, fonts)
    }

    fn late(&mut self, page: usize, item: PrintItem, fonts: &FontSet) -> Result<()> {
        PdfStream::late(self, page, std::slice::from_ref(&item), fonts)
    }
}

/// Serialises a filled document to PDF bytes.
pub fn write_pdf(doc: &PrintDocument, fonts: &FontSet, options: &PdfOptions) -> Result<Vec<u8>> {
    let mut pdf = PdfStream::new(Vec::new(), doc.page_width, doc.page_height, options.clone())?;
    for page in &doc.pages {
        pdf.page(&page.items, fonts)?;
    }
    let mut bytes = pdf.finish(fonts)?;
    if let Some(signature) = &options.signature {
        crate::sign::apply(&mut bytes, signature)?;
    }
    Ok(bytes)
}

/// Whether an RGB image is a chart bitmap whose transparent pixels a color key can mask:
/// transparent pixels black, opaque ones not.
fn keyed(id: usize, samples: &[u8], alpha: &[u8]) -> bool {
    id >= crate::image::KEYED_IDS
        && alpha.iter().zip(samples.chunks(3)).all(|(&a, c)| match a {
            0 => c == [0, 0, 0],
            255 => c != [0, 0, 0],
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    /// The decompressed streams of a PDF, as text.
    fn streams(pdf: &[u8]) -> String {
        let mut out = String::new();
        let mut rest = pdf;
        while let Some(i) = rest.windows(7).position(|w| w == b"stream\n") {
            let body = &rest[i + 7..];
            let end = body.windows(9).position(|w| w == b"endstream").unwrap_or(body.len());
            match miniz_oxide::inflate::decompress_to_vec_zlib(&body[..end]) {
                Ok(data) => out.push_str(&String::from_utf8_lossy(&data)),
                Err(_) => out.push_str(&String::from_utf8_lossy(&body[..end])),
            }
            rest = &body[end..];
        }
        out
    }

    fn render(text: &str, font: &str, fonts: &[(&str, Vec<u8>)]) -> String {
        let template = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<jasperReport xmlns="http://jasperreports.sourceforge.net/jasperreports" name="t" whenNoDataType="AllSectionsNoDetail" pageWidth="300" pageHeight="100" columnWidth="280" leftMargin="10" rightMargin="10" topMargin="10" bottomMargin="10">
<title><band height="40"><staticText><reportElement x="0" y="0" width="280" height="40"/><textElement><font fontName="{font}" size="12"/></textElement><text><![CDATA[{text}]]></text></staticText></band></title>
</jasperReport>"#
        );
        let mut r = crate::Renderer::new();
        for (family, data) in fonts {
            r.register_font(family, false, false, data.clone()).unwrap();
        }
        let report = r.parse(&template).unwrap();
        let doc = r.fill(&report, &[], Default::default()).unwrap();
        streams(&r.to_pdf(&doc).unwrap())
    }

    #[test]
    fn right_to_left_glyphs_map_back_to_their_characters() {
        let s = render("لا سلام", "DejaVu Sans", &[]);
        // Contextual forms map to the letters themselves; the lam-alef ligature to both
        // letters, as drawn from left to right (alef, lam), which readers reverse.
        assert!(s.contains("<0627 0644>") || s.contains("<06270644>"), "{s}");
        assert!(s.contains("<0633>") && s.contains("<0645>"));
        // Readers order right-to-left text themselves: no actual text for it.
        assert!(!s.contains("ActualText"));
    }

    #[test]
    fn reordered_clusters_carry_their_text() {
        let Ok(font) = std::fs::read("/usr/share/fonts/truetype/freefont/FreeSerif.ttf") else {
            eprintln!("FreeSerif is not installed: skipped");
            return;
        };
        let s = render("हिन्दी", "FreeSerif", &[("FreeSerif", font)]);
        // The vowel sign is drawn before its consonant: the span says what is written.
        assert!(s.contains("/ActualText"), "{s}");
    }
}

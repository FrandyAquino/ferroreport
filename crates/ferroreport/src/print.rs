//! The filled document: absolutely positioned primitives, ready for any exporter.
//!
//! Coordinates are in points with the origin at the top-left corner of the page.

use crate::fonts::FaceId;
use crate::image::ImageData;
use crate::model::{Color, Pen};
use std::sync::Arc;

/// One line of a printed text. New fields may be added in minor versions, so build it inside
/// the crate only; code outside reads it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PrintLine {
    pub text: String,
    /// Left edge of the line.
    pub x: f32,
    /// Baseline position.
    pub baseline: f32,
    pub width: f32,
    /// Extra space added after each space character (justified text).
    pub word_spacing: f32,
    /// The line as shaped glyphs in visual order, for text that is right to left or in a
    /// script that needs shaping (`None`: one glyph per character, left to right).
    pub shaped: Option<Arc<crate::shape::ShapedLine>>,
}

#[derive(Debug, Clone)]
pub struct PrintText {
    pub face: FaceId,
    pub size: f32,
    pub color: Color,
    pub underline: bool,
    pub strike: bool,
    pub lines: Vec<PrintLine>,
    /// The PDF standard font (an index into `standard::STANDARD`) to write the text in instead
    /// of `face`, as JasperReports does for families it does not have.
    pub standard: Option<crate::standard::Standard>,
    /// How the lines are aligned, so that a standard font, whose widths differ from the
    /// face's, is aligned the same way.
    pub halign: crate::model::HAlign,
}

/// What spreadsheet and text exporters need to know about a printed text element: its box,
/// its text, the number behind it, and its style. PDF ignores it and draws the wrapped item.
#[derive(Debug, Clone)]
pub struct CellInfo {
    /// What the text is in a tagged PDF, when it is more than a paragraph (a heading).
    pub tag: Option<crate::model::Tag>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    /// The styled runs of a styled or HTML text (empty for plain text).
    pub runs: Vec<crate::rich::Run>,
    /// The value when it is a number (exported as a number when type detection is on).
    pub number: Option<f64>,
    /// The value when it is a date (`java.util.Date` and its `java.sql` subclasses), in the
    /// report time zone.
    pub date: Option<chrono::NaiveDateTime>,
    /// The Java format pattern of the value (`#,##0.00`, `dd/MM/yyyy`).
    pub pattern: Option<String>,
    pub font: crate::model::FontSpec,
    pub color: Color,
    /// Background, for opaque elements.
    pub back: Option<Color>,
    pub halign: crate::model::HAlign,
    pub valign: crate::model::VAlign,
    pub rotation: crate::model::Rotation,
    pub boxed: crate::model::BoxStyle,
    /// The PDF form field the element is written as, instead of its text.
    pub field: Option<Arc<crate::model::FormField>>,
}

/// What a paragraph of a text with HTML lists is: how deep in lists it is (0: not in one),
/// and whether it starts a list item (it has the item's bullet or number, drawn first).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParaKind {
    pub level: u8,
    pub item: bool,
}

/// What a non-text element is, for spreadsheet exporters.
#[derive(Debug, Clone)]
pub enum Shape {
    /// A frame, subreport, table or list: its children follow it.
    Frame,
    Rectangle(Option<Pen>),
    Ellipse(Option<Pen>),
    /// A line; `top_down` is its direction when it is neither horizontal nor vertical.
    Line {
        pen: Pen,
        top_down: bool,
    },
    /// An image from the report (`None` when it is blank).
    Image {
        data: Option<Arc<ImageData>>,
        scale: crate::model::ScaleImage,
        halign: crate::model::HAlign,
        valign: crate::model::VAlign,
    },
    /// A chart, or a barcode with the size of its symbol (in its own units; JasperReports
    /// keeps the symbol's shape when it places the barcode picture in a spreadsheet).
    Picture {
        symbol: Option<(f64, f64)>,
    },
}

/// A non-text element as spreadsheet exporters see it. It marks the start of the items that
/// draw the element: the `len` items after it belong to it (for a frame, its children).
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub shape: Shape,
    /// Background, for opaque elements.
    pub back: Option<Color>,
    pub fore: Color,
    pub boxed: Option<crate::model::BoxStyle>,
    pub len: usize,
    /// What a picture shows, in words (an image's tooltip), for tagged PDFs.
    pub alt: Option<String>,
    /// What the element is in a tagged PDF (a table, one of its cells).
    pub tag: Option<crate::model::Tag>,
}

/// Where a link, an anchor or a bookmark sits on the page (the element's box).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkInfo {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Where a click leads.
    pub target: Option<LinkTarget>,
    /// What the link says when pointed at (its tooltip).
    pub tooltip: Option<String>,
    /// The anchor the element is: links and bookmarks lead here.
    pub anchor: Option<String>,
    /// The level of the element's bookmark (0: none).
    pub bookmark: u32,
}

/// Where a link leads.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    Url(String),
    Anchor(String),
    /// A page of this document, from 1.
    Page(u32),
    RemoteAnchor {
        file: String,
        anchor: String,
    },
    RemotePage {
        file: String,
        page: u32,
    },
}

#[derive(Debug, Clone)]
pub enum PrintItem {
    /// A link, anchor or bookmark: what the next items draw leads somewhere. Draws nothing.
    Link(Arc<LinkInfo>),
    /// A printed text element: the item drawing it, with the data exporters other than PDF use.
    Cell(Arc<CellInfo>, Box<PrintItem>),
    /// Where a non-text element starts, for exporters other than PDF. Draws nothing.
    Element(Arc<ElementInfo>),
    /// A path in page coordinates (charts), filled with the nonzero rule and/or stroked. A path
    /// that neither fills nor strokes clips the items after it inside the same `Transform`
    /// (charts clip their data to the plot); elsewhere it draws nothing.
    Path {
        ops: Arc<Vec<PathOp>>,
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
        dash: Option<[f32; 2]>,
    },
    /// Several items drawn in order (the runs of a styled text).
    Group(Vec<PrintItem>),
    /// The paragraphs of a text with HTML lists, one item each, and what each paragraph is:
    /// drawn like a group; tagged PDFs make lists of them.
    Paragraphs(Arc<Vec<ParaKind>>, Vec<PrintItem>),
    /// Items drawn in a local coordinate system: a local point `(u, v)` lands on the page at
    /// `(a·u + c·v + e, b·u + d·v + f)` (points, y down). Used for rotated text.
    Transform([f32; 6], Vec<PrintItem>),
    /// Filled rectangles drawn in a local drawing scaled by `scale` from `(x, y)` (barcodes).
    Shapes {
        x: f32,
        y: f32,
        scale: f64,
        color: Color,
        rects: Arc<Vec<[f64; 4]>>,
        /// Outlines (glyphs of human-readable text), filled with the nonzero rule.
        path: Arc<Vec<PathOp>>,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Color>,
        stroke: Option<Pen>,
        radius: f32,
        /// Written as a path like iText's `roundRectangle` (rectangle elements) rather than
        /// with `re` (backgrounds), as JasperReports does: PDF viewers anti-alias the two
        /// differently.
        path: bool,
    },
    Ellipse {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Color>,
        stroke: Option<Pen>,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        pen: Pen,
    },
    Text(PrintText),
    /// An image drawn into `x, y, w, h`, optionally clipped to `clip` (x, y, w, h).
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        clip: Option<[f32; 4]>,
        image: Arc<ImageData>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct PrintPage {
    pub items: Vec<PrintItem>,
}

#[derive(Debug, Clone)]
pub struct PrintDocument {
    pub name: String,
    pub page_width: f32,
    pub page_height: f32,
    /// The report's orientation is landscape.
    pub landscape: bool,
    /// What the template asks of PDF output (see [`crate::Report`]).
    pub pdf: crate::model::PdfSettings,
    pub pages: Vec<PrintPage>,
}

/// One step of an outline, in the coordinates of a [`PrintItem::Shapes`] drawing.
#[derive(Debug, Clone, Copy)]
pub enum PathOp {
    Move(f64, f64),
    Line(f64, f64),
    /// A quadratic segment, which JasperReports' PDF exporter (iText's `PdfGraphics2D`)
    /// writes as a cubic whose first control point is the current point (`v`).
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

/// A horizontal slice of filled content (one band of a subreport), positioned relative to
/// its own top edge.
#[derive(Debug, Clone)]
pub struct Segment {
    pub height: f32,
    pub items: Vec<PrintItem>,
    /// Page or column header: printed again at the top of each page the content continues on.
    pub header: bool,
    /// The page footer for pages the content continues past: not part of the flow, printed
    /// after the last band that fits on each such page.
    pub footer: bool,
}

impl Segment {
    /// Height of content made of `segments`, leaving out page-footer templates.
    pub fn flow_height(segments: &[Segment]) -> f32 {
        segments.iter().filter(|s| !s.footer).map(|s| s.height).sum()
    }
}

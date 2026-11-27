//! The rules JFreeChart lays a chart out by, as JasperReports 7 uses it (JFreeChart 1.5,
//! JasperReports' `DefaultChartTheme`), measured on the charts JasperReports draws and read
//! from JFreeChart's classes.
//!
//! JFreeChart measures text with AWT's integer font metrics: a string is as wide as the sum
//! of its glyph advances rounded to whole points, and as tall as the rounded-up ascent plus
//! the rounded-up descent. Where a text goes depends on those numbers, and on the font's
//! exact ascent and descent where it is anchored by its baseline. The glyphs themselves are
//! drawn with their exact advances.

use super::Canvas;
use crate::model::{Color, FontSpec};
use crate::print::PathOp;

/// A rectangle in points, y down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }
    pub fn center_x(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn center_y(&self) -> f64 {
        self.y + self.h / 2.0
    }
    /// `RectangleInsets.trim`.
    pub fn trim(&self, top: f64, left: f64, bottom: f64, right: f64) -> Rect {
        Rect { x: self.x + left, y: self.y + top, w: self.w - left - right, h: self.h - top - bottom }
    }
    /// CategoryPlot's and XYPlot's `integerise`: the whole points inside.
    pub fn integerise(&self) -> Rect {
        let (x0, y0) = (self.x.ceil(), self.y.ceil());
        let (x1, y1) = (self.max_x().floor(), self.max_y().floor());
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

/// Which side of the plot an axis or a title is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    pub fn vertical(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

/// A font's metrics as AWT gives them: exact (`LineMetrics`) and rounded (`FontMetrics`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Metrics {
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    /// `FontMetrics.getHeight()`: ascent and descent (with leading) each rounded up.
    pub height: f64,
}

impl Canvas<'_> {
    pub(crate) fn metrics(&mut self, font: &FontSpec) -> Metrics {
        let id = self.face(font);
        let face = self.fonts.face(id);
        // AWT computes these in float.
        let ascent = face.ascent(font.size) as f64;
        let descent = face.descent(font.size) as f64;
        let leading = (face.line_height(font.size) - face.ascent(font.size) - face.descent(font.size)).max(0.0) as f64;
        // FontDesignMetrics: values within 0.05 of the next whole number are rounded up.
        let up = |v: f64| (0.95f32 + v as f32).floor() as f64;
        Metrics { ascent, descent, leading, height: up(ascent) + up(descent + leading) }
    }

    /// `FontMetrics.stringWidth`. Drawing SVG (Batik's graphics), the glyph advances rounded
    /// to whole points, added up; drawing on the PDF page (OpenPDF's graphics measure with
    /// fractional metrics), the exact advances added up, then rounded.
    pub(crate) fn awt_width(&mut self, font: &FontSpec, text: &str) -> f64 {
        let id = self.face(font);
        let face = self.fonts.face(id);
        let k = font.size / face.units_per_em;
        let advances = text.chars().filter(|c| !c.is_control()).map(|c| face.advance(face.glyph(c)) * k);
        if self.svg {
            advances.map(|a| (a + 0.5).floor() as f64).sum()
        } else {
            (advances.sum::<f32>() + 0.5).floor() as f64
        }
    }

    /// Text with its baseline starting at `(x, baseline)`, glyphs at their exact advances.
    pub(crate) fn draw_string(&mut self, font: &FontSpec, color: Color, text: &str, x: f64, baseline: f64) {
        self.text_baseline(font, color, text, x as f32, baseline as f32);
    }

    /// A filled polygon.
    pub(crate) fn fill_poly(&mut self, pts: &[(f64, f64)], color: Color) {
        if pts.is_empty() {
            return;
        }
        let mut ops = vec![PathOp::Move(pts[0].0, pts[0].1)];
        ops.extend(pts[1..].iter().map(|p| PathOp::Line(p.0, p.1)));
        ops.push(PathOp::Close);
        self.path(ops, Some(color), None, None);
    }

    /// A filled rectangle, written as a path as Java2D's PDF graphics write it.
    pub(crate) fn fill_rect(&mut self, r: Rect, color: Color) {
        self.fill_poly(&[(r.x, r.y), (r.max_x(), r.y), (r.max_x(), r.max_y()), (r.x, r.max_y())], color);
    }

    /// A line with square caps (Java2D's default stroke): the ends reach half the width
    /// further. Lines of no length draw nothing.
    pub(crate) fn line_square(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, color: Color, width: f64) {
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            return;
        }
        let (ux, uy) = (dx / len * width / 2.0, dy / len * width / 2.0);
        self.path(vec![PathOp::Move(x1 - ux, y1 - uy), PathOp::Line(x2 + ux, y2 + uy)], None, Some((color, width as f32)), None);
    }

    /// The outline of a closed shape (`Graphics2D.draw`).
    pub(crate) fn stroke_closed(&mut self, mut ops: Vec<PathOp>, color: Color, width: f64) {
        if !matches!(ops.last(), Some(PathOp::Close)) {
            ops.push(PathOp::Close);
        }
        self.path(ops, None, Some((color, width as f32)), None);
    }
}

// ---------------------------------------------------------------------------------------------
// Text anchors (TextUtils).

/// Where a text's anchor point is: horizontally (left, center, right) and vertically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum VAnchor {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum HAnchor {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TextAnchor(pub HAnchor, pub VAnchor);

pub(crate) const CENTER: TextAnchor = TextAnchor(HAnchor::Center, VAnchor::Center);
pub(crate) const CENTER_RIGHT: TextAnchor = TextAnchor(HAnchor::Right, VAnchor::Center);
pub(crate) const CENTER_LEFT: TextAnchor = TextAnchor(HAnchor::Left, VAnchor::Center);
pub(crate) const TOP_CENTER: TextAnchor = TextAnchor(HAnchor::Center, VAnchor::Top);
pub(crate) const BOTTOM_CENTER: TextAnchor = TextAnchor(HAnchor::Center, VAnchor::Bottom);

/// `TextUtils.deriveTextBoundsAnchorOffsets`: from the anchor point to where the text starts.
fn text_offsets(cv: &mut Canvas, font: &FontSpec, text: &str, anchor: TextAnchor) -> (f64, f64) {
    let m = cv.metrics(font);
    let w = cv.awt_width(font, text);
    let h = m.height;
    let x = match anchor.0 {
        HAnchor::Left => 0.0,
        HAnchor::Center => -w / 2.0,
        HAnchor::Right => -w,
    };
    let y = match anchor.1 {
        VAnchor::Top => -m.descent - m.leading + h,
        VAnchor::Center => -m.descent - m.leading + h / 2.0,
        VAnchor::Bottom => -m.descent - m.leading,
    };
    // Java adds these as floats.
    (x as f32 as f64, y as f32 as f64)
}

/// `TextUtils.deriveRotationAnchorOffsets`: from where the text starts to the point it turns about.
fn rotation_offsets(cv: &mut Canvas, font: &FontSpec, text: &str, anchor: TextAnchor) -> (f64, f64) {
    let m = cv.metrics(font);
    let w = cv.awt_width(font, text);
    let h = m.height;
    let x = match anchor.0 {
        HAnchor::Left => 0.0,
        HAnchor::Center => w / 2.0,
        HAnchor::Right => w,
    };
    let y = match anchor.1 {
        VAnchor::Top => m.descent + m.leading - h,
        VAnchor::Center => m.descent + m.leading - h / 2.0,
        VAnchor::Bottom => m.descent + m.leading,
    };
    (x as f32 as f64, y as f32 as f64)
}

/// `TextUtils.drawAlignedString`.
pub(crate) fn draw_aligned(cv: &mut Canvas, font: &FontSpec, color: Color, text: &str, x: f64, y: f64, anchor: TextAnchor) {
    let (dx, dy) = text_offsets(cv, font, text, anchor);
    cv.draw_string(font, color, text, x + dx, y + dy);
}

/// `TextUtils.drawRotatedString(text, g2, x, y, textAnchor, angle, rotationAnchor)`.
pub(crate) fn draw_rotated(
    cv: &mut Canvas,
    font: &FontSpec,
    color: Color,
    text: &str,
    x: f64,
    y: f64,
    anchor: TextAnchor,
    angle: f64,
    rotation: TextAnchor,
) {
    let (dx, dy) = text_offsets(cv, font, text, anchor);
    if angle == 0.0 {
        cv.draw_string(font, color, text, x + dx, y + dy);
        return;
    }
    let (rx, ry) = rotation_offsets(cv, font, text, rotation);
    let (tx, ty) = (x + dx, y + dy);
    draw_string_rotated(cv, font, color, text, tx, ty, angle, tx + rx, ty + ry);
}

/// A string drawn at `(x, y)` turned by `angle` (radians, clockwise on the page) about `(px, py)`.
pub(crate) fn draw_string_rotated(
    cv: &mut Canvas,
    font: &FontSpec,
    color: Color,
    text: &str,
    x: f64,
    y: f64,
    angle: f64,
    px: f64,
    py: f64,
) {
    let (s, c) = angle.sin_cos();
    let mut inner = Canvas { fonts: cv.fonts, items: Vec::new(), svg: cv.svg, origin: cv.origin };
    inner.draw_string(font, color, text, 0.0, 0.0);
    let items = std::mem::take(&mut inner.items);
    // The point (x, y) turned about (px, py) is where local (0, 0) lands.
    let ox = px + (x - px) * c - (y - py) * s;
    let oy = py + (x - px) * s + (y - py) * c;
    cv.items.push(crate::print::PrintItem::Transform([c as f32, s as f32, -s as f32, c as f32, ox as f32, oy as f32], items));
}

// ---------------------------------------------------------------------------------------------
// Text blocks (TextUtils.createTextBlock, TextBlock).

/// Lines of text as JFreeChart breaks a label to fit a width.
#[derive(Debug, Clone, Default)]
pub(crate) struct TextBlock {
    pub lines: Vec<String>,
}

/// Where a text block is anchored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlockAnchor(pub HAnchor, pub VAnchor);

impl TextBlock {
    /// `TextUtils.createTextBlock(text, font, paint, maxWidth, maxLines, measurer)`.
    pub fn wrap(cv: &mut Canvas, font: &FontSpec, text: &str, max_width: f64, max_lines: usize) -> TextBlock {
        let chars: Vec<char> = text.chars().collect();
        let width = |cv: &mut Canvas, a: usize, b: usize| -> f64 {
            let s: String = chars[a..b].iter().collect();
            cv.awt_width(font, &s)
        };
        let breaks = line_breaks(&chars);
        let max_width = max_width as f32 as f64;
        let mut lines: Vec<String> = Vec::new();
        let mut current = 0;
        let len = chars.len();
        while current < len && lines.len() < max_lines {
            // nextLineBreak
            let newline = chars[current..].iter().position(|&c| c == '\n').map(|p| p + current).unwrap_or(usize::MAX);
            let mut x = 0.0f32;
            let mut first = true;
            let mut pos = current;
            let mut next: Option<usize> = None;
            while let Some(end) = breaks.iter().copied().find(|&b| b > pos) {
                x += width(cv, pos, end) as f32;
                if x as f64 > max_width {
                    if first {
                        let mut e = end;
                        loop {
                            if width(cv, current, e) <= max_width {
                                break;
                            }
                            e -= 1;
                            if e <= current {
                                break;
                            }
                        }
                        next = Some(e);
                    } else {
                        next = Some(pos);
                    }
                    break;
                } else if end > newline {
                    next = Some(newline);
                    break;
                }
                first = false;
                pos = end;
            }
            match next {
                None => {
                    lines.push(chars[current..].iter().collect());
                    return TextBlock { lines };
                }
                Some(mut n) => {
                    if n == current {
                        n += 1;
                    }
                    lines.push(chars[current..n].iter().collect());
                    current = n;
                    while current < len && chars[current] == '\n' {
                        current += 1;
                    }
                }
            }
        }
        if current < len {
            if let Some(last) = lines.last_mut() {
                let n = last.chars().count();
                *last = if n > 3 { last.chars().take(n - 3).collect::<String>() + "..." } else { "...".into() };
            }
        }
        TextBlock { lines }
    }

    /// `TextBlock.calculateDimensions`: the widest line, and a line height per line.
    pub fn size(&self, cv: &mut Canvas, font: &FontSpec) -> (f64, f64) {
        let h = cv.metrics(font).height;
        let w = self.lines.iter().map(|l| cv.awt_width(font, l)).fold(0.0, f64::max);
        (w, h * self.lines.len() as f64)
    }

    /// `TextBlock.draw` with no rotation: lines centered on each other.
    pub fn draw(&self, cv: &mut Canvas, font: &FontSpec, color: Color, x: f64, y: f64, anchor: BlockAnchor) {
        self.draw_rotated(cv, font, color, x, y, anchor, 0.0);
    }

    /// `TextBlock.draw(g2, x, y, anchor, x, y, angle)`: the block turned about its anchor.
    pub fn draw_rotated(&self, cv: &mut Canvas, font: &FontSpec, color: Color, x: f64, y: f64, anchor: BlockAnchor, angle: f64) {
        let (w, h) = self.size(cv, font);
        let (x, y) = (x as f32 as f64, y as f32 as f64);
        let ox = match anchor.0 {
            HAnchor::Left => 0.0,
            HAnchor::Center => -w / 2.0,
            HAnchor::Right => -w,
        };
        let oy = match anchor.1 {
            VAnchor::Top => 0.0,
            VAnchor::Center => -h / 2.0,
            VAnchor::Bottom => -h,
        };
        let m = cv.metrics(font);
        let mut cursor = 0.0f32;
        for line in &self.lines {
            let lw = cv.awt_width(font, line);
            let offset = ((w - lw) / 2.0) as f32;
            let lx = (x as f32 + ox as f32 + offset) as f64;
            let top = (y as f32 + oy as f32 + cursor) as f64;
            // TextLine.draw anchored TOP_LEFT: the baseline an ascent below.
            let baseline = (top as f32 + m.ascent as f32) as f64;
            if angle == 0.0 {
                cv.draw_string(font, color, line, lx, baseline);
            } else {
                draw_string_rotated(cv, font, color, line, lx, baseline, angle, x, y);
            }
            cursor += m.height as f32;
        }
    }
}

/// Where lines may break (java.text.BreakIterator.getLineInstance, roughly: after spaces and
/// hyphens), as offsets of the characters that may start a line, ending with the length.
fn line_breaks(chars: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    for i in 1..chars.len() {
        let (a, b) = (chars[i - 1], chars[i]);
        if (a.is_whitespace() && !b.is_whitespace())
            || (a == '-' && b.is_alphanumeric() && i >= 2 && chars[i - 2].is_alphanumeric())
            || a == '\n'
        {
            out.push(i);
        }
    }
    out.push(chars.len());
    out
}

// ---------------------------------------------------------------------------------------------
// Titles and the legend.

/// A text title (`TextTitle`): lines centered, 1 point of padding around.
pub(crate) fn text_title(cv: &mut Canvas, area: &mut Rect, text: &str, font: &FontSpec, color: Color, side: Side) {
    if text.is_empty() {
        return;
    }
    if side.vertical() {
        // Left and right titles read upwards / downwards; JFreeChart wraps them to the height.
        let block = TextBlock::wrap(cv, font, text, area.h - 2.0, usize::MAX);
        let (_, bh) = block.size(cv, font);
        let total = bh + 2.0;
        let (rect, angle) = if side == Side::Left {
            let r = Rect::new(area.x, area.y, total, area.h);
            area.x += total;
            area.w -= total;
            (r, -std::f64::consts::FRAC_PI_2)
        } else {
            let r = Rect::new(area.max_x() - total, area.y, total, area.h);
            area.w -= total;
            (r, std::f64::consts::FRAC_PI_2)
        };
        let c = rect.trim(1.0, 1.0, 1.0, 1.0);
        let (x, y) = if side == Side::Left { (c.x, c.center_y()) } else { (c.max_x(), c.center_y()) };
        block.draw_rotated(cv, font, color, x, y, BlockAnchor(HAnchor::Center, VAnchor::Top), angle);
        return;
    }
    let block = TextBlock::wrap(cv, font, text, area.w - 2.0, usize::MAX);
    let (_, bh) = block.size(cv, font);
    let total = bh + 2.0;
    let rect = if side == Side::Top {
        let r = Rect::new(area.x, area.y, area.w, total);
        area.y += total;
        area.h -= total;
        r
    } else {
        let r = Rect::new(area.x, area.max_y() - total, area.w, total);
        area.h -= total;
        r
    };
    let c = rect.trim(1.0, 1.0, 1.0, 1.0);
    if side == Side::Top {
        block.draw(cv, font, color, c.center_x(), c.y, BlockAnchor(HAnchor::Center, VAnchor::Top));
    } else {
        block.draw(cv, font, color, c.center_x(), c.max_y(), BlockAnchor(HAnchor::Center, VAnchor::Bottom));
    }
}

/// What a legend item shows beside its label.
#[derive(Debug, Clone)]
pub(crate) struct LegendGraphic {
    /// A filled shape (centered on the origin), with its outline color and width.
    pub shape: Option<(Vec<PathOp>, (f64, f64, f64, f64), Option<(Color, f64)>)>,
    /// A line from (-7, 0) to (7, 0) in the series color.
    pub line: Option<f64>,
    pub color: Color,
}

impl LegendGraphic {
    /// The 8 by 8 square bar and area charts show.
    pub fn square(color: Color, outline: Option<(Color, f64)>) -> LegendGraphic {
        LegendGraphic { shape: Some((rect_ops(-4.0, -4.0, 8.0, 8.0), (-4.0, -4.0, 8.0, 8.0), outline)), line: None, color }
    }

    /// Size of the shape and line together.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        let mut b: Option<(f64, f64, f64, f64)> = None;
        let mut add = |r: (f64, f64, f64, f64)| {
            b = Some(match b {
                None => r,
                Some(o) => {
                    let x0 = o.0.min(r.0);
                    let y0 = o.1.min(r.1);
                    let x1 = (o.0 + o.2).max(r.0 + r.2);
                    let y1 = (o.1 + o.3).max(r.1 + r.3);
                    (x0, y0, x1 - x0, y1 - y0)
                }
            })
        };
        if let Some((_, r, _)) = &self.shape {
            add(*r);
        }
        if self.line.is_some() {
            add((-7.0, 0.0, 14.0, 0.0));
        }
        b.unwrap_or((0.0, 0.0, 0.0, 0.0))
    }
}

pub(crate) fn rect_ops(x: f64, y: f64, w: f64, h: f64) -> Vec<PathOp> {
    vec![
        PathOp::Move(x, y),
        PathOp::Line(x + w, y),
        PathOp::Line(x + w, y + h),
        PathOp::Line(x, y + h),
        PathOp::Line(x, y),
        PathOp::Close,
    ]
}

/// Moves a path by `(dx, dy)`.
pub(crate) fn translate(ops: &[PathOp], dx: f64, dy: f64) -> Vec<PathOp> {
    ops.iter()
        .map(|op| match *op {
            PathOp::Move(x, y) => PathOp::Move(x + dx, y + dy),
            PathOp::Line(x, y) => PathOp::Line(x + dx, y + dy),
            PathOp::Quad(a, b, x, y) => PathOp::Quad(a + dx, b + dy, x + dx, y + dy),
            PathOp::Cubic(a, b, c, d, x, y) => PathOp::Cubic(a + dx, b + dy, c + dx, d + dy, x + dx, y + dy),
            PathOp::Close => PathOp::Close,
        })
        .collect()
}

pub(crate) struct LegendItem {
    pub label: String,
    pub graphic: LegendGraphic,
}

/// The legend (`LegendTitle`, margin 1 and padding 1 around, items 2 apart) on `side` of
/// `area`, which it takes its room from.
pub(crate) fn legend(
    cv: &mut Canvas,
    area: &mut Rect,
    items: &[LegendItem],
    font: &FontSpec,
    color: Color,
    background: Option<Color>,
    side: Side,
) {
    if items.is_empty() {
        return;
    }
    let h_text = cv.metrics(font).height;
    // Each item: the graphic (2 of padding around) left of the label (2 of padding around).
    let sizes: Vec<(f64, f64, f64)> = items
        .iter()
        .map(|it| {
            let b = it.graphic.bounds();
            let (gw, gh) = (b.2 + 4.0, b.3 + 4.0);
            let lw = cv.awt_width(font, &it.label) + 4.0;
            let lh = h_text + 4.0;
            (gw + lw, gw, gh.max(lh))
        })
        .collect();
    // Positions within the content area, and its size.
    let mut placed: Vec<(f64, f64)> = Vec::new();
    let (cw, ch);
    if !side.vertical() {
        // FlowArrangement: one row if it fits, otherwise rows as wide as the area.
        let avail = area.w - 4.0;
        let natural: f64 = sizes.iter().map(|s| s.0).sum::<f64>() + 2.0 * (sizes.len() as f64 - 1.0);
        if natural <= avail {
            let mut x = 0.0;
            for s in &sizes {
                placed.push((x, 0.0));
                x += s.0 + 2.0;
            }
            cw = natural;
            ch = sizes.iter().map(|s| s.2).fold(0.0, f64::max);
        } else {
            let (mut x, mut y, mut row_h) = (0.0, 0.0, 0.0f64);
            let mut row_items = 0;
            for s in &sizes {
                if x + s.0 <= avail {
                    placed.push((x, y));
                    x += s.0 + 2.0;
                    row_h = row_h.max(s.2);
                    row_items += 1;
                } else if row_items == 0 {
                    placed.push((x, y));
                    x = 0.0;
                    y += s.2 + 2.0;
                } else {
                    y += row_h + 2.0;
                    row_h = s.2;
                    placed.push((0.0, y));
                    x = s.0 + 2.0;
                    row_items = 1;
                }
            }
            cw = avail;
            ch = y + row_h;
        }
    } else {
        // ColumnArrangement: items under each other, centered.
        let mut y = 0.0;
        let widest = sizes.iter().map(|s| s.0).fold(0.0, f64::max);
        for s in &sizes {
            placed.push(((widest - s.0) / 2.0, y));
            y += s.2;
        }
        cw = widest;
        ch = y;
    }
    let (tw, th) = (cw + 4.0, ch + 4.0);
    let rect = match side {
        Side::Top => {
            let r = Rect::new(area.center_x() - tw / 2.0, area.y, tw, th);
            area.y += th;
            area.h -= th;
            r
        }
        Side::Bottom => {
            let r = Rect::new(area.center_x() - tw / 2.0, area.max_y() - th, tw, th);
            area.h -= th;
            r
        }
        Side::Left => {
            let r = Rect::new(area.x, area.center_y() - th / 2.0, tw, th);
            area.x += tw;
            area.w -= tw;
            r
        }
        Side::Right => {
            let r = Rect::new(area.max_x() - tw, area.center_y() - th / 2.0, tw, th);
            area.w -= tw;
            r
        }
    };
    if let Some(bg) = background {
        cv.fill_rect(rect.trim(1.0, 1.0, 1.0, 1.0), bg);
    }
    let content = rect.trim(2.0, 2.0, 2.0, 2.0);
    let m = cv.metrics(font);
    for ((item, &(px, py)), &(iw, gw, ih)) in items.iter().zip(&placed).zip(&sizes) {
        let (ix, iy) = (content.x + px, content.y + py);
        // The graphic, centered in its block.
        let g = &item.graphic;
        let b = g.bounds();
        let (cx, cy) = (ix + gw / 2.0, iy + ih / 2.0);
        if let Some(lw) = g.line {
            let (x0, x1) = (cx - 7.0, cx + 7.0);
            cv.line_square(x0, cy, x1, cy, g.color, lw);
        }
        if let Some((ops, r, outline)) = &g.shape {
            // The shape's bounds centered on the point.
            let (dx, dy) = (cx - (r.0 + r.2 / 2.0), cy - (r.1 + r.3 / 2.0));
            let moved = translate(ops, dx, dy);
            cv.path(moved.clone(), Some(g.color), None, None);
            if let Some((oc, ow)) = outline {
                cv.stroke_closed(moved, *oc, *ow);
            }
        }
        let _ = b;
        // The label, centered in what is left of the item.
        let label_x = ix + gw + 2.0;
        let label_h = ih - 4.0;
        let top = (iy + 2.0) + label_h / 2.0 - m.height / 2.0;
        let baseline = (top as f32 + m.ascent as f32) as f64;
        let _ = iw;
        cv.draw_string(font, color, &item.label, label_x, baseline);
    }
}

// ---------------------------------------------------------------------------------------------
// Number axes.

/// JasperReports' standard tick units (`ChartUtil.createStandardTickUnits`): 1, 2.5 and 5
/// times powers of ten, each with its label pattern.
pub(crate) fn standard_units() -> &'static [(f64, &'static str)] {
    use std::sync::OnceLock;
    static UNITS: OnceLock<Vec<(f64, &'static str)>> = OnceLock::new();
    UNITS.get_or_init(|| {
        const DECIMALS: [&str; 9] = ["#,##0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000000", "0.0000000", "0.00000000"];
        let mut out = Vec::new();
        for e in -7i32..=18 {
            for m in ["1", "2.5", "5"] {
                let size: f64 = format!("{m}E{e}").parse().unwrap_or(1.0);
                let pattern = if e >= 0 {
                    if m == "2.5" && e == 0 {
                        "0.0"
                    } else {
                        "#,##0"
                    }
                } else {
                    let d = (-e) as usize + usize::from(m == "2.5");
                    DECIMALS[d.min(8)]
                };
                if e >= 12 || e <= 11 {
                    out.push((size, pattern));
                }
            }
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out
    })
}

/// JasperReports' integer tick units (`ChartUtil.createIntegerTickUnits`, which thermometers
/// use): 1, 2 and 5 times powers of ten up to 5E18, labeled `0` below 1,000 and `#,##0` from
/// there.
pub(crate) fn integer_units() -> &'static [(f64, &'static str)] {
    use std::sync::OnceLock;
    static UNITS: OnceLock<Vec<(f64, &'static str)>> = OnceLock::new();
    UNITS.get_or_init(|| {
        let mut out = Vec::new();
        for e in 0..=18 {
            for m in [1.0, 2.0, 5.0] {
                out.push((m * 10f64.powi(e), if e < 3 { "0" } else { "#,##0" }));
            }
        }
        out
    })
}

/// `TickUnits.getCeilingTickUnit(size)`: the smallest of `units` at least `size`.
fn ceiling_unit(units: &[(f64, &'static str)], size: f64) -> usize {
    units.iter().position(|u| u.0 >= size).unwrap_or(units.len() - 1)
}

/// A number axis (`NumberAxis` with JasperReports' tick units).
#[derive(Debug, Clone)]
pub(crate) struct NumberAxis {
    pub lower: f64,
    pub upper: f64,
    /// The current tick unit: an index into the standard units, or a fixed unit.
    unit: f64,
    unit_pattern: &'static str,
    /// The units ticks are chosen from (`setStandardTickUnits`).
    units: &'static [(f64, &'static str)],
    /// A pattern for every label (`tickLabelMask`).
    pub format: Option<String>,
    pub font: FontSpec,
    pub label: Option<String>,
    pub label_font: FontSpec,
    pub label_color: Color,
    pub tick_label_color: Color,
    pub line_color: Color,
    /// The axis line (`setAxisLineVisible`); tick marks are drawn either way.
    pub line_visible: bool,
    pub tick_marks: bool,
    pub tick_labels: bool,
    /// Where the last `draw` left the cursor (past the tick labels and the label).
    pub end_cursor: f64,
    pub inverted: bool,
}

impl NumberAxis {
    pub fn new(lower: f64, upper: f64, font: FontSpec) -> NumberAxis {
        NumberAxis {
            lower,
            upper,
            unit: 1.0,
            unit_pattern: "0",
            units: standard_units(),
            format: None,
            label_font: font.clone(),
            font,
            label: None,
            label_color: Color::BLACK,
            tick_label_color: Color::BLACK,
            line_color: Color::BLACK,
            line_visible: true,
            tick_marks: true,
            tick_labels: true,
            end_cursor: 0.0,
            inverted: false,
        }
    }

    /// Ticks chosen from JasperReports' integer tick units instead of its standard ones.
    pub fn integer_units(mut self) -> NumberAxis {
        self.units = integer_units();
        self
    }

    /// `NumberAxis.autoAdjustRange` over data from `min` to `max`: 5% margins, sticking to
    /// zero, and zero included when `include_zero`.
    pub fn auto_range(min: f64, max: f64, include_zero: bool) -> (f64, f64) {
        let (mut lower, mut upper) = if min.is_finite() && max.is_finite() { (min, max) } else { (0.0, 1.0) };
        if include_zero {
            lower = lower.min(0.0);
            upper = upper.max(0.0);
        }
        let mut range = upper - lower;
        let min_range = 1.0e-8;
        if range < min_range {
            let expand = (min_range - range) / 2.0;
            upper += expand;
            lower -= expand;
            if lower == upper {
                let adjust = lower.abs() / 10.0;
                lower -= adjust;
                upper += adjust;
            }
            range = upper - lower;
        }
        // Sticky zero.
        upper = if upper <= 0.0 { (upper + 0.05 * range).min(0.0) } else { upper + 0.05 * range };
        lower = if lower >= 0.0 { (lower - 0.05 * range).max(0.0) } else { lower - 0.05 * range };
        (lower, upper)
    }

    /// Applies the bounds a template fixes (`rangeAxisMinValueExpression`...), then
    /// JasperReports' `ChartUtil.setAutoTickUnit`.
    pub fn fix(&mut self, min: Option<f64>, max: Option<f64>) {
        if let Some(v) = min {
            self.lower = v;
        }
        if let Some(v) = max {
            self.upper = v;
        }
        let length = self.upper - self.lower;
        if length >= 1.0e12 {
            let i = ceiling_unit(self.units, length / 1.0e12);
            self.set_unit(i);
        }
    }

    fn set_unit(&mut self, i: usize) {
        let (u, p) = self.units[i];
        self.unit = u;
        self.unit_pattern = p;
    }

    /// Position of `v` in `area` along the side the axis is on.
    pub fn java2d(&self, v: f64, area: Rect, side: Side) -> f64 {
        let (min, max) = if side.vertical() { (area.max_y(), area.y) } else { (area.x, area.max_x()) };
        let t = (v - self.lower) / (self.upper - self.lower);
        if self.inverted {
            max - t * (max - min)
        } else {
            min + t * (max - min)
        }
    }

    fn length_to_java2d(&self, length: f64, area: Rect, side: Side) -> f64 {
        (self.java2d(length, area, side) - self.java2d(0.0, area, side)).abs()
    }

    pub fn label_of(&self, v: f64, locale: &crate::format::Locale) -> String {
        let pattern = self.format.as_deref().unwrap_or(self.unit_pattern);
        crate::format::format_value(&crate::value::Value::Double(v), Some(pattern), locale)
    }

    fn unit_label(&self, v: f64, pattern: &str, locale: &crate::format::Locale) -> String {
        let pattern = self.format.as_deref().unwrap_or(pattern);
        crate::format::format_value(&crate::value::Value::Double(v), Some(pattern), locale)
    }

    /// `NumberAxis.selectAutoTickUnit` for `area`.
    fn select_unit(&mut self, cv: &mut Canvas, area: Rect, side: Side, locale: &crate::format::Locale) {
        let units = self.units;
        if side.vertical() {
            // estimateMaximumTickLabelHeight: insets and the font's line height.
            let m = cv.metrics(&self.font);
            let label_h = 4.0 + (m.ascent + m.descent + m.leading) as f32 as f64;
            let i1 = ceiling_unit(units, self.unit);
            let unit_h = self.length_to_java2d(units[i1].0, area, side);
            let guess = if unit_h > 0.0 { units[i1].0 * label_h / unit_h } else { (self.upper - self.lower) / 20.0 };
            let mut i2 = ceiling_unit(units, guess);
            let unit2_h = self.length_to_java2d(units[i2].0, area, side);
            if label_h > unit2_h {
                i2 = (i2 + 1).min(units.len() - 1);
            }
            self.set_unit(i2);
        } else {
            let est = |s: &mut NumberAxis, cv: &mut Canvas, pattern: &str| -> f64 {
                let w1 = cv.awt_width(&s.font, &s.unit_label(s.lower, pattern, locale));
                let w2 = cv.awt_width(&s.font, &s.unit_label(s.upper, pattern, locale));
                8.0 + w1.max(w2)
            };
            let w = est(self, cv, self.unit_pattern);
            let i1 = ceiling_unit(units, self.unit);
            let unit1_w = self.length_to_java2d(units[i1].0, area, side);
            let guess = w / unit1_w * units[i1].0;
            let mut i2 = ceiling_unit(units, guess);
            let unit2_w = self.length_to_java2d(units[i2].0, area, side);
            let w = est(self, cv, units[i2].1);
            if w > unit2_w {
                i2 = (i2 + 1).min(units.len() - 1);
            }
            self.set_unit(i2);
        }
    }

    /// `refreshTicks`: the tick values and labels for `area`.
    pub fn ticks(&mut self, cv: &mut Canvas, area: Rect, side: Side, locale: &crate::format::Locale) -> Vec<(f64, String)> {
        self.select_unit(cv, area, side, locale);
        let size = self.unit;
        let lowest = (self.lower / size).ceil() * size;
        let count = ((self.upper / size).floor() - (self.lower / size).ceil() + 1.0) as i64;
        let mut out = Vec::new();
        if count <= 500 {
            for i in 0..count.max(0) {
                let v = lowest + i as f64 * size;
                out.push((v, self.label_of(v, locale)));
            }
        }
        out
    }

    /// `ValueAxis.reserveSpace`: room the axis takes beside `plot`.
    pub fn reserve(&mut self, cv: &mut Canvas, plot: Rect, side: Side, locale: &crate::format::Locale) -> f64 {
        let mut space = 0.0;
        if self.tick_labels {
            let ticks = self.ticks(cv, plot, side, locale);
            space += max_label_extent(cv, &self.font, &ticks, side);
        }
        space + label_enclosure(cv, self.label.as_deref(), &self.label_font)
    }

    /// Draws the axis on `side` of `data`, at `cursor`; returns the ticks for grid lines.
    pub fn draw(
        &mut self,
        cv: &mut Canvas,
        cursor: f64,
        _plot: Rect,
        data: Rect,
        side: Side,
        locale: &crate::format::Locale,
    ) -> Vec<(f64, String)> {
        let ticks = self.ticks(cv, data, side, locale);
        let style = AxisStyle {
            font: self.font.clone(),
            line_color: self.line_color,
            line_visible: self.line_visible,
            tick_label_color: self.tick_label_color,
            tick_marks: self.tick_marks,
            tick_labels: self.tick_labels,
            label: self.label.clone(),
            label_font: self.label_font.clone(),
            label_color: self.label_color,
        };
        let positions: Vec<f64> = ticks.iter().map(|(v, _)| self.java2d(*v, data, side)).collect();
        self.end_cursor = draw_value_axis(cv, &style, &ticks, &positions, cursor, data, side);
        ticks
    }
}

/// `Axis.getLabelEnclosure`: the room an axis label takes (3 of insets around).
pub(crate) fn label_enclosure(cv: &mut Canvas, label: Option<&str>, font: &FontSpec) -> f64 {
    match label.filter(|l| !l.is_empty()) {
        Some(_) => cv.metrics(font).height + 6.0,
        None => 0.0,
    }
}

/// `Axis.drawLabel` with the label in the middle of the axis.
pub(crate) fn draw_axis_label(cv: &mut Canvas, label: &str, font: &FontSpec, color: Color, cursor: f64, data: Rect, side: Side) {
    let h = cv.metrics(font).height;
    match side {
        Side::Bottom => {
            let y = (cursor + 3.0 + h / 2.0) as f32 as f64;
            draw_rotated(cv, font, color, label, data.center_x() as f32 as f64, y, CENTER, 0.0, CENTER);
        }
        Side::Top => {
            let y = (cursor - 3.0 - h / 2.0) as f32 as f64;
            draw_rotated(cv, font, color, label, data.center_x() as f32 as f64, y, CENTER, 0.0, CENTER);
        }
        Side::Left => {
            let x = (cursor - 3.0 - h / 2.0) as f32 as f64;
            draw_rotated(cv, font, color, label, x, data.center_y() as f32 as f64, CENTER, -std::f64::consts::FRAC_PI_2, CENTER);
        }
        Side::Right => {
            let x = (cursor + 3.0 + h / 2.0) as f32 as f64;
            draw_rotated(cv, font, color, label, x, data.center_y() as f32 as f64, CENTER, std::f64::consts::FRAC_PI_2, CENTER);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Category axes.

/// A category axis (`CategoryAxis`): 5% margins at both ends, `margin` shared between the
/// categories.
#[derive(Debug, Clone)]
pub(crate) struct CategoryAxis {
    pub categories: Vec<String>,
    pub lower_margin: f64,
    pub upper_margin: f64,
    pub category_margin: f64,
    pub font: FontSpec,
    pub label: Option<String>,
    pub label_font: FontSpec,
    pub label_color: Color,
    pub tick_label_color: Color,
    pub line_color: Color,
    pub tick_marks: bool,
    pub tick_labels: bool,
    /// Turned labels (`labelRotation`, radians; negative turns them up).
    pub rotation: f64,
}

impl CategoryAxis {
    pub fn new(categories: Vec<String>, font: FontSpec) -> CategoryAxis {
        CategoryAxis {
            categories,
            lower_margin: 0.05,
            upper_margin: 0.05,
            category_margin: 0.2,
            label_font: font.clone(),
            font,
            label: None,
            label_color: Color::BLACK,
            tick_label_color: Color::BLACK,
            line_color: Color::BLACK,
            tick_marks: true,
            tick_labels: true,
            rotation: 0.0,
        }
    }

    fn available(area: Rect, side: Side) -> f64 {
        if side.vertical() {
            area.h
        } else {
            area.w
        }
    }

    pub fn size(&self, n: usize, area: Rect, side: Side) -> f64 {
        let a = Self::available(area, side);
        if n > 1 {
            a * (1.0 - self.lower_margin - self.upper_margin - self.category_margin) / n as f64
        } else {
            a * (1.0 - self.lower_margin - self.upper_margin)
        }
    }

    pub fn gap(&self, n: usize, area: Rect, side: Side) -> f64 {
        if n > 1 {
            Self::available(area, side) * self.category_margin / (n - 1) as f64
        } else {
            0.0
        }
    }

    pub fn start(&self, i: usize, n: usize, area: Rect, side: Side) -> f64 {
        let a = Self::available(area, side);
        let origin = if side.vertical() { area.y } else { area.x };
        origin + a * self.lower_margin + i as f64 * (self.size(n, area, side) + self.gap(n, area, side))
    }

    pub fn middle(&self, i: usize, n: usize, area: Rect, side: Side) -> f64 {
        self.start(i, n, area, side) + self.size(n, area, side) / 2.0
    }

    pub fn end(&self, i: usize, n: usize, area: Rect, side: Side) -> f64 {
        self.start(i, n, area, side) + self.size(n, area, side)
    }

    /// The label position of `side` (`CategoryLabelPositions`), for the rotation.
    fn position(&self, side: Side) -> LabelPosition {
        use HAnchor::{Center as HC, Left, Right};
        use VAnchor::{Bottom, Center as VC, Top};
        let p = |anchor: (f64, f64), h: HAnchor, v: VAnchor, angle: f64, range: bool, ratio: f32| LabelPosition {
            anchor,
            block: BlockAnchor(h, v),
            angle,
            range,
            ratio,
        };
        let r = self.rotation;
        let a = r.abs();
        if r == 0.0 {
            match side {
                Side::Top => p((0.5, 1.0), HC, Bottom, 0.0, false, 0.95),
                Side::Bottom => p((0.5, 0.0), HC, Top, 0.0, false, 0.95),
                Side::Left => p((1.0, 0.5), Right, VC, 0.0, true, 0.3),
                Side::Right => p((0.0, 0.5), Left, VC, 0.0, true, 0.3),
            }
        } else if r < 0.0 {
            // createUpRotationLabelPositions
            match side {
                Side::Top => p((0.5, 1.0), Left, Bottom, -a, true, 0.5),
                Side::Bottom => p((0.5, 0.0), Right, Top, -a, true, 0.5),
                Side::Left => p((1.0, 0.5), Right, Bottom, -a, true, 0.5),
                Side::Right => p((0.0, 0.5), Left, Top, -a, true, 0.5),
            }
        } else {
            // createDownRotationLabelPositions
            match side {
                Side::Top => p((0.5, 1.0), Right, Bottom, a, true, 0.5),
                Side::Bottom => p((0.5, 0.0), Left, Top, a, true, 0.5),
                Side::Left => p((1.0, 0.5), Right, Top, a, true, 0.5),
                Side::Right => p((0.0, 0.5), Left, Bottom, a, true, 0.5),
            }
        }
    }

    /// `refreshTicks`: each category's label broken to fit, and the room the widest takes.
    fn labels(&self, cv: &mut Canvas, plot: Rect, side: Side) -> (Vec<TextBlock>, f64) {
        let pos = self.position(side);
        let n = self.categories.len();
        let l = if pos.range { (if side.vertical() { plot.w } else { plot.h }) as f32 } else { self.size(n, plot, side) as f32 };
        let width = (l * pos.ratio) as f64;
        let mut max = 0.0f64;
        let mut blocks = Vec::new();
        for c in &self.categories {
            let block = TextBlock::wrap(cv, &self.font, c, width, 1);
            let (bw, bh) = block.size(cv, &self.font);
            let (sa, ca) = pos.angle.sin_cos();
            // The block turned, its bounding box.
            let rw = (bw * ca).abs() + (bh * sa).abs();
            let rh = (bw * sa).abs() + (bh * ca).abs();
            let extent = if side.vertical() { rw + 8.0 } else { rh + 4.0 };
            max = max.max(extent);
            blocks.push(block);
        }
        (blocks, max)
    }

    /// `CategoryAxis.reserveSpace`.
    pub fn reserve(&self, cv: &mut Canvas, plot: Rect, side: Side) -> f64 {
        let mut space = 0.0;
        if self.tick_labels {
            let (_, max) = self.labels(cv, plot, side);
            space += max + 4.0;
        } else {
            space += 4.0;
        }
        space + label_enclosure(cv, self.label.as_deref(), &self.label_font)
    }

    /// `CategoryAxis.draw` at `cursor` beside `data`.
    pub fn draw(&self, cv: &mut Canvas, cursor: f64, plot: Rect, data: Rect, side: Side) {
        let n = self.categories.len();
        match side {
            Side::Left | Side::Right => cv.line_square(cursor, data.y, cursor, data.max_y(), self.line_color, 0.5),
            Side::Top | Side::Bottom => cv.line_square(data.x, cursor, data.max_x(), cursor, self.line_color, 0.5),
        }
        let mut cursor = cursor;
        if self.tick_marks {
            for i in 0..n {
                let p = self.middle(i, n, data, side);
                match side {
                    Side::Bottom => cv.line_square(p, cursor, p, cursor + 2.0, self.line_color, 0.5),
                    Side::Top => cv.line_square(p, cursor, p, cursor - 2.0, self.line_color, 0.5),
                    Side::Left => cv.line_square(cursor, p, cursor - 2.0, p, self.line_color, 0.5),
                    Side::Right => cv.line_square(cursor, p, cursor + 2.0, p, self.line_color, 0.5),
                }
            }
            cursor = match side {
                Side::Bottom | Side::Right => cursor + 2.0,
                Side::Top | Side::Left => cursor - 2.0,
            };
        }
        if self.tick_labels {
            let pos = self.position(side);
            let (blocks, max) = self.labels(cv, plot, side);
            for (i, block) in blocks.iter().enumerate() {
                let (a, b) = (self.start(i, n, data, side), self.end(i, n, data, side));
                let area = match side {
                    Side::Bottom => Rect::new(a, cursor + 4.0, b - a, max),
                    Side::Top => Rect::new(a, cursor - 4.0 - max, b - a, max),
                    Side::Left => Rect::new(cursor - 4.0 - max, a, max, b - a),
                    Side::Right => Rect::new(cursor + 4.0, a, max, b - a),
                };
                let (px, py) = (area.x + area.w * pos.anchor.0, area.y + area.h * pos.anchor.1);
                block.draw_rotated(cv, &self.font, self.tick_label_color, px, py, pos.block, pos.angle);
            }
            cursor = match side {
                Side::Bottom | Side::Right => cursor + max + 4.0,
                Side::Top | Side::Left => cursor - max - 4.0,
            };
        }
        if let Some(l) = self.label.clone().filter(|l| !l.is_empty()) {
            draw_axis_label(cv, &l, &self.label_font, self.label_color, cursor, data, side);
        }
    }
}

struct LabelPosition {
    /// Where on the category's label area the label is anchored (fractions of its size).
    anchor: (f64, f64),
    block: BlockAnchor,
    angle: f64,
    /// Labels as wide as a share of the plot (`CategoryLabelWidthType.RANGE`), or of a category.
    range: bool,
    ratio: f32,
}

// ---------------------------------------------------------------------------------------------
// Date axes.

/// A calendar field of a date tick unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateField {
    Milli,
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

impl DateField {
    /// `DateTickUnit.getMillisecondCount` of one: months are 31 days, years 365.
    fn millis(self) -> f64 {
        match self {
            DateField::Milli => 1.0,
            DateField::Second => 1_000.0,
            DateField::Minute => 60_000.0,
            DateField::Hour => 3_600_000.0,
            DateField::Day => 86_400_000.0,
            DateField::Month => 2_678_400_000.0,
            DateField::Year => 31_536_000_000.0,
        }
    }
}

/// `DateAxis.createStandardDateTickUnits`: the field, how many of it, the label pattern.
const DATE_UNITS: &[(DateField, i64, &str)] = &[
    (DateField::Milli, 1, "HH:mm:ss.SSS"),
    (DateField::Milli, 5, "HH:mm:ss.SSS"),
    (DateField::Milli, 10, "HH:mm:ss.SSS"),
    (DateField::Milli, 25, "HH:mm:ss.SSS"),
    (DateField::Milli, 50, "HH:mm:ss.SSS"),
    (DateField::Milli, 100, "HH:mm:ss.SSS"),
    (DateField::Milli, 250, "HH:mm:ss.SSS"),
    (DateField::Milli, 500, "HH:mm:ss.SSS"),
    (DateField::Second, 1, "HH:mm:ss"),
    (DateField::Second, 5, "HH:mm:ss"),
    (DateField::Second, 10, "HH:mm:ss"),
    (DateField::Second, 30, "HH:mm:ss"),
    (DateField::Minute, 1, "HH:mm"),
    (DateField::Minute, 2, "HH:mm"),
    (DateField::Minute, 5, "HH:mm"),
    (DateField::Minute, 10, "HH:mm"),
    (DateField::Minute, 15, "HH:mm"),
    (DateField::Minute, 20, "HH:mm"),
    (DateField::Minute, 30, "HH:mm"),
    (DateField::Hour, 1, "HH:mm"),
    (DateField::Hour, 2, "HH:mm"),
    (DateField::Hour, 4, "HH:mm"),
    (DateField::Hour, 6, "HH:mm"),
    (DateField::Hour, 12, "d-MMM, HH:mm"),
    (DateField::Day, 1, "d-MMM"),
    (DateField::Day, 2, "d-MMM"),
    (DateField::Day, 7, "d-MMM"),
    (DateField::Day, 15, "d-MMM"),
    (DateField::Month, 1, "MMM-yyyy"),
    (DateField::Month, 2, "MMM-yyyy"),
    (DateField::Month, 3, "MMM-yyyy"),
    (DateField::Month, 4, "MMM-yyyy"),
    (DateField::Month, 6, "MMM-yyyy"),
    (DateField::Year, 1, "yyyy"),
    (DateField::Year, 2, "yyyy"),
    (DateField::Year, 5, "yyyy"),
    (DateField::Year, 10, "yyyy"),
    (DateField::Year, 25, "yyyy"),
    (DateField::Year, 50, "yyyy"),
    (DateField::Year, 100, "yyyy"),
];

fn date_unit_size(i: usize) -> f64 {
    let (f, n, _) = DATE_UNITS[i];
    f.millis() * n as f64
}

fn date_ceiling_unit(size: f64) -> usize {
    (0..DATE_UNITS.len()).find(|&i| date_unit_size(i) >= size).unwrap_or(DATE_UNITS.len() - 1)
}

pub(crate) fn to_date(ms: f64) -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp_millis(ms as i64).map(|d| d.naive_utc()).unwrap_or_default()
}

fn date_millis(t: chrono::NaiveDateTime) -> f64 {
    t.and_utc().timestamp_millis() as f64
}

/// Java's lenient `Calendar.set(year, month, day, hour, minute, second)` (months and days
/// may be out of range).
fn calendar(year: i32, month: i64, day: i64, hour: i64, minute: i64, second: i64, milli: i64) -> chrono::NaiveDateTime {
    let y = year as i64 + month.div_euclid(12);
    let m = month.rem_euclid(12) as u32 + 1;
    let first = chrono::NaiveDate::from_ymd_opt(y as i32, m, 1).unwrap_or_default();
    first.and_hms_opt(0, 0, 0).unwrap_or_default()
        + chrono::Duration::days(day - 1)
        + chrono::Duration::hours(hour)
        + chrono::Duration::minutes(minute)
        + chrono::Duration::seconds(second)
        + chrono::Duration::milliseconds(milli)
}

/// `DateTickUnit.addToDate`: `count` of the field later.
fn add_unit(t: chrono::NaiveDateTime, field: DateField, count: i64) -> chrono::NaiveDateTime {
    use chrono::{Datelike, Timelike};
    match field {
        DateField::Milli => t + chrono::Duration::milliseconds(count),
        DateField::Second => t + chrono::Duration::seconds(count),
        DateField::Minute => t + chrono::Duration::minutes(count),
        DateField::Hour => t + chrono::Duration::hours(count),
        DateField::Day => t + chrono::Duration::days(count),
        DateField::Month | DateField::Year => {
            let months = if field == DateField::Month { count } else { count * 12 };
            let m = t.month0() as i64 + months;
            let y = t.year() + m.div_euclid(12) as i32;
            let m = m.rem_euclid(12) as u32 + 1;
            // Calendar.add keeps the day, or the month's last when it has fewer.
            let mut d = t.day();
            let date = loop {
                if let Some(date) = chrono::NaiveDate::from_ymd_opt(y, m, d) {
                    break date;
                }
                d -= 1;
            };
            date.and_hms_milli_opt(t.hour(), t.minute(), t.second(), t.nanosecond() / 1_000_000).unwrap_or(t)
        }
    }
}

/// `DateAxis.previousStandardDate` with ticks at the start of periods (JFreeChart 1.5).
fn previous_standard_date(t: chrono::NaiveDateTime, field: DateField, count: i64) -> chrono::NaiveDateTime {
    use chrono::{Datelike, Timelike};
    let (y, mo, d) = (t.year(), t.month0() as i64, t.day() as i64);
    let (h, mi, s) = (t.hour() as i64, t.minute() as i64, t.second() as i64);
    let current = match field {
        DateField::Milli => (t.nanosecond() / 1_000_000) as i64,
        DateField::Second => s,
        DateField::Minute => mi,
        DateField::Hour => h,
        DateField::Day => d,
        DateField::Month => mo,
        DateField::Year => y as i64,
    };
    let value = count * (current / count);
    let pick = |a: chrono::NaiveDateTime, b: chrono::NaiveDateTime| if a >= t { b } else { a };
    match field {
        DateField::Milli => pick(calendar(y, mo, d, h, mi, s, value), calendar(y, mo, d, h, mi, s, value - count)),
        DateField::Second => pick(calendar(y, mo, d, h, mi, value, 0), calendar(y, mo, d, h, mi, value - count, 0)),
        DateField::Minute => pick(calendar(y, mo, d, h, value, 0, 0), calendar(y, mo, d, h, value - count, 0, 0)),
        DateField::Hour => pick(calendar(y, mo, d, value, 0, 0, 0), calendar(y, mo, d, value - count, 0, 0, 0)),
        DateField::Day => pick(calendar(y, mo, value, 0, 0, 0, 0), calendar(y, mo, value - count, 0, 0, 0, 0)),
        DateField::Month => {
            let value = count * ((current + 1) / count) - 1;
            let start = calendar(y, value, 1, 0, 0, 0, 0);
            if start >= t {
                calendar(y, value - count, 1, 0, 0, 0, 0)
            } else {
                start
            }
        }
        DateField::Year => pick(calendar(value as i32, 0, 1, 0, 0, 0, 0), calendar((value - count) as i32, 0, 1, 0, 0, 0, 0)),
    }
}

/// A date axis (`DateAxis`), in milliseconds.
#[derive(Debug, Clone)]
pub(crate) struct DateAxis {
    pub lower: f64,
    pub upper: f64,
    /// The current tick unit: an index into the standard units, `None` before the first choice.
    unit: Option<usize>,
    /// A pattern for every label (`timeAxisTickLabelMask`).
    pub format: Option<String>,
    pub font: FontSpec,
    pub label: Option<String>,
    pub label_font: FontSpec,
    pub label_color: Color,
    pub tick_label_color: Color,
    pub line_color: Color,
    pub tick_marks: bool,
    pub tick_labels: bool,
}

impl DateAxis {
    pub fn new(lower: f64, upper: f64, font: FontSpec) -> DateAxis {
        DateAxis {
            lower,
            upper,
            unit: None,
            format: None,
            label_font: font.clone(),
            font,
            label: None,
            label_color: Color::BLACK,
            tick_label_color: Color::BLACK,
            line_color: Color::BLACK,
            tick_marks: true,
            tick_labels: true,
        }
    }

    /// `DateAxis.autoAdjustRange` with `margin` on each side (at least 2 ms wide).
    pub fn auto_range(min: f64, max: f64, margin: f64) -> (f64, f64) {
        if !min.is_finite() || !max.is_finite() {
            return (0.0, 1.0);
        }
        let (mut lower, mut upper) = (min, max);
        let range = upper - lower;
        if range < 2.0 {
            let expand = (2.0 - range) / 2.0;
            upper += expand;
            lower -= expand;
        }
        (lower - margin * range, upper + margin * range)
    }

    pub fn java2d(&self, v: f64, area: Rect, side: Side) -> f64 {
        let (min, max) = if side.vertical() { (area.max_y(), area.y) } else { (area.x, area.max_x()) };
        min + (v - self.lower) / (self.upper - self.lower) * (max - min)
    }

    fn label_of(&self, t: f64, pattern: &str, locale: &crate::format::Locale) -> String {
        let value = crate::value::Value::Date(to_date(t));
        crate::format::format_value(&value, Some(self.format.as_deref().unwrap_or(pattern)), locale)
    }

    /// `estimateMaximumTickLabelWidth` with a unit (`None`: the axis' first unit, whose labels
    /// are in the short date and time format).
    fn estimate_width(&self, cv: &mut Canvas, unit: Option<usize>, locale: &crate::format::Locale) -> f64 {
        let label = |t: f64| match (unit, &self.format) {
            (_, Some(f)) => crate::format::format_value(&crate::value::Value::Date(to_date(t)), Some(f), locale),
            (Some(i), None) => self.label_of(t, DATE_UNITS[i].2, locale),
            (None, None) => {
                crate::format::format_value(&crate::value::Value::Date(to_date(t)), None, &crate::format::locale::EN_US)
            }
        };
        let (a, b) = (label(self.lower.trunc()), label(self.upper.trunc()));
        8.0 + cv.awt_width(&self.font, &a).max(cv.awt_width(&self.font, &b))
    }

    fn select_unit(&mut self, cv: &mut Canvas, area: Rect, side: Side, locale: &crate::format::Locale) {
        let zero = self.java2d(0.0, area, side);
        let length = |s: &DateAxis, size: f64| (s.java2d(size, area, side) - zero).abs();
        if !side.vertical() {
            let w = self.estimate_width(cv, self.unit, locale);
            // The first unit is a day.
            let current = self.unit.map(date_unit_size).unwrap_or(86_400_000.0);
            let i1 = date_ceiling_unit(current);
            let guess = w / length(self, date_unit_size(i1)) * date_unit_size(i1);
            let mut i2 = date_ceiling_unit(guess);
            let w2 = self.estimate_width(cv, Some(i2), locale);
            if w2 > length(self, date_unit_size(i2)) {
                i2 = (i2 + 1).min(DATE_UNITS.len() - 1);
            }
            self.unit = Some(i2);
        } else {
            let m = cv.metrics(&self.font);
            let label_h = 4.0 + (m.ascent + m.descent + m.leading) as f32 as f64;
            let i1 = date_ceiling_unit((self.upper - self.lower) / 10.0);
            let guess = label_h / length(self, date_unit_size(i1)) * date_unit_size(i1);
            let i2 = date_ceiling_unit(guess);
            let i = if label_h < length(self, date_unit_size(i2)) { i2 } else { (i2 + 1).min(DATE_UNITS.len() - 1) };
            self.unit = Some(i);
        }
    }

    /// `refreshTicks`: dates from the first standard date after the lower bound, while before
    /// the upper bound.
    pub fn ticks(&mut self, cv: &mut Canvas, area: Rect, side: Side, locale: &crate::format::Locale) -> Vec<(f64, String)> {
        self.select_unit(cv, area, side, locale);
        let i = self.unit.unwrap_or(0);
        let (field, count, pattern) = DATE_UNITS[i];
        let mut tick = add_unit(previous_standard_date(to_date(self.lower), field, count), field, count);
        let upper = to_date(self.upper);
        let mut out = Vec::new();
        while tick < upper && out.len() < 1000 {
            let ms = date_millis(tick);
            out.push((ms, self.label_of(ms, pattern, locale)));
            tick = add_unit(tick, field, count);
        }
        out
    }
}

/// A value axis: numbers or dates.
#[derive(Debug, Clone)]
pub(crate) enum ValueAxis {
    Number(NumberAxis),
    Date(DateAxis),
}

impl ValueAxis {
    pub fn java2d(&self, v: f64, area: Rect, side: Side) -> f64 {
        match self {
            ValueAxis::Number(a) => a.java2d(v, area, side),
            ValueAxis::Date(a) => a.java2d(v, area, side),
        }
    }

    pub fn bounds(&self) -> (f64, f64) {
        match self {
            ValueAxis::Number(a) => (a.lower, a.upper),
            ValueAxis::Date(a) => (a.lower, a.upper),
        }
    }

    pub fn reserve(&mut self, cv: &mut Canvas, plot: Rect, side: Side, locale: &crate::format::Locale) -> f64 {
        match self {
            ValueAxis::Number(a) => a.reserve(cv, plot, side, locale),
            ValueAxis::Date(a) => {
                let mut space = 0.0;
                if a.tick_labels {
                    let ticks = a.ticks(cv, plot, side, locale);
                    space += max_label_extent(cv, &a.font, &ticks, side);
                }
                space + label_enclosure(cv, a.label.as_deref(), &a.label_font)
            }
        }
    }

    pub fn draw(
        &mut self,
        cv: &mut Canvas,
        cursor: f64,
        plot: Rect,
        data: Rect,
        side: Side,
        locale: &crate::format::Locale,
    ) -> Vec<(f64, String)> {
        match self {
            ValueAxis::Number(a) => a.draw(cv, cursor, plot, data, side, locale),
            ValueAxis::Date(a) => {
                let ticks = a.ticks(cv, data, side, locale);
                let style = AxisStyle {
                    font: a.font.clone(),
                    line_color: a.line_color,
                    line_visible: true,
                    tick_label_color: a.tick_label_color,
                    tick_marks: a.tick_marks,
                    tick_labels: a.tick_labels,
                    label: a.label.clone(),
                    label_font: a.label_font.clone(),
                    label_color: a.label_color,
                };
                let positions: Vec<f64> = ticks.iter().map(|(v, _)| a.java2d(*v, data, side)).collect();
                let _ = draw_value_axis(cv, &style, &ticks, &positions, cursor, data, side);
                ticks
            }
        }
    }
}

/// What a value axis looks like.
pub(crate) struct AxisStyle {
    pub font: FontSpec,
    pub line_color: Color,
    pub line_visible: bool,
    pub tick_label_color: Color,
    pub tick_marks: bool,
    pub tick_labels: bool,
    pub label: Option<String>,
    pub label_font: FontSpec,
    pub label_color: Color,
}

/// `findMaximumTickLabelWidth` / `Height`, insets included: the widest label, or the font's
/// line height (`LineMetrics`, not rounded).
pub(crate) fn max_label_extent(cv: &mut Canvas, font: &FontSpec, ticks: &[(f64, String)], side: Side) -> f64 {
    if side.vertical() {
        ticks.iter().map(|(_, l)| cv.awt_width(font, l) + 8.0).fold(0.0, f64::max)
    } else {
        let m = cv.metrics(font);
        (m.ascent + m.descent + m.leading) as f32 as f64 + 4.0
    }
}

/// `ValueAxis.drawTickMarksAndLabels` and the axis label: the axis line, each tick's label
/// then its mark, at `positions`.
pub(crate) fn draw_value_axis(
    cv: &mut Canvas,
    s: &AxisStyle,
    ticks: &[(f64, String)],
    positions: &[f64],
    cursor: f64,
    data: Rect,
    side: Side,
) -> f64 {
    match side {
        _ if !s.line_visible => {}
        Side::Left | Side::Right => cv.line_square(cursor, data.y, cursor, data.max_y(), s.line_color, 0.5),
        Side::Top | Side::Bottom => cv.line_square(data.x, cursor, data.max_x(), cursor, s.line_color, 0.5),
    }
    for ((_, label), &p) in ticks.iter().zip(positions) {
        let p = p as f32 as f64;
        if s.tick_labels {
            let (x, y, anchor) = match side {
                Side::Left => (cursor - 4.0 - 2.0, p, CENTER_RIGHT),
                Side::Right => (cursor + 4.0 + 2.0, p, CENTER_LEFT),
                Side::Top => (p, cursor - 2.0 - 2.0, BOTTOM_CENTER),
                Side::Bottom => (p, cursor + 2.0 + 2.0, TOP_CENTER),
            };
            draw_aligned(cv, &s.font, s.tick_label_color, label, x as f32 as f64, y as f32 as f64, anchor);
        }
        if s.tick_marks {
            match side {
                Side::Left => cv.line_square(cursor - 2.0, p, cursor, p, s.line_color, 0.5),
                Side::Right => cv.line_square(cursor + 2.0, p, cursor, p, s.line_color, 0.5),
                Side::Top => cv.line_square(p, cursor - 2.0, p, cursor, s.line_color, 0.5),
                Side::Bottom => cv.line_square(p, cursor + 2.0, p, cursor, s.line_color, 0.5),
            }
        }
    }
    let used = if s.tick_labels { max_label_extent(cv, &s.font, ticks, side) } else { 0.0 };
    let cursor = match side {
        Side::Left | Side::Top => cursor - used,
        Side::Right | Side::Bottom => cursor + used,
    };
    let enclosure = label_enclosure(cv, s.label.as_deref(), &s.label_font);
    if let Some(l) = s.label.clone().filter(|l| !l.is_empty()) {
        draw_axis_label(cv, &l, &s.label_font, s.label_color, cursor, data, side);
    }
    match side {
        Side::Left | Side::Top => cursor - enclosure,
        Side::Right | Side::Bottom => cursor + enclosure,
    }
}

//! Paragraph settings of a text element (`<paragraph>`): indents and tab stops, and how
//! JasperReports breaks a paragraph into lines of tab segments with them.
//!
//! The rules are JasperReports' own (`ParagraphUtil`, `TextMeasurer.renderNextLine` and
//! `AbstractTextRenderer.renderParagraph`), with their quirks:
//!
//! - x coordinates are measured from the element's left edge, the left padding included, but
//!   the left indent is not: a segment is drawn at `left indent + leftX`, and tab stop
//!   positions count from the left indent. The right end of the lines, `endX`, is
//!   `width - right indent - right padding`, while the room a segment has is
//!   `endX - left indent - where it starts`.
//! - The first line of each paragraph starts at its first line indent.
//! - Each tab moves to the next tab stop after the end of the text before it: the custom stops
//!   first, then default stops every `tabStopWidth` points counted from 0 after the last
//!   custom one. A left stop starts the text there, a right one ends it there, a center one
//!   centers it on it, never before the end of the text before it.
//! - A segment that does not fit where its tab leads moves to the next line, at the first tab
//!   stop; a word that does not fit in what is left of a line goes to the next line too.
//! - Segments are positioned by their visible advance: trailing spaces do not count, unless a
//!   tab follows them.
//! - A right-aligned or centered element aligns each line as a whole by its last segment.
//!
//! The settings live outside [`crate::model::TextStyle`] in 1.x, whose fields cannot grow: the
//! parser keeps those of the elements that have any in a hidden report parameter
//! ([`crate::text::PARAGRAPHS`]), keyed by a fingerprint of the text element, and the filler
//! hands them to the [`crate::fonts::FontSet`] of the render.

use crate::model::TextElement;

/// How a tab stop aligns the text after the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabAlign {
    Left,
    Center,
    Right,
}

/// A tab stop: a position in whole points from the left indent, and an alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabStop {
    pub position: i32,
    pub align: TabAlign,
}

/// JasperReports' default distance between tab stops
/// (`net.sf.jasperreports.default.tab.stop.width`).
pub(crate) const DEFAULT_TAB_STOP_WIDTH: i32 = 40;

/// The paragraph settings that change how a text is laid out across its width.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Paragraph {
    pub first_line_indent: i32,
    pub left_indent: i32,
    pub right_indent: i32,
    pub tab_stop_width: i32,
    pub tab_stops: Vec<TabStop>,
}

impl Default for Paragraph {
    fn default() -> Self {
        Paragraph {
            first_line_indent: 0,
            left_indent: 0,
            right_indent: 0,
            tab_stop_width: DEFAULT_TAB_STOP_WIDTH,
            tab_stops: Vec::new(),
        }
    }
}

impl Paragraph {
    pub fn is_default(&self) -> bool {
        *self == Paragraph::default()
    }

    /// The settings as the hidden report parameter keeps them.
    pub fn to_json(&self) -> serde_json::Value {
        let stops: Vec<serde_json::Value> = self
            .tab_stops
            .iter()
            .map(|t| {
                let align = match t.align {
                    TabAlign::Left => "Left",
                    TabAlign::Center => "Center",
                    TabAlign::Right => "Right",
                };
                serde_json::json!([t.position, align])
            })
            .collect();
        serde_json::json!({
            "firstLineIndent": self.first_line_indent,
            "leftIndent": self.left_indent,
            "rightIndent": self.right_indent,
            "tabStopWidth": self.tab_stop_width,
            "tabStops": stops,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Paragraph> {
        let int = |name: &str, default: i32| v.get(name).and_then(|n| n.as_i64()).map_or(default, |n| n as i32);
        let tab_stops = v
            .get("tabStops")
            .and_then(|s| s.as_array())
            .map(|stops| {
                stops
                    .iter()
                    .filter_map(|s| {
                        let position = s.get(0)?.as_i64()? as i32;
                        let align = match s.get(1)?.as_str()? {
                            "Center" => TabAlign::Center,
                            "Right" => TabAlign::Right,
                            _ => TabAlign::Left,
                        };
                        Some(TabStop { position, align })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let width = int("tabStopWidth", DEFAULT_TAB_STOP_WIDTH);
        Some(Paragraph {
            first_line_indent: int("firstLineIndent", 0),
            left_indent: int("leftIndent", 0),
            right_indent: int("rightIndent", 0),
            // A width of zero or less would never reach the next stop.
            tab_stop_width: if width > 0 { width } else { DEFAULT_TAB_STOP_WIDTH },
            tab_stops,
        })
    }

    /// The tab stops up to `end_x`: the custom ones, then default ones every `tab_stop_width`
    /// points (`ParagraphUtil.getTabStops`).
    fn tab_stops_to(&self, end_x: f32) -> Vec<TabStop> {
        let width = self.tab_stop_width.max(1);
        let mut out: Vec<TabStop> = self.tab_stops.iter().copied().filter(|t| t.position as f32 <= end_x).collect();
        let mut last = self.tab_stops.last().copied().unwrap_or(TabStop { position: 0, align: TabAlign::Left });
        while ((last.position + width) as f32) <= end_x {
            last = TabStop { position: (last.position / width + 1) * width, align: TabAlign::Left };
            out.push(last);
        }
        out
    }

    /// The first tab stop after `right_x` (`ParagraphUtil.getNextTabStop`).
    pub fn next_tab_stop(&self, end_x: f32, right_x: f32) -> TabStop {
        let stops = self.tab_stops_to(end_x);
        if let Some(t) = stops.iter().find(|t| t.position as f32 > right_x) {
            return *t;
        }
        let width = self.tab_stop_width.max(1) as f32;
        TabStop { position: ((right_x / width + 1.0) * width) as i32, align: TabAlign::Left }
    }

    /// `ParagraphUtil.getFirstTabStop`.
    pub fn first_tab_stop(&self, end_x: f32) -> TabStop {
        self.tab_stops_to(end_x).first().copied().unwrap_or(TabStop { position: end_x as i32, align: TabAlign::Left })
    }

    /// `ParagraphUtil.getLastTabStop`.
    pub fn last_tab_stop(&self, end_x: f32) -> TabStop {
        self.tab_stops_to(end_x)
            .iter()
            .rev()
            .find(|t| (t.position as f32) < end_x)
            .copied()
            .unwrap_or(TabStop { position: end_x as i32, align: TabAlign::Left })
    }
}

/// Where a segment of `advance` starts at `stop` (`ParagraphUtil.getLeftX`).
fn left_x(stop: Option<TabStop>, advance: f32) -> f32 {
    match stop {
        None => 0.0,
        Some(t) => match t.align {
            TabAlign::Left => t.position as f32,
            TabAlign::Center => t.position as f32 - advance / 2.0,
            TabAlign::Right => t.position as f32 - advance,
        },
    }
}

/// Where a segment of `advance` ends at `stop` (`ParagraphUtil.getRightX`).
fn right_x(stop: Option<TabStop>, advance: f32) -> f32 {
    match stop {
        None => advance,
        Some(t) => match t.align {
            TabAlign::Left => t.position as f32 + advance,
            TabAlign::Center => t.position as f32 + advance / 2.0,
            TabAlign::Right => t.position as f32,
        },
    }
}

/// Where the room of a segment going to `stop` starts (`ParagraphUtil.getSegmentOffset`).
fn segment_offset(stop: Option<TabStop>, right_x: f32) -> f32 {
    match stop {
        Some(TabStop { position, align: TabAlign::Left }) => position as f32,
        _ => right_x,
    }
}

/// The box a text is laid out in, as JasperReports sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Geometry {
    pub left_padding: f32,
    pub width: f32,
    pub right_padding: f32,
}

/// One tab segment of a line: `start..end` in the text (the tab ending it included), drawn
/// from `left_x` (in the coordinates above), `visible` wide, in room `available` wide (what
/// justified text is spread over).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segment {
    pub start: usize,
    pub end: usize,
    pub left_x: f32,
    pub visible: f32,
    pub available: f32,
}

/// A line of tab segments: the text `start..next`, its segments, and the right end of its
/// last segment and of the line (`endX`), which right and center alignment use.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TabLine {
    pub start: usize,
    pub next: usize,
    pub segments: Vec<Segment>,
    pub right_x: f32,
    pub end_x: f32,
}

/// Breaks the paragraph `text[para]` (no line break inside) into lines of tab segments.
///
/// `advance(a, b)` is the width of `text[a..b]` as the line is measured (a tab is as wide as
/// nothing), `breaks` the ends of the units a line may break after, in order, and
/// `list_indent` the indent of an HTML list item. `first_paragraph`: the paragraph starts the
/// text.
pub(crate) fn break_paragraph(
    text: &str,
    para: std::ops::Range<usize>,
    p: &Paragraph,
    geo: Geometry,
    list_indent: f32,
    advance: &dyn Fn(usize, usize) -> f32,
    breaks: &[usize],
) -> Vec<TabLine> {
    let (from, to) = (para.start, para.end);
    let tabs: Vec<usize> = text[from..to].match_indices('\t').map(|(i, _)| from + i).collect();
    let mut lines = Vec::new();
    let mut pos = from;
    let mut current_tab = 0;
    let mut next_stop: Option<TabStop> = None;
    let mut require_next_word = false;
    // The width a text without spaces at its end has: trailing spaces hang, unless a tab
    // follows them (a tab is not a space to Java's TextLayout).
    let visible = |a: usize, b: usize| advance(a, a + text[a..b].trim_end_matches(is_space).len());
    let hanging = |a: usize, b: usize| advance(a, a + text[a..b].trim_end_matches(|c| is_space(c) || c == '\t').len());
    let mut guard = 0usize;
    while pos < to || (pos == from && from == to && lines.is_empty()) {
        guard += 1;
        if guard > 4 * (to - from + 2) {
            break;
        }
        let line_start = pos;
        let mut segments: Vec<Segment> = Vec::new();
        let mut last_right = 0.0f32;
        let mut end_x;
        loop {
            let limit = tabs.get(current_tab).map_or(to, |&t| t + 1);
            let first_indent = if pos == from { p.first_line_indent as f32 } else { 0.0 };
            let start_x = list_indent + first_indent + geo.left_padding;
            end_x = (geo.width - p.right_indent as f32 - geo.right_padding).max(start_x);
            let at = if segments.is_empty() {
                start_x
            } else {
                next_stop = Some(p.next_tab_stop(end_x, last_right));
                last_right
            };
            let available = end_x - p.left_indent as f32 - segment_offset(next_stop, at);
            let taken = next_layout(text, pos, limit, available, require_next_word, &hanging, breaks);
            if let Some(end) = taken {
                let vis = visible(pos, end);
                let lx = left_x(next_stop, vis);
                let (seg_left, seg_right) = if at > lx { (at, at + vis) } else { (lx, right_x(next_stop, vis)) };
                segments.push(Segment { start: pos, end, left_x: seg_left, visible: vis, available });
                last_right = seg_right;
                pos = end;
            }
            require_next_word = true;
            if pos == limit && limit != to {
                current_tab += 1;
            }
            if pos >= to {
                next_stop = None;
                break;
            }
            if pos == limit {
                // Stopped at a tab: the line goes on, unless it already reaches the last stop.
                if last_right >= p.last_tab_stop(end_x).position as f32 {
                    next_stop = Some(p.first_tab_stop(end_x));
                    break;
                }
                continue;
            }
            // The segment did not fit whole.
            match taken {
                None => {
                    let first = p.first_tab_stop(end_x);
                    if next_stop.map(|t| t.position) == Some(first.position) {
                        // Not even at the first stop: give up on stops, an empty line, and the
                        // next line takes at least a character.
                        next_stop = None;
                        require_next_word = false;
                    } else {
                        next_stop = Some(first);
                    }
                }
                Some(_) => {
                    next_stop = None;
                    require_next_word = false;
                }
            }
            break;
        }
        lines.push(TabLine { start: line_start, next: pos, segments, right_x: last_right, end_x });
        if from == to {
            break;
        }
    }
    lines
}

fn is_space(c: char) -> bool {
    // Java's Character.isSpaceChar: the Unicode space separators.
    matches!(
        c,
        ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// `LineBreakMeasurer.nextLayout`: the end of the longest text from `pos` up to `limit` that
/// fits in `available` (its hanging spaces aside), broken after a unit; with
/// `require_next_word`, nothing when not even the first unit fits, else at least a character.
fn next_layout(
    text: &str,
    pos: usize,
    limit: usize,
    available: f32,
    require_next_word: bool,
    width: &dyn Fn(usize, usize) -> f32,
    breaks: &[usize],
) -> Option<usize> {
    if pos >= limit {
        return None;
    }
    let fits = |end: usize| width(pos, end) <= available + 1e-3;
    if fits(limit) {
        return Some(limit);
    }
    let mut best = None;
    for &b in breaks.iter().filter(|&&b| b > pos && b < limit) {
        if fits(b) {
            best = Some(b);
        } else {
            break;
        }
    }
    if best.is_some() || require_next_word {
        return best;
    }
    // A word wider than the room: as many characters as fit, at least one.
    let mut end = pos + text[pos..].chars().next().map_or(1, char::len_utf8);
    for (i, c) in text[pos..limit].char_indices().skip(1) {
        let next = pos + i + c.len_utf8();
        if !fits(next) {
            break;
        }
        end = next;
    }
    Some(end.min(limit))
}

/// Draw position of `segment` on `line`, from the element's left edge: aligned to its tab
/// stop, the line then aligned as a whole by the element's alignment.
pub(crate) fn segment_x(line: &TabLine, segment: &Segment, p: &Paragraph, halign: crate::model::HAlign) -> f32 {
    use crate::model::HAlign;
    match halign {
        HAlign::Right => line.end_x - line.right_x + segment.left_x,
        HAlign::Center => (line.end_x - line.right_x) / 2.0 + segment.left_x,
        HAlign::Left | HAlign::Justified => p.left_indent as f32 + segment.left_x,
    }
}

/// A fingerprint of a text element, to find its paragraph settings again while filling: its
/// text and the style that does not change between the template and the filled element (the
/// font family, which a font set may replace, the rotation and the box are left out).
pub(crate) fn fingerprint(te: &TextElement) -> u64 {
    let s = &te.style;
    let key = format!(
        "{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}",
        te.content, te.markup, s.font.size, s.font.bold, s.font.italic, s.halign, s.valign, s.line_spacing
    );
    // FNV-1a: stable across runs and builds.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(stops: &[(i32, TabAlign)]) -> Paragraph {
        Paragraph {
            tab_stops: stops.iter().map(|&(position, align)| TabStop { position, align }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn tab_stops_like_jasperreports() {
        let d = Paragraph::default();
        assert_eq!(d.next_tab_stop(235.0, 20.0).position, 40);
        assert_eq!(d.next_tab_stop(235.0, 87.3).position, 120);
        // Past the last stop that fits: a stop computed from the position, not a multiple.
        assert_eq!(d.next_tab_stop(235.0, 201.0).position, 241);
        assert_eq!(d.last_tab_stop(235.0).position, 200);
        // Default stops after a custom one start again from the multiples of the width.
        let c = p(&[(33, TabAlign::Center)]);
        assert_eq!(c.next_tab_stop(235.0, 20.0), TabStop { position: 33, align: TabAlign::Center });
        assert_eq!(c.next_tab_stop(235.0, 67.3).position, 80);
        assert_eq!(c.first_tab_stop(235.0).position, 33);
    }

    /// The first band of JasperReports' paragraphs sample: default stops, a left indent of 35,
    /// a first line indent of 20 and a right indent of 20 in a box 255 wide, with widths of
    /// DejaVu Sans 10 pt.
    #[test]
    fn segments_like_the_paragraphs_sample() {
        let text = "\tpig          \tchicken   \tsheep   \thorse   \tgoat   ";
        let widths: std::collections::HashMap<&str, f32> =
            [("pig", 15.45), ("chicken", 38.35), ("sheep", 30.17), ("horse", 27.9), ("goat", 22.49)].into_iter().collect();
        let advance = |a: usize, b: usize| -> f32 {
            let s = &text[a..b];
            let words: f32 = s.split([' ', '\t']).filter(|w| !w.is_empty()).map(|w| widths[w]).sum();
            words + s.matches(' ').count() as f32 * 3.1787
        };
        let mut breaks = Vec::new();
        for (i, c) in text.char_indices() {
            if (c == ' ' || c == '\t') && text[i + 1..].starts_with(|n: char| n != ' ' && n != '\t') {
                breaks.push(i + 1);
            }
        }
        let para = Paragraph { first_line_indent: 20, left_indent: 35, right_indent: 20, ..Default::default() };
        let geo = Geometry { left_padding: 0.0, width: 255.0, right_padding: 0.0 };
        let lines = break_paragraph(text, 0..text.len(), &para, geo, 0.0, &advance, &breaks);
        let words = |l: &TabLine| -> Vec<(String, i32)> {
            l.segments
                .iter()
                .filter(|s| !text[s.start..s.end].trim().is_empty())
                .map(|s| (text[s.start..s.end].trim().to_string(), (para.left_indent as f32 + s.left_x).round() as i32))
                .collect()
        };
        assert_eq!(lines.len(), 2);
        assert_eq!(words(&lines[0]), [("pig".to_string(), 75), ("chicken".to_string(), 155)]);
        assert_eq!(words(&lines[1]), [("sheep".to_string(), 75), ("horse".to_string(), 115), ("goat".to_string(), 155)]);
    }
}

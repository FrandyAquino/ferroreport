//! OpenDocument spreadsheet (ODS) export, laid out like JasperReports' `JROdsExporter` and
//! like this crate's XLSX export: all pages on one sheet named after the report, the page cut
//! into a grid of cells at element edges (page margins kept), each text in the cells it
//! covers (merged), with its font, colors, borders, padding and alignment. Values are written
//! as text, like Jasper's default; with [`XlsxOptions::detect_cell_type`] numbers are written
//! as numbers, with a number style made from their format pattern, and dates and times (read
//! back from their text, when their pattern is all digits) as dates and times.
//!
//! The columns come from every page, so a workbook is made in two passes, as in
//! [`crate::xlsx`]. The automatic styles of `content.xml` come before the sheet, so a
//! [`OdsStream`] made with [`OdsStream::new`] keeps the sheet in memory until the end; a
//! large report is measured first instead (its styles found in a pass that keeps nothing),
//! and its rows are then compressed and written as they are made
//! ([`crate::Renderer::render_rows_to`]).

use crate::fonts::FontSet;
use crate::grid::{self, Cell, CellStyle, Sides, What};
use crate::model::{Color, HAlign, Rotation, VAlign};
use crate::odf::{self, pt, Pictures, Styles};
use crate::print::{CellInfo, PrintDocument, PrintItem, Shape};
use crate::xlsx::{escape, Border, XlsxColumns, XlsxOptions, MAX_COLUMNS, MAX_ROWS, NATURE};
use std::collections::HashMap;
use std::fmt::Write as _;

pub(crate) const MIMETYPE: &str = "application/vnd.oasis.opendocument.spreadsheet";

/// Writes a spreadsheet page by page. `columns` must have seen every page that will be
/// written (see [`XlsxColumns`]).
pub struct OdsStream<W: std::io::Write> {
    content: odf::Content<W>,
    /// The rows of the page being written.
    piece: String,
    xcuts: grid::Cuts,
    height: i32,
    page_width: f32,
    page_height: f32,
    landscape: bool,
    options: XlsxOptions,
    styles: Styles,
    pictures: Pictures,
    row_no: usize,
    pages_done: usize,
    name: String,
    /// Named ranges: name and the cell (row, column, from 0).
    anchors: Vec<(String, usize, usize)>,
    frames: usize,
}

impl<W: std::io::Write> OdsStream<W> {
    /// A sheet kept in memory until [`OdsStream::finish`], when its styles are known.
    pub fn new(
        out: W,
        columns: XlsxColumns,
        name: &str,
        page_width: f32,
        page_height: f32,
        landscape: bool,
        options: XlsxOptions,
    ) -> crate::Result<OdsStream<W>> {
        let page = (page_width, page_height, landscape);
        let mut s =
            OdsStream::start(odf::Content::kept(out), columns, name, page, options, Styles::default(), Pictures::default())?;
        // The columns, first in the sheet.
        s.content.push(&mut s.piece)?;
        Ok(s)
    }

    /// A sheet that is only measured: its rows are made and thrown away, and
    /// [`OdsStream::into_styles`] gives the styles they use.
    pub(crate) fn measuring(
        out: W,
        columns: XlsxColumns,
        name: &str,
        page: (f32, f32, bool),
        options: XlsxOptions,
    ) -> crate::Result<OdsStream<W>> {
        let mut s = OdsStream::start(
            odf::Content::measured(out),
            columns,
            name,
            page,
            options,
            Styles::default(),
            Pictures::measuring(),
        )?;
        s.content.push(&mut s.piece)?;
        Ok(s)
    }

    /// A sheet whose styles were measured (from the same pages): its rows are compressed and
    /// written as they are made.
    pub(crate) fn streamed(
        out: W,
        columns: XlsxColumns,
        name: &str,
        page: (f32, f32, bool),
        options: XlsxOptions,
        styles: Styles,
    ) -> crate::Result<OdsStream<W>> {
        // The package is started once the columns are known (they ask for their styles).
        let mut s = OdsStream::start(odf::Content::measured(out), columns, name, page, options, styles, Pictures::default())?;
        let ta = table_style(&mut s.styles);
        let body_start = s.body_start(&ta);
        let odf::Content::Held { out, .. } = s.content else { unreachable!("started as measured") };
        s.content = odf::Content::streamed(out, MIMETYPE, &mut s.styles, &body_start)?;
        // The columns, first in the sheet.
        s.content.push(&mut s.piece)?;
        Ok(s)
    }

    fn start(
        content: odf::Content<W>,
        columns: XlsxColumns,
        name: &str,
        (page_width, page_height, landscape): (f32, f32, bool),
        options: XlsxOptions,
        mut styles: Styles,
        pictures: Pictures,
    ) -> crate::Result<OdsStream<W>> {
        let mut xcuts = columns.cuts;
        xcuts.close(page_width.round() as i32);
        if xcuts.spans() > MAX_COLUMNS {
            return Err(crate::Error::Limit(format!(
                "the report needs {} columns and a sheet holds {MAX_COLUMNS}",
                xcuts.spans()
            )));
        }
        let plain = empty_style(&mut styles, &CellStyle::default());
        let mut piece = String::new();
        for i in 0..xcuts.spans() {
            let co = column_style(&mut styles, xcuts.size(i) as f32);
            let _ = writeln!(piece, "<table:table-column table:style-name=\"{co}\" table:default-cell-style-name=\"{plain}\"/>");
        }
        Ok(OdsStream {
            content,
            piece,
            xcuts,
            height: page_height.round() as i32,
            page_width,
            page_height,
            landscape,
            options,
            styles,
            pictures,
            row_no: 0,
            pages_done: 0,
            name: crate::xlsx::sheet_name(name),
            anchors: Vec::new(),
            frames: 0,
        })
    }

    /// Writes the rows of one page.
    pub fn add(&mut self, items: &[PrintItem], fonts: &FontSet) -> crate::Result<()> {
        let nodes = grid::nodes(items);
        let g = grid::layout(&nodes, &mut self.xcuts, self.height, NATURE);
        if self.row_no + g.cells.len() > MAX_ROWS {
            return Err(crate::Error::Limit(format!(
                "the report has more rows than a sheet holds ({MAX_ROWS}): page {} would go past the last one. \
                 Export it as CSV, or in parts",
                self.pages_done + 1
            )));
        }
        let links = odf::page_links(items);
        // Anchors as names of the cell where their element starts, like the XLSX export.
        let mut found = Vec::new();
        crate::xlsx::links(items, &mut found);
        for link in &found {
            let Some(anchor) = &link.anchor else { continue };
            let name = crate::xlsx::excel_name(anchor);
            if name.is_empty() || self.anchors.iter().any(|(n, ..)| *n == name) {
                continue;
            }
            let col = self.xcuts.index(link.x.round() as i32).min(self.xcuts.spans().saturating_sub(1));
            let row = self.row_no + g.rows.index(link.y.round() as i32).min(g.rows.spans().saturating_sub(1));
            self.anchors.push((name, row, col));
        }
        self.anchors.push((format!("JR_PAGE_ANCHOR_0_{}", self.pages_done + 1), self.row_no, 0));
        // The style of each element's corner cell, for the cells it covers.
        let mut corner: HashMap<(usize, usize), String> = HashMap::new();
        let mut row = Vec::new();
        for (ri, cells) in g.cells.iter().enumerate() {
            let ro = row_style(&mut self.styles, g.rows.size(ri) as f32);
            let _ = writeln!(self.piece, "<table:table-row table:style-name=\"{ro}\">");
            row.clear();
            for (ci, cell) in cells.iter().enumerate() {
                let xml = match cell {
                    Cell::Empty(style) => {
                        format!("<table:table-cell table:style-name=\"{}\"/>", empty_style(&mut self.styles, style))
                    }
                    Cell::Element { node, rows, cols, style } => {
                        let mut xml = String::new();
                        let ce = self.element(&mut xml, node, style, *rows, *cols, &links, fonts);
                        corner.insert((ri, ci), ce);
                        xml
                    }
                    Cell::Occupied(r0, c0) => match corner.get(&(*r0, *c0)) {
                        Some(ce) => format!("<table:covered-table-cell table:style-name=\"{ce}\"/>"),
                        None => "<table:covered-table-cell/>".to_string(),
                    },
                };
                row.push(xml);
            }
            write_cells(&mut self.piece, &row);
            self.piece.push_str("</table:table-row>\n");
        }
        self.row_no += g.cells.len();
        self.pages_done += 1;
        self.content.push(&mut self.piece)
    }

    /// The corner cell of an element (written to `out`), and its style.
    #[allow(clippy::too_many_arguments)]
    fn element(
        &mut self,
        out: &mut String,
        node: &grid::Node<'_>,
        style: &CellStyle,
        rows: usize,
        cols: usize,
        links: &HashMap<(i32, i32), std::sync::Arc<crate::print::LinkInfo>>,
        fonts: &FontSet,
    ) -> String {
        let span = match (rows, cols) {
            (1, 1) => String::new(),
            _ => format!(" table:number-columns-spanned=\"{cols}\" table:number-rows-spanned=\"{rows}\""),
        };
        match &node.what {
            What::Text(t) => {
                let number = t.number.filter(|n| self.options.detect_cell_type && n.is_finite());
                let pattern = match (number, &t.pattern) {
                    (Some(_), Some(p)) => parse_pattern(p),
                    _ => None,
                };
                // Calc ignores the display factor needed for per mille. Keep the formatted
                // text, as JasperReports does, instead of displaying an unscaled number.
                let number = number.filter(|_| !pattern.as_ref().is_some_and(|p| p.per_mille));
                let data = match (number, pattern.as_ref()) {
                    (Some(_), Some(p)) => Some(number_style(&mut self.styles, p)),
                    _ => None,
                };
                // A date or a time, read back from its text with its pattern, when a date style
                // shows it as the pattern does; otherwise its text, as JasperReports writes
                // every value.
                let date = match &t.pattern {
                    Some(p) if self.options.detect_cell_type && number.is_none() => date_cell(&mut self.styles, p, &t.text),
                    _ => None,
                };
                let data_name = match (&data, &date) {
                    (Some((n, _)), _) | (None, Some((_, n))) => Some(n.as_str()),
                    _ => None,
                };
                let ce = text_style(&mut self.styles, t, style, data_name);
                let value = match (number, &date) {
                    (Some(n), _) if data.as_ref().is_some_and(|(_, percent)| *percent) => {
                        format!(" office:value-type=\"percentage\" office:value=\"{}\"", odf::number(n))
                    }
                    (Some(n), _) => format!(" office:value-type=\"float\" office:value=\"{}\"", odf::number(n)),
                    (None, Some((value, _))) => value.clone(),
                    (None, None) if t.text.is_empty() => String::new(),
                    (None, None) => " office:value-type=\"string\"".to_string(),
                };
                let _ = write!(out, "<table:table-cell table:style-name=\"{ce}\"{span}{value}>");
                if !t.text.is_empty() {
                    let link = links.get(&(node.x, node.y)).filter(|l| l.target.is_some());
                    odf::paragraphs(&mut self.styles, out, t, None, !t.runs.is_empty(), true, None, link.map(|l| l.as_ref()));
                }
                out.push_str("</table:table-cell>");
                ce
            }
            What::Shape(e, _, drawn) => {
                let border = match &e.shape {
                    Shape::Rectangle(pen) | Shape::Ellipse(pen) => Border::Sides(Some([pen.unwrap_or(grid::NO_PEN); 4])),
                    Shape::Line { pen, top_down } => crate::xlsx::line_border(node.w, node.h, *pen, *top_down),
                    Shape::Picture { .. } | Shape::Frame | Shape::Image { .. } => Border::Sides(style.boxed),
                };
                let ce = shape_style(&mut self.styles, style.back, border);
                let _ = write!(out, "<table:table-cell table:style-name=\"{ce}\"{span}>");
                if let Some((href, placed)) =
                    odf::element_picture(&mut self.pictures, fonts, e, drawn, node.w as f32, node.h as f32)
                {
                    let gr = odf::picture_style(&mut self.styles, &placed, false);
                    self.frames += 1;
                    odf::frame(out, &gr, self.frames, &href, &placed, None);
                }
                out.push_str("</table:table-cell>");
                ce
            }
        }
    }

    /// What is written after the last page: the row of a report without pages, and the
    /// style of the sheet (asked for in this order, as the styles are numbered by it).
    fn close(&mut self) -> crate::Result<String> {
        if self.pages_done == 0 {
            // A report without pages: one empty row the height of a page, as the XLSX export
            // writes.
            let ro = row_style(&mut self.styles, self.page_height);
            let plain = empty_style(&mut self.styles, &CellStyle::default());
            let _ = writeln!(
                self.piece,
                "<table:table-row table:style-name=\"{ro}\"><table:table-cell table:style-name=\"{plain}\"/></table:table-row>"
            );
            self.content.push(&mut self.piece)?;
        }
        Ok(table_style(&mut self.styles))
    }

    /// `content.xml` from its styles to the first column.
    fn body_start(&self, ta: &str) -> String {
        let sheet = escape(&self.name);
        format!("<office:body>\n<office:spreadsheet>\n<table:table table:name=\"{sheet}\" table:style-name=\"{ta}\">\n")
    }

    /// The styles of a sheet that was only measured, and the output it was given.
    pub(crate) fn into_styles(mut self) -> crate::Result<(Styles, W)> {
        self.close()?;
        let out = self.content.into_output().ok_or_else(|| crate::Error::Io("the sheet was not measured".into()))?;
        Ok((self.styles, out))
    }

    /// Ends the sheet and writes the rest of the package.
    pub fn finish(mut self) -> crate::Result<W> {
        let ta = self.close()?;
        let body_start = self.body_start(&ta);
        let mut end = String::from("</table:table>\n");
        if self.pages_done > 0 && !self.anchors.is_empty() {
            end.push_str("<table:named-expressions>\n");
            let quoted = escape(&self.name.replace('\'', "''"));
            for (name, row, col) in &self.anchors {
                let cell = format!("$&apos;{quoted}&apos;.${}${}", crate::xlsx::column_name(*col), row + 1);
                let _ = writeln!(
                    end,
                    "<table:named-range table:name=\"{}\" table:base-cell-address=\"{cell}\" table:cell-range-address=\"{cell}\"/>",
                    escape(name)
                );
            }
            end.push_str("</table:named-expressions>\n");
        }
        end.push_str("</office:spreadsheet>\n</office:body>\n</office:document-content>\n");
        let styles = odf::styles_xml(self.page_width, self.page_height, self.landscape, "", &self.styles);
        self.content.finish(MIMETYPE, &self.styles, &body_start, &end, &styles, &self.name, &self.pictures)
    }
}

impl<W: std::io::Write> crate::fill::PageSink for OdsStream<W> {
    fn page(&mut self, page: crate::print::PrintPage, fonts: &FontSet) -> crate::Result<()> {
        self.add(&page.items, fonts)
    }
}

/// Exports a filled document to an OpenDocument spreadsheet.
pub fn write_ods(doc: &PrintDocument, fonts: &FontSet, options: &XlsxOptions) -> crate::Result<Vec<u8>> {
    let mut columns = XlsxColumns::new();
    for page in &doc.pages {
        columns.add(&page.items);
    }
    let mut out =
        OdsStream::new(Vec::new(), columns, &doc.name, doc.page_width, doc.page_height, doc.landscape, options.clone())?;
    for page in &doc.pages {
        out.add(&page.items, fonts)?;
    }
    out.finish()
}

/// The cells of a row, runs of the same empty cell written once with a repeat count.
fn write_cells(out: &mut String, cells: &[String]) {
    let mut i = 0;
    while i < cells.len() {
        let cell = &cells[i];
        let mut n = 1;
        if cell.ends_with("/>") {
            while i + n < cells.len() && cells[i + n] == *cell {
                n += 1;
            }
        }
        if n > 1 {
            let _ = writeln!(out, "{} table:number-columns-repeated=\"{n}\"/>", &cell[..cell.len() - 2]);
        } else {
            out.push_str(cell);
            out.push('\n');
        }
        i += n;
    }
}

/// The style of the sheet: it is printed on the page of the report.
fn table_style(styles: &mut Styles) -> String {
    styles.style(
        "ta",
        "table",
        " style:master-page-name=\"Standard\"",
        "<style:table-properties table:display=\"true\" style:writing-mode=\"lr-tb\"/>",
    )
}

fn column_style(styles: &mut Styles, width: f32) -> String {
    let props = format!("<style:table-column-properties fo:break-before=\"auto\" style:column-width=\"{}\"/>", pt(width));
    styles.style("co", "table-column", "", &props)
}

fn row_style(styles: &mut Styles, height: f32) -> String {
    let props = format!(
        "<style:table-row-properties style:row-height=\"{}\" fo:break-before=\"auto\" style:use-optimal-row-height=\"false\"/>",
        pt(height)
    );
    styles.style("ro", "table-row", "", &props)
}

fn background(back: Option<Color>) -> String {
    match back {
        Some(c) => format!(" fo:background-color=\"{}\"", odf::hex(c)),
        None => String::new(),
    }
}

/// Empty cells: the background and borders the grid gave them (frames).
fn empty_style(styles: &mut Styles, style: &CellStyle) -> String {
    shape_style(styles, style.back, Border::Sides(style.boxed))
}

/// Cells of shapes and pictures: a background and borders, or a diagonal for a slanted line.
fn shape_style(styles: &mut Styles, back: Option<Color>, border: Border) -> String {
    let lines = match border {
        Border::Sides(b) => odf::borders(b.as_ref()),
        Border::Diagonal { up, pen } => {
            let attr = if up { "style:diagonal-bl-tr" } else { "style:diagonal-tl-br" };
            format!(" fo:border=\"none\" {attr}=\"{}\"", odf::border(&pen))
        }
    };
    let props = format!(
        "<style:table-cell-properties fo:wrap-option=\"wrap\" style:shrink-to-fit=\"false\"{}{lines} fo:padding=\"0pt\"/>",
        background(back)
    );
    styles.style("ce", "table-cell", "", &props)
}

/// The style of a text's cell: its box, alignment and font (the font of a number's cell is
/// the one it is shown in), and the number style of its value.
fn text_style(styles: &mut Styles, t: &CellInfo, style: &CellStyle, data: Option<&str>) -> String {
    let rotation = match t.rotation {
        Rotation::Left => " style:rotation-angle=\"90\"",
        Rotation::Right => " style:rotation-angle=\"270\"",
        Rotation::UpsideDown => " style:rotation-angle=\"180\"",
        _ => "",
    };
    // Rotated text is aligned in the turned box (a left-rotated text starts at the bottom), as in
    // the XLSX export.
    let along = |start: &'static str, end: &'static str| match t.halign {
        HAlign::Left => start,
        HAlign::Center => "middle",
        HAlign::Right => end,
        HAlign::Justified => "middle",
    };
    let across = |top: &'static str, bottom: &'static str| match t.valign {
        VAlign::Top => top,
        VAlign::Middle => "center",
        VAlign::Bottom => bottom,
    };
    let (horizontal, vertical) = match t.rotation {
        Rotation::Left => (across("start", "end"), along("bottom", "top")),
        Rotation::Right => (across("end", "start"), along("top", "bottom")),
        Rotation::UpsideDown => (
            match t.halign {
                HAlign::Left => "end",
                HAlign::Right => "start",
                h => odf::text_align(h),
            },
            match t.valign {
                VAlign::Top => "bottom",
                VAlign::Bottom => "top",
                VAlign::Middle => "middle",
            },
        ),
        _ => (odf::text_align(t.halign), odf::vertical_align(t.valign)),
    };
    let sides: Option<Sides> = style.boxed;
    let mut props = format!(
        "<style:table-cell-properties fo:wrap-option=\"wrap\" style:shrink-to-fit=\"false\"{}{}{} style:vertical-align=\"{vertical}\" style:text-align-source=\"fix\" style:repeat-content=\"false\"{rotation}/>",
        background(style.back),
        odf::borders(sides.as_ref()),
        odf::padding(&t.boxed),
    );
    let _ = write!(props, "<style:paragraph-properties fo:text-align=\"{horizontal}\" fo:margin-left=\"0pt\"/>");
    props.push_str(&styles.text_properties(&t.font, t.color, None, crate::rich::Script::Normal));
    let attributes = match data {
        Some(n) => format!(" style:data-style-name=\"{n}\""),
        None => String::new(),
    };
    styles.style("ce", "table-cell", &attributes, &props)
}

/// A number style made from a parsed Java number pattern, and whether it shows a percentage.
fn number_style(styles: &mut Styles, p: &Pattern) -> (String, bool) {
    let mut body = String::new();
    let literal = |body: &mut String, s: &str| {
        if !s.is_empty() {
            let _ = write!(body, "<number:text>{}</number:text>", escape(s));
        }
    };
    literal(&mut body, &p.prefix);
    match p.exponent {
        Some(e) => {
            let _ = write!(
                body,
                "<number:scientific-number number:decimal-places=\"{}\" number:min-decimal-places=\"{}\" number:min-integer-digits=\"{}\" number:min-exponent-digits=\"{e}\"/>",
                p.max_decimals, p.min_decimals, p.min_integer
            );
        }
        None => {
            let _ = write!(
                body,
                "<number:number number:decimal-places=\"{}\" number:min-decimal-places=\"{}\" number:min-integer-digits=\"{}\"{}/>",
                p.max_decimals,
                p.min_decimals,
                p.min_integer,
                if p.grouping { " number:grouping=\"true\"" } else { "" }
            );
        }
    }
    literal(&mut body, &p.suffix);
    let element = if p.percent { "percentage-style" } else { "number-style" };
    (styles.data_style(element, &body), p.percent)
}

/// A part of a Java `SimpleDateFormat` pattern: literal text, or a letter and how many times
/// it is repeated.
#[derive(Debug, PartialEq)]
enum DatePart {
    Text(String),
    Field(char, usize),
}

/// The parts of a `SimpleDateFormat` pattern, or `None` when one of its letters is not one a
/// date style shows as Java does: days, months and four-digit years as digits, hours of the
/// day, minutes and seconds. Names of months and days and AM/PM are in the language of the
/// report, which the style does not know; a two-digit year needs the century Java would read
/// it in; 12-hour clocks and milliseconds have no element.
fn date_parts(pattern: &str) -> Option<Vec<DatePart>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                text.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        text.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                text.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if !c.is_ascii_alphabetic() {
            text.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        i += n;
        match (c, n) {
            ('y', 1 | 3 | 4) | ('M' | 'd' | 'H' | 'm' | 's', 1 | 2) => {}
            _ => return None,
        }
        if !text.is_empty() {
            parts.push(DatePart::Text(std::mem::take(&mut text)));
        }
        parts.push(DatePart::Field(c, n));
    }
    if !text.is_empty() {
        parts.push(DatePart::Text(text));
    }
    Some(parts)
}

/// The value attributes and data style of a cell whose text is a date or a time written with
/// `pattern`, or `None` when the text cannot be read back as one with certainty. The pattern
/// must have a whole date (day, month and year), a time of day without a date, or both. The
/// text is read with the pattern and written again: only a text that comes out the same is a
/// date, so a cell holds exactly what it shows (a date without a time in its pattern is a date
/// at midnight).
fn date_cell(styles: &mut Styles, pattern: &str, text: &str) -> Option<(String, String)> {
    use chrono::{NaiveDate, NaiveTime};
    let parts = date_parts(pattern)?;
    let has = |l: char| parts.iter().any(|p| matches!(p, DatePart::Field(c, _) if *c == l));
    let date = has('y') && has('M') && has('d');
    let time = has('H') || has('m') || has('s');
    let part_of_date = has('y') || has('M') || has('d');
    if !(date || (time && !part_of_date)) {
        return None;
    }
    // Read every field. One followed by another field, with nothing between, has the width of
    // its pattern (Java pads it to that); otherwise it takes every digit up to the next text.
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let (mut year, mut month, mut day, mut hour, mut minute, mut second) = (0, 1, 1, 0, 0, 0);
    for (i, part) in parts.iter().enumerate() {
        match part {
            DatePart::Text(s) => {
                for c in s.chars() {
                    (chars.get(at) == Some(&c)).then_some(())?;
                    at += 1;
                }
            }
            DatePart::Field(c, n) => {
                let digits = chars[at..].iter().take_while(|c| c.is_ascii_digit()).count();
                let width = match parts.get(i + 1) {
                    Some(DatePart::Field(..)) if *n >= 2 => *n,
                    Some(DatePart::Field(..)) => return None,
                    _ => digits,
                };
                if width == 0 || width > digits || width > 9 {
                    return None;
                }
                let v: u32 = chars[at..at + width].iter().collect::<String>().parse().ok()?;
                at += width;
                match c {
                    'y' => year = v as i32,
                    'M' => month = v,
                    'd' => day = v,
                    'H' => hour = v,
                    'm' => minute = v,
                    _ => second = v,
                }
            }
        }
    }
    if at != chars.len() {
        return None;
    }
    let clock = NaiveTime::from_hms_opt(hour, minute, second)?;
    // Only the time of a time is kept; its day is any.
    let day = match date {
        true if (1..=9999).contains(&year) => NaiveDate::from_ymd_opt(year, month, day)?,
        true => return None,
        false => NaiveDate::from_ymd_opt(2000, 1, 1)?,
    };
    let dt = day.and_time(clock);
    if crate::format::date::format_date(pattern, &dt, &crate::format::locale::EN_US).as_deref() != Some(text) {
        return None;
    }
    let mut body = String::new();
    for part in &parts {
        match part {
            DatePart::Text(s) => {
                let _ = write!(body, "<number:text>{}</number:text>", escape(s));
            }
            DatePart::Field(c, n) => {
                let element = match c {
                    'y' => "year",
                    'M' => "month",
                    'd' => "day",
                    'H' => "hours",
                    'm' => "minutes",
                    _ => "seconds",
                };
                // `y`, `yyy` and `yyyy` all print a four-digit year.
                let long = if *n == 2 || *c == 'y' { " number:style=\"long\"" } else { "" };
                let _ = write!(body, "<number:{element}{long}/>");
            }
        }
    }
    if date {
        let value = format!(" office:value-type=\"date\" office:date-value=\"{}\"", dt.format("%Y-%m-%dT%H:%M:%S"));
        Some((value, styles.data_style("date-style", &body)))
    } else {
        let value = format!(" office:value-type=\"time\" office:time-value=\"{}\"", clock.format("PT%HH%MM%SS"));
        Some((value, styles.data_style("time-style", &body)))
    }
}

/// The parts of a Java `DecimalFormat` pattern that a number style can say.
#[derive(Debug, PartialEq)]
struct Pattern {
    prefix: String,
    suffix: String,
    min_integer: usize,
    min_decimals: usize,
    max_decimals: usize,
    grouping: bool,
    exponent: Option<usize>,
    percent: bool,
    per_mille: bool,
}

fn parse_pattern(pattern: &str) -> Option<Pattern> {
    // The positive part (before an unquoted `;`).
    let chars: Vec<char> = pattern.chars().collect();
    let (mut prefix, mut number, mut suffix) = (String::new(), String::new(), String::new());
    let mut part = 0; // 0 prefix, 1 number, 2 suffix
    let mut percent = false;
    let mut per_mille = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ';' => break,
            '\'' => {
                let mut s = String::new();
                if chars.get(i + 1) == Some(&'\'') {
                    s.push('\'');
                    i += 2;
                } else {
                    i += 1;
                    while i < chars.len() && chars[i] != '\'' {
                        s.push(chars[i]);
                        i += 1;
                    }
                    i += 1;
                }
                if part == 1 {
                    part = 2;
                }
                if part == 0 { &mut prefix } else { &mut suffix }.push_str(&s);
                continue;
            }
            '#' | '0'..='9' | ',' | '.' if part < 2 => {
                part = 1;
                number.push(c);
            }
            'E' if part == 1 => number.push(c),
            c => {
                if part == 1 {
                    part = 2;
                }
                let c = match c {
                    '%' => {
                        percent = true;
                        '%'
                    }
                    '\u{2030}' => {
                        per_mille = true;
                        '\u{2030}'
                    }
                    '\u{a4}' => '$',
                    c => c,
                };
                if part == 0 { &mut prefix } else { &mut suffix }.push(c);
            }
        }
        i += 1;
    }
    if !number.chars().any(|c| c == '#' || c.is_ascii_digit()) {
        return None;
    }
    let (mantissa, exponent) = match number.split_once('E') {
        Some((m, e)) => (m, Some(e.chars().filter(|c| c.is_ascii_digit()).count().max(1))),
        None => (number.as_str(), None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    Some(Pattern {
        prefix,
        suffix,
        min_integer: int.chars().filter(|c| c.is_ascii_digit()).count(),
        min_decimals: frac.chars().filter(|c| c.is_ascii_digit()).count(),
        max_decimals: frac.chars().filter(|c| *c == '#' || c.is_ascii_digit()).count(),
        grouping: int.contains(','),
        exponent,
        percent,
        per_mille,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fixture's spreadsheet, and its parts by name.
    fn render(name: &str, rows: Option<serde_json::Value>, numbers: bool) -> (Vec<u8>, Vec<(String, String)>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
        let read = |f: &str| std::fs::read_to_string(dir.join(f));
        let rows = rows.unwrap_or_else(|| read("data.json").map(|t| serde_json::from_str(&t).unwrap()).unwrap_or_default());
        let params: serde_json::Value =
            read("params.json").map(|t| serde_json::from_str(&t).unwrap()).unwrap_or(serde_json::json!({}));
        let mut r = crate::Renderer::new()
            .resource_dir(&dir)
            .xlsx_options(XlsxOptions { detect_cell_type: numbers })
            .now(crate::data::parse_datetime("2026-01-15T10:30:00").unwrap());
        let report = r.parse(&read("report.jrxml").unwrap()).unwrap();
        let ods = r.render_report_json(&report, &rows, &params, crate::Format::Ods).unwrap();
        let parts =
            crate::zip::tests::read(&ods).into_iter().map(|(n, d)| (n, String::from_utf8_lossy(&d).into_owned())).collect();
        (ods, parts)
    }

    fn part<'a>(parts: &'a [(String, String)], name: &str) -> &'a str {
        parts.iter().find(|(n, _)| n == name).map(|(_, d)| d.as_str()).unwrap_or_else(|| panic!("no {name}"))
    }

    /// The package: `mimetype` first and stored as it is, then the parts a reader needs.
    pub(crate) fn check_package(bytes: &[u8], parts: &[(String, String)], mimetype: &str) {
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 0, "mimetype is stored");
        assert_eq!(&bytes[30..38], b"mimetype");
        assert_eq!(&bytes[38..38 + mimetype.len()], mimetype.as_bytes());
        assert_eq!(parts[0].0, "mimetype");
        for name in ["content.xml", "styles.xml", "meta.xml", "META-INF/manifest.xml"] {
            assert!(parts.iter().any(|(n, _)| n == name), "{name}");
        }
        assert!(part(parts, "META-INF/manifest.xml").contains(&format!("manifest:media-type=\"{mimetype}\"")));
    }

    #[test]
    fn writes_every_page_on_one_sheet() {
        let (bytes, parts) = render("02-invoice", None, false);
        check_package(&bytes, &parts, MIMETYPE);
        let content = part(&parts, "content.xml");
        assert_eq!(content.matches("<table:table ").count(), 1);
        assert!(content.contains("table:name=\"invoice\""));
        // Texts in the cells they cover, with their fonts and borders.
        assert!(content.contains("<text:p>Ferro Industrial SRL</text:p>"));
        assert!(content.contains("table:number-columns-spanned="));
        assert!(content.contains("<table:covered-table-cell"));
        assert!(content.contains("fo:font-weight=\"bold\""));
        assert!(content.contains("fo:border-bottom="));
        // Both pages, each with its anchor; numbers as text by default.
        assert!(content.contains("table:name=\"JR_PAGE_ANCHOR_0_2\""));
        assert!(content.contains("<text:p>Page 2 of<text:s/>2</text:p>") || content.contains("<text:p><text:s/>2</text:p>"));
        assert!(content.contains("office:value-type=\"string\"><text:p>3,885.00</text:p>"));
        assert!(!content.contains("office:value-type=\"float\""));
        // The page of the report, its margins in the grid.
        let styles = part(&parts, "styles.xml");
        assert!(styles.contains("fo:page-width=\"595pt\" fo:page-height=\"842pt\""));
    }

    #[test]
    fn numbers_are_numbers_with_their_pattern() {
        let (_, parts) = render("02-invoice", None, true);
        let content = part(&parts, "content.xml");
        assert!(content.contains("office:value-type=\"float\" office:value=\"3885\"><text:p>3,885.00</text:p>"));
        let style = content.split("<number:number-style ").nth(1).unwrap();
        assert!(style.contains("number:decimal-places=\"2\" number:min-decimal-places=\"2\""));
        assert!(style.contains("number:grouping=\"true\""));
        assert!(content.contains("style:data-style-name=\"N"));
    }

    #[test]
    fn per_mille_cells_stay_text_like_jasperreports() {
        // JROdsExporter 6.20.6 and 7.1.1 retain the formatted text, with type detection
        // both off and on. Calc 24.2 ignores a numeric style's display-factor="0.001".
        for (pattern, value, displayed, value_type) in [
            ("#,##0.0‰", 0.0123, "12.3‰", "string"),
            ("#,##0.0‰", -0.0123, "-12.3‰", "string"),
            ("#,##0.0‰", 0.0, "0.0‰", "string"),
            ("#,##0.0‰", 12.3456, "12,345.6‰", "string"),
            ("‰0.0", 0.0123, "‰12.3", "string"),
            ("0.0'‰'‰", 0.0123, "12.3‰‰", "string"),
            ("0.0'‰'", 12.3, "12.3‰", "float"),
            ("'‰'0.0", 12.3, "‰12.3", "float"),
            ("0.0%", 0.123, "12.3%", "percentage"),
            ("0.0", 12.3, "12.3", "float"),
        ] {
            let template = format!(
                r#"<jasperReport name="per-mille" pageWidth="300" pageHeight="100" columnWidth="260" leftMargin="20" rightMargin="20" topMargin="20" bottomMargin="20">
                <field name="value" class="java.lang.Double"/>
                <detail><band height="20"><textField pattern="{pattern}">
                <reportElement x="0" y="0" width="260" height="20"/>
                <textFieldExpression>$F{{value}}</textFieldExpression>
                </textField></band></detail></jasperReport>"#
            );
            for numbers in [true, false] {
                let mut r = crate::Renderer::new().xlsx_options(XlsxOptions { detect_cell_type: numbers });
                let report = r.parse(&template).unwrap();
                let bytes = r
                    .render_report_json(
                        &report,
                        &serde_json::json!([{ "value": value }]),
                        &serde_json::json!({}),
                        crate::Format::Ods,
                    )
                    .unwrap();
                let parts: Vec<_> = crate::zip::tests::read(&bytes)
                    .into_iter()
                    .map(|(n, d)| (n, String::from_utf8_lossy(&d).into_owned()))
                    .collect();
                let content = part(&parts, "content.xml");
                assert!(content.contains(&format!("<text:p>{displayed}</text:p>")), "{pattern}: {content}");
                assert!(!content.contains("number:display-factor="));
                if numbers && value_type != "string" {
                    assert!(content.contains(&format!("office:value-type=\"{value_type}\" office:value=\"{value}\"")));
                    assert!(content.contains("style:data-style-name=\"N"));
                } else {
                    assert!(!content.contains(" office:value="));
                    assert!(!content.contains("style:data-style-name="));
                    assert!(content.contains("office:value-type=\"string\""));
                }
            }
        }
    }

    /// One field of `class` with `pattern`, exported with and without type detection.
    fn date_content(class: &str, pattern: Option<&str>, value: &str, numbers: bool) -> String {
        let pattern = pattern.map(|p| format!(" pattern=\"{p}\"")).unwrap_or_default();
        let template = format!(
            r#"<jasperReport name="dates" pageWidth="300" pageHeight="100" columnWidth="260" leftMargin="20" rightMargin="20" topMargin="20" bottomMargin="20">
            <field name="when" class="{class}"/>
            <detail><band height="20"><textField{pattern}>
            <reportElement x="0" y="0" width="260" height="20"/>
            <textFieldExpression>$F{{when}}</textFieldExpression>
            </textField></band></detail></jasperReport>"#
        );
        let mut r = crate::Renderer::new().xlsx_options(XlsxOptions { detect_cell_type: numbers });
        let report = r.parse(&template).unwrap();
        let bytes = r
            .render_report_json(&report, &serde_json::json!([{ "when": value }]), &serde_json::json!({}), crate::Format::Ods)
            .unwrap();
        let parts: Vec<_> =
            crate::zip::tests::read(&bytes).into_iter().map(|(n, d)| (n, String::from_utf8_lossy(&d).into_owned())).collect();
        check_package(&bytes, &parts, MIMETYPE);
        part(&parts, "content.xml").to_string()
    }

    #[test]
    fn dates_are_dates_with_their_pattern() {
        let long = |e: &str| format!("<number:{e} number:style=\"long\"/>");
        let text = |t: &str| format!("<number:text>{t}</number:text>");
        for (pattern, displayed, value, style) in [
            (
                "dd/MM/yyyy",
                "05/03/2026",
                "office:value-type=\"date\" office:date-value=\"2026-03-05T00:00:00\"",
                ["date-style".into(), long("day"), text("/"), long("month"), text("/"), long("year")].concat(),
            ),
            (
                "yyyy-MM-dd HH:mm:ss",
                "2026-03-05 14:07:09",
                "office:value-type=\"date\" office:date-value=\"2026-03-05T14:07:09\"",
                [
                    "date-style".into(),
                    long("year"),
                    text("-"),
                    long("month"),
                    text("-"),
                    long("day"),
                    text(" "),
                    long("hours"),
                    text(":"),
                    long("minutes"),
                    text(":"),
                    long("seconds"),
                ]
                .concat(),
            ),
            (
                "d/M/y H:mm",
                "5/3/2026 14:07",
                "office:value-type=\"date\" office:date-value=\"2026-03-05T14:07:00\"",
                [
                    "date-style".into(),
                    "<number:day/>".into(),
                    text("/"),
                    "<number:month/>".into(),
                    text("/"),
                    long("year"),
                    text(" "),
                    "<number:hours/>".into(),
                    text(":"),
                    long("minutes"),
                ]
                .concat(),
            ),
            (
                "ddMMyyyy",
                "05032026",
                "office:value-type=\"date\" office:date-value=\"2026-03-05T00:00:00\"",
                ["date-style".into(), long("day"), long("month"), long("year")].concat(),
            ),
            (
                "H:mm 'h'",
                "14:07 h",
                "office:value-type=\"time\" office:time-value=\"PT14H07M00S\"",
                ["time-style".into(), "<number:hours/>".into(), text(":"), long("minutes"), text(" h")].concat(),
            ),
        ] {
            let content = date_content("java.util.Date", Some(pattern), "2026-03-05T14:07:09", true);
            assert!(content.contains(&format!("{value}><text:p>{displayed}</text:p>")), "{pattern}: {content}");
            let (element, body) = style.split_at(style.find('<').unwrap());
            let declared = content.split(&format!("<number:{element} ")).nth(1).unwrap_or_else(|| panic!("{pattern}: no style"));
            let name = &declared["style:name=\"".len()..declared.find("\">").unwrap()];
            assert!(declared.contains(&format!("\">{body}</number:{element}>")), "{pattern}: {declared}");
            assert!(content.contains(&format!("style:data-style-name=\"{name}\"")), "{pattern}");

            // Without type detection, the text alone, as JasperReports writes it.
            let content = date_content("java.util.Date", Some(pattern), "2026-03-05T14:07:09", false);
            assert!(content.contains(&format!("office:value-type=\"string\"><text:p>{displayed}</text:p>")));
            assert!(!content.contains("office:date-value=") && !content.contains("office:time-value="));
            assert!(!content.contains("<number:date-style") && !content.contains("<number:time-style"));
        }
    }

    #[test]
    fn dates_a_date_style_cannot_show_stay_text() {
        let stays_text = |class: &str, pattern: Option<&str>, value: &str| {
            let content = date_content(class, pattern, value, true);
            assert!(content.contains("office:value-type=\"string\""), "{pattern:?} {value}: {content}");
            assert!(!content.contains("office:date-value=") && !content.contains("office:time-value="), "{pattern:?}");
            assert!(!content.contains("<number:date-style") && !content.contains("<number:time-style"), "{pattern:?}");
        };
        // Names of months and days and AM/PM are in the report's language; a two-digit year
        // needs its century; 12-hour clocks and milliseconds have no element; a date without
        // its day or year is not a whole date. Without a pattern the locale's short date and
        // time is shown, with AM/PM in English.
        for pattern in [
            Some("dd MMM yyyy"),
            Some("EEEE d"),
            Some("h:mm a"),
            Some("hh:mm"),
            Some("HH:mm:ss.SSS"),
            Some("d.M.yy"),
            Some("dd/MM"),
            Some("MM/yyyy"),
            Some("dMyyyy"),
            None,
        ] {
            stays_text("java.util.Date", pattern, "2026-03-05T14:07:09");
        }
        // A text that is not what the pattern writes, from a value that is not a date.
        stays_text("java.lang.String", Some("dd/MM/yyyy"), "soon");
        stays_text("java.lang.String", Some("dd/MM/yyyy"), "31/02/2026");
        stays_text("java.lang.String", Some("dd/MM/yyyy"), "5/3/2026");
    }

    #[test]
    fn a_report_without_pages_is_one_empty_row() {
        let (bytes, parts) = render("02-invoice", Some(serde_json::json!([])), false);
        check_package(&bytes, &parts, MIMETYPE);
        let content = part(&parts, "content.xml");
        assert_eq!(content.matches("<table:table-row ").count(), 1);
        assert!(!content.contains("named-range"));
        assert!(content.contains("style:row-height=\"842pt\""));
    }

    /// Every style a part names, and every style it declares.
    pub(crate) fn style_names(xml: &str) -> (std::collections::BTreeSet<String>, std::collections::BTreeSet<String>) {
        let values = |attr: &str| -> std::collections::BTreeSet<String> {
            xml.split(attr).skip(1).map(|rest| rest[..rest.find('"').unwrap()].to_string()).collect()
        };
        let mut used = values(" table:style-name=\"");
        for attr in
            [" text:style-name=\"", " draw:style-name=\"", " style:data-style-name=\"", " table:default-cell-style-name=\""]
        {
            used.extend(values(attr));
        }
        (used, values(" style:name=\""))
    }

    /// Measured and written from different pages (rows that changed between the passes),
    /// a sheet still declares every style it uses: those found late are in `styles.xml`.
    #[test]
    fn styles_found_late_go_to_styles_xml() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/02-invoice");
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
        let mut r = crate::Renderer::new().resource_dir(&dir);
        let report = r.parse(&read("report.jrxml")).unwrap();
        let rows = crate::data::records_from_json(&report, &serde_json::from_str(&read("data.json")).unwrap()).unwrap();
        let params = crate::params_from_json(&report, &serde_json::from_str(&read("params.json")).unwrap()).unwrap();
        let doc = r.fill(&report, &rows, params).unwrap();
        let fonts = crate::fonts::FontSet::new();
        let mut columns = XlsxColumns::new();
        for page in &doc.pages {
            columns.add(&page.items);
        }
        let page = (doc.page_width, doc.page_height, doc.landscape);
        let options = XlsxOptions { detect_cell_type: true };
        // Measured from no pages at all.
        let measure = OdsStream::measuring(Vec::new(), columns.clone(), &doc.name, page, options.clone()).unwrap();
        let (styles, out) = measure.into_styles().unwrap();
        let mut sheet = OdsStream::streamed(out, columns, &doc.name, page, options, styles).unwrap();
        for p in &doc.pages {
            sheet.add(&p.items, &fonts).unwrap();
        }
        let bytes = sheet.finish().unwrap();
        let parts: Vec<(String, String)> =
            crate::zip::tests::read(&bytes).into_iter().map(|(n, d)| (n, String::from_utf8_lossy(&d).into_owned())).collect();
        check_package(&bytes, &parts, MIMETYPE);
        let (content, styles) = (part(&parts, "content.xml"), part(&parts, "styles.xml"));
        let (used, mut declared) = style_names(content);
        let (_, common) = style_names(styles);
        assert!(common.iter().any(|n| n.starts_with("ce")), "late cell styles are common styles");
        assert!(styles.contains("<office:font-face-decls>"), "with their fonts");
        declared.extend(common);
        let missing: Vec<_> = used.difference(&declared).collect();
        assert!(missing.is_empty(), "styles used but not declared: {missing:?}");
    }

    #[test]
    fn java_patterns() {
        let p = parse_pattern("#,##0.00").unwrap();
        assert_eq!((p.min_integer, p.min_decimals, p.max_decimals, p.grouping), (1, 2, 2, true));
        let p = parse_pattern("0.0%").unwrap();
        assert!(p.percent);
        assert_eq!((p.suffix.as_str(), p.min_decimals), ("%", 1));
        let p = parse_pattern("¤ #,##0.##;(¤ #,##0.##)").unwrap();
        assert_eq!((p.prefix.as_str(), p.min_decimals, p.max_decimals), ("$ ", 0, 2));
        let p = parse_pattern("'RD$' #,##0").unwrap();
        assert_eq!(p.prefix, "RD$ ");
        let p = parse_pattern("0.###E0").unwrap();
        assert_eq!(p.exponent, Some(1));
        let p = parse_pattern("#,##0\u{2030}").unwrap();
        assert!(p.per_mille);
        assert!(!p.percent);
        assert_eq!(p.suffix, "‰");
        assert!(!parse_pattern("0.0'‰'").unwrap().per_mille);
        assert!(parse_pattern("dd/MM/yyyy").is_none());
    }
}

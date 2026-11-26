//! Crosstabs: the data bucketed by row and column groups, measures aggregated per cell, and
//! the grid laid out the way JasperReports does it: column parts as wide as the element, rows
//! continuing on the next pages with the column headers repeated.

use crate::model::{CrosstabElement, Element, TotalPosition};
use crate::value::Value;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// One row or column of the grid: the bucket values of the groups above it. `level` is the
/// number of groups with a value: all of them for a detail row or column, fewer for the total
/// of group `level`.
#[derive(Debug, Clone)]
pub struct Leaf {
    pub values: Vec<Value>,
    pub level: usize,
}

/// A key for a list of bucket values (values compare by their Java text and type).
pub(crate) fn key(values: &[Value]) -> String {
    let mut s = String::new();
    for v in values {
        s.push_str(v.type_name());
        s.push('\u{1}');
        if !v.is_null() {
            s.push_str(&v.to_java_string());
        }
        s.push('\u{2}');
    }
    s
}

/// The leaves of one axis, in print order, from the distinct bucket paths found in the data.
/// `totals[i]` says where the total of group `i` goes; `descending[i]` how its values sort.
pub(crate) fn leaves(
    paths: &[Vec<Value>],
    totals: &[TotalPosition],
    descending: &[bool],
    order_by: &HashMap<String, Value>,
) -> Vec<Leaf> {
    let sorted = sorted_paths(paths, descending, order_by);
    let sorted: Vec<&Vec<Value>> = sorted.iter().collect();
    let mut out = Vec::new();
    walk(&sorted, 0, &[], totals, &mut out);
    out
}

/// Distinct bucket paths in print order. `order_by` holds the sort value of the buckets of
/// groups with an `orderByExpression`, by `key(path prefix)`; ties keep the natural order.
pub(crate) fn sorted_paths(paths: &[Vec<Value>], descending: &[bool], order_by: &HashMap<String, Value>) -> Vec<Vec<Value>> {
    let n = descending.len();
    // Bucket values come from the data and may mix types: a sort that cannot panic.
    let mut sorted = crate::expr::groovy::merge_sort(paths.to_vec(), &mut |a: &Vec<Value>, b: &Vec<Value>| {
        for i in 0..n {
            let by = (order_by.get(&key(&a[..=i])), order_by.get(&key(&b[..=i])));
            if let (Some(x), Some(y)) = by {
                let ord = compare(x, y);
                let ord = if descending[i] { ord.reverse() } else { ord };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                let natural = compare(&a[i], &b[i]);
                if natural != std::cmp::Ordering::Equal {
                    return natural;
                }
                continue;
            }
            let ord = compare(&a[i], &b[i]);
            let ord = if descending[i] { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    sorted.dedup_by(|a, b| key(a) == key(b));
    sorted
}

/// The distinct prefixes of length `len` of sorted paths, in order.
pub(crate) fn prefixes(sorted: &[Vec<Value>], len: usize) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    for p in sorted {
        if out.last().is_none_or(|last| key(last) != key(&p[..len])) {
            out.push(p[..len].to_vec());
        }
    }
    out
}

/// Nulls first, then natural order (Java's `compareTo`); mixed types by kind.
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a.is_null(), b.is_null()) {
        (true, true) => Equal,
        (true, false) => Less,
        (false, true) => Greater,
        // Numbers, then texts, dates and the rest, each in its own order.
        _ => {
            let rank = |v: &Value| match v {
                v if v.is_numeric() => 0,
                Value::Str(_) => 1,
                Value::Date(_) => 2,
                Value::Bool(_) => 3,
                _ => 4,
            };
            rank(a).cmp(&rank(b)).then_with(|| {
                a.compare(b).unwrap_or_else(|| match (a.as_f64(), b.as_f64()) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    _ => a.to_java_string().cmp(&b.to_java_string()),
                })
            })
        }
    }
}

fn walk(paths: &[&Vec<Value>], level: usize, prefix: &[Value], totals: &[TotalPosition], out: &mut Vec<Leaf>) {
    let n = totals.len();
    if level == n {
        out.push(Leaf { values: prefix.to_vec(), level: n });
        return;
    }
    let total = Leaf { values: prefix.to_vec(), level };
    if totals[level] == TotalPosition::Start {
        out.push(total.clone());
    }
    let mut i = 0;
    while i < paths.len() {
        let v = &paths[i][level];
        let mut j = i + 1;
        while j < paths.len() && key(std::slice::from_ref(&paths[j][level])) == key(std::slice::from_ref(v)) {
            j += 1;
        }
        let mut p = prefix.to_vec();
        p.push(v.clone());
        walk(&paths[i..j], level + 1, &p, totals, out);
        i = j;
    }
    if totals[level] == TotalPosition::End {
        out.push(total);
    }
}

/// A crosstab filled with data, ready to be laid out.
#[derive(Debug)]
pub struct Filled {
    pub crosstab: CrosstabElement,
    pub rows: Vec<Leaf>,
    pub columns: Vec<Leaf>,
    /// Height of each row and width of each column of the grid.
    pub row_heights: Vec<f32>,
    pub column_widths: Vec<f32>,
    /// Measure values by `key(row values) + "|" + key(column values)`.
    pub values: HashMap<String, Vec<Value>>,
    /// Value of each measure for a cell without data.
    pub empty: Vec<Value>,
    /// Values of the crosstab parameters.
    pub params: HashMap<String, Value>,
    /// Column ranges printed one after the other when the grid is wider than the element.
    pub parts: Vec<Range<usize>>,
}

/// Where a crosstab continues: the column part and the first row still to print.
#[derive(Debug, Clone)]
pub struct Rest {
    pub filled: Arc<Filled>,
    pub part: usize,
    pub row: usize,
}

/// One piece of a crosstab printed on a page: rows `rows` of column part `part`, `y` below the
/// top of what is printed there.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub part: usize,
    pub rows: Range<usize>,
    pub y: f32,
    pub with_headers: bool,
}

impl Filled {
    pub fn header_height(&self) -> f32 {
        self.crosstab.column_groups.iter().map(|g| g.size).sum()
    }

    pub fn row_header_width(&self) -> f32 {
        self.crosstab.row_groups.iter().map(|g| g.size).sum()
    }

    /// Width of the row headers printed with column part `part`.
    pub fn headers_width(&self, part: usize) -> f32 {
        if part == 0 || self.crosstab.repeat_row_headers {
            self.row_header_width()
        } else {
            0.0
        }
    }

    pub fn part_width(&self, part: usize) -> f32 {
        self.headers_width(part) + self.parts[part].clone().map(|c| self.column_widths[c]).sum::<f32>()
    }

    /// The pieces that fit in `room` points from `(part, row)`, and where the crosstab goes on
    /// after them (`None` when it is done). Nothing fits when the first row does not.
    pub fn plan(&self, part: usize, row: usize, room: f32) -> (Vec<Chunk>, f32, Option<(usize, usize)>) {
        let mut chunks = Vec::new();
        let (mut part, mut row, mut y) = (part, row, 0.0f32);
        loop {
            if self.parts.is_empty() || self.rows.is_empty() {
                return (chunks, y, None);
            }
            let with_headers = row == 0 || self.crosstab.repeat_column_headers;
            let hh = if with_headers { self.header_height() } else { 0.0 };
            let mut end = row;
            let mut h = hh;
            while end < self.rows.len() && y + h + self.row_heights[end] <= room + 1e-3 {
                h += self.row_heights[end];
                end += 1;
            }
            if end == row {
                return (chunks, y, Some((part, row)));
            }
            chunks.push(Chunk { part, rows: row..end, y, with_headers });
            y += h;
            if end < self.rows.len() {
                return (chunks, y, Some((part, end)));
            }
            if part + 1 >= self.parts.len() {
                return (chunks, y, None);
            }
            part += 1;
            row = 0;
            y += self.crosstab.column_break_offset;
        }
    }

    /// Design width of a column group's header: one column of each level below it (a detail
    /// column and the totals of the groups that have one). Headers are aligned on it.
    pub fn design_width(&self, group: usize) -> f32 {
        let (widths, _) = sizes(&self.crosstab);
        let groups = &self.crosstab.column_groups;
        widths[groups.len()]
            + (group + 1..groups.len())
                .filter(|&l| groups[l].total_position != TotalPosition::None)
                .map(|l| widths[l])
                .sum::<f32>()
    }

    /// Design height of a row group's header (see [`Filled::design_width`]).
    pub fn design_height(&self, group: usize) -> f32 {
        let (_, heights) = sizes(&self.crosstab);
        let groups = &self.crosstab.row_groups;
        heights[groups.len()]
            + (group + 1..groups.len())
                .filter(|&l| groups[l].total_position != TotalPosition::None)
                .map(|l| heights[l])
                .sum::<f32>()
    }

    /// The measure values of a cell.
    pub fn cell_values(&self, row: &[Value], column: &[Value]) -> &[Value] {
        let k = format!("{}|{}", key(row), key(column));
        self.values.get(&k).map(|v| v.as_slice()).unwrap_or(&self.empty)
    }
}

/// Splits columns into parts no wider than `width` (with the row headers in front of each).
pub(crate) fn split_parts(widths: &[f32], headers: f32, repeat_headers: bool, width: f32) -> Vec<Range<usize>> {
    let mut parts = Vec::new();
    let mut start = 0;
    while start < widths.len() {
        let mut used = if parts.is_empty() || repeat_headers { headers } else { 0.0 };
        let mut end = start;
        while end < widths.len() && (end == start || used + widths[end] <= width + 1e-3) {
            used += widths[end];
            end += 1;
        }
        parts.push(start..end);
        start = end;
    }
    parts
}

/// A header cell to print: the contents of a group header (or total header) placed over the
/// grid cells it spans.
#[derive(Debug)]
pub struct HeaderCell<'a> {
    pub group: usize,
    pub total: bool,
    pub contents: Option<&'a Element>,
    pub values: Vec<Value>,
    /// Position and size in the piece, in points.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Filled {
    /// Column headers of a part, level by level, at the top of the piece.
    pub fn column_headers(&self, part: usize) -> Vec<HeaderCell<'_>> {
        self.column_headers_from(part).into_iter().map(|(_, h)| h).collect()
    }

    /// For each row (`rows`) or column of the grid, how many detail rows or columns come up to
    /// it, totals not counted: the crosstab's `ROW_COUNT` and `COLUMN_COUNT`
    /// (`JRFillCrosstab.computeCounts`). A total row has the count of the row before it.
    pub(crate) fn detail_counts(&self, rows: bool) -> Vec<usize> {
        let (leaves, depth) =
            if rows { (&self.rows, self.crosstab.row_groups.len()) } else { (&self.columns, self.crosstab.column_groups.len()) };
        let mut n = 0;
        leaves
            .iter()
            .map(|l| {
                n += usize::from(l.level >= depth);
                n
            })
            .collect()
    }

    /// [`Filled::column_headers`], each with the index of the first column it spans.
    pub(crate) fn column_headers_from(&self, part: usize) -> Vec<(usize, HeaderCell<'_>)> {
        let groups = &self.crosstab.column_groups;
        let range = self.parts[part].clone();
        let x0 = self.headers_width(part);
        let mut out = Vec::new();
        let mut y = 0.0;
        for (j, g) in groups.iter().enumerate() {
            let mut x = x0;
            let mut c = range.start;
            while c < range.end {
                let leaf = &self.columns[c];
                let w = self.column_widths[c];
                if leaf.level == j {
                    let h: f32 = groups[j..].iter().map(|g| g.size).sum();
                    out.push((
                        c,
                        HeaderCell {
                            group: j,
                            total: true,
                            contents: g.total_header.as_ref(),
                            values: leaf.values.clone(),
                            x,
                            y,
                            w,
                            h,
                        },
                    ));
                    x += w;
                    c += 1;
                } else if leaf.level > j {
                    let prefix = key(&leaf.values[..=j]);
                    let (start_x, first, mut span) = (x, c, 0.0);
                    while c < range.end && self.columns[c].level > j && key(&self.columns[c].values[..=j]) == prefix {
                        span += self.column_widths[c];
                        c += 1;
                    }
                    out.push((
                        first,
                        HeaderCell {
                            group: j,
                            total: false,
                            contents: g.header.as_ref(),
                            values: leaf.values[..=j].to_vec(),
                            x: start_x,
                            y,
                            w: span,
                            h: g.size,
                        },
                    ));
                    x += span;
                } else {
                    x += w;
                    c += 1;
                }
            }
            y += g.size;
        }
        out
    }

    /// Row headers that start at row `r` of a piece holding rows `rows`, outer groups first.
    /// `y` is the top of row `r` in the piece.
    pub fn row_headers(&self, r: usize, rows: &Range<usize>, y: f32) -> Vec<HeaderCell<'_>> {
        let groups = &self.crosstab.row_groups;
        let leaf = &self.rows[r];
        let mut out = Vec::new();
        let mut x = 0.0;
        for (i, g) in groups.iter().enumerate() {
            if leaf.level == i {
                let w: f32 = groups[i..].iter().map(|g| g.size).sum();
                out.push(HeaderCell {
                    group: i,
                    total: true,
                    contents: g.total_header.as_ref(),
                    values: leaf.values.clone(),
                    x,
                    y,
                    w,
                    h: self.row_heights[r],
                });
                break;
            }
            // A group header starts at the first row of its bucket in this piece.
            let prefix = key(&leaf.values[..=i]);
            let continues = r > rows.start && self.rows[r - 1].level > i && key(&self.rows[r - 1].values[..=i]) == prefix;
            if !continues {
                let mut end = r;
                let mut h = 0.0;
                while end < rows.end && self.rows[end].level > i && key(&self.rows[end].values[..=i]) == prefix {
                    h += self.row_heights[end];
                    end += 1;
                }
                out.push(HeaderCell {
                    group: i,
                    total: false,
                    contents: g.header.as_ref(),
                    values: leaf.values[..=i].to_vec(),
                    x,
                    y,
                    w: g.size,
                    h,
                });
            }
            x += g.size;
        }
        out
    }

    /// The contents of the data cell for a row and a column (`JRDesignCrosstab` cell matrix:
    /// the cell defined for both totals, else for one of them, else the detail cell).
    pub fn cell_contents(&self, row: &Leaf, column: &Leaf) -> Option<&Element> {
        let ct = &self.crosstab;
        let row_total = (row.level < ct.row_groups.len()).then(|| ct.row_groups[row.level].name.as_str());
        let column_total = (column.level < ct.column_groups.len()).then(|| ct.column_groups[column.level].name.as_str());
        let find = |r: Option<&str>, c: Option<&str>| {
            ct.cells.iter().find(|cell| cell.row_total.as_deref() == r && cell.column_total.as_deref() == c)
        };
        find(row_total, column_total)
            .or_else(|| find(row_total, None))
            .or_else(|| find(None, column_total))
            .or_else(|| find(None, None))
            .and_then(|c| c.contents.as_ref())
    }
}

/// Width of the columns and height of the rows at each total level (the last level is the
/// detail), from the cells defined for them.
pub(crate) fn sizes(ct: &CrosstabElement) -> (Vec<f32>, Vec<f32>) {
    let detail = ct.cells.iter().find(|c| c.row_total.is_none() && c.column_total.is_none());
    let detail_w = detail.and_then(|c| c.width).unwrap_or(0.0);
    let detail_h = detail.and_then(|c| c.height).unwrap_or(0.0);
    let widths = (0..=ct.column_groups.len())
        .map(|l| {
            let name = ct.column_groups.get(l).map(|g| g.name.as_str());
            ct.cells.iter().filter(|c| c.column_total.as_deref() == name).find_map(|c| c.width).unwrap_or(detail_w)
        })
        .collect();
    let heights = (0..=ct.row_groups.len())
        .map(|l| {
            let name = ct.row_groups.get(l).map(|g| g.name.as_str());
            ct.cells.iter().filter(|c| c.row_total.as_deref() == name).find_map(|c| c.height).unwrap_or(detail_h)
        })
        .collect();
    (widths, heights)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::str(v)
    }

    #[test]
    fn leaves_sort_and_place_totals() {
        let paths = vec![vec![s("Sur"), s("Azua")], vec![s("Este"), s("Higuey")], vec![s("Este"), s("Bani")]];
        let none = HashMap::new();
        let l = leaves(&paths, &[TotalPosition::End, TotalPosition::End], &[false, false], &none);
        let names: Vec<String> = l
            .iter()
            .map(|x| format!("{}:{}", x.level, x.values.iter().map(|v| v.to_java_string()).collect::<Vec<_>>().join("/")))
            .collect();
        assert_eq!(names, ["2:Este/Bani", "2:Este/Higuey", "1:Este", "2:Sur/Azua", "1:Sur", "0:"]);
        let l = leaves(&paths, &[TotalPosition::Start, TotalPosition::None], &[true, false], &none);
        let names: Vec<String> = l.iter().map(|x| format!("{}", x.level)).collect();
        assert_eq!(names, ["0", "2", "2", "2"]);
        assert_eq!(l[1].values[0].to_java_string(), "Sur");
    }

    #[test]
    fn parts_fit_the_width() {
        assert_eq!(split_parts(&[60.0; 5], 100.0, true, 280.0), vec![0..3, 3..5]);
        assert_eq!(split_parts(&[300.0, 60.0], 100.0, true, 280.0), vec![0..1, 1..2]);
    }
}

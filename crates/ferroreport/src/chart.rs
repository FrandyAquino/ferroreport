//! Charts (`<pieChart>`, `<barChart>`, `<lineChart>`, ...): datasets collected while filling
//! and drawing that follows JFreeChart 1.5's rules as JasperReports 7 uses them (plots, axes,
//! tick units, legends, renderers), checked pixel by pixel against JasperReports' own chart
//! samples (`category`, `pie`, `xy`, `meter`, `thermometer` and the shared pieces in
//! `jfree`).
//!
//! Chart text is written as glyph outlines, as JasperReports writes it. Multiple axis charts
//! of XY charts still follow JFreeChart's look without being
//! pixel-identical.

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use crate::expr::Expr;
use crate::fonts::{FaceId, FontSet};
use crate::format::{format_value, Locale};
use crate::model::{BoxStyle, Color, EvaluationTime, FontSpec, Pen, ResetType};
use crate::print::{PathOp, PrintItem, PrintLine, PrintText};
use crate::value::Value;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

mod bitmap;
mod category;
mod jfree;
mod meter;
mod pie;
mod thermometer;
mod xy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Pie,
    Pie3D,
    Bar,
    Bar3D,
    StackedBar,
    StackedBar3D,
    Line,
    Area,
    StackedArea,
    XyLine,
    XyArea,
    Scatter,
    TimeSeries,
    XyBar,
    Bubble,
    Candlestick,
    HighLow,
    Gantt,
    Meter,
    Thermometer,
    MultiAxis,
}

impl ChartKind {
    pub fn from_tag(tag: &str) -> Option<ChartKind> {
        Some(match tag {
            "pieChart" => ChartKind::Pie,
            "pie3DChart" => ChartKind::Pie3D,
            "barChart" => ChartKind::Bar,
            "bar3DChart" => ChartKind::Bar3D,
            "stackedBarChart" => ChartKind::StackedBar,
            "stackedBar3DChart" => ChartKind::StackedBar3D,
            "lineChart" => ChartKind::Line,
            "areaChart" => ChartKind::Area,
            "stackedAreaChart" => ChartKind::StackedArea,
            "xyLineChart" => ChartKind::XyLine,
            "xyAreaChart" => ChartKind::XyArea,
            "scatterChart" => ChartKind::Scatter,
            "timeSeriesChart" => ChartKind::TimeSeries,
            "xyBarChart" => ChartKind::XyBar,
            "bubbleChart" => ChartKind::Bubble,
            "candlestickChart" => ChartKind::Candlestick,
            "highLowChart" => ChartKind::HighLow,
            "ganttChart" => ChartKind::Gantt,
            "meterChart" => ChartKind::Meter,
            "thermometerChart" => ChartKind::Thermometer,
            "multiAxisChart" => ChartKind::MultiAxis,
            _ => return None,
        })
    }

    fn is_pie(self) -> bool {
        matches!(self, ChartKind::Pie | ChartKind::Pie3D)
    }
    fn is_xy(self) -> bool {
        matches!(
            self,
            ChartKind::XyLine
                | ChartKind::XyArea
                | ChartKind::Scatter
                | ChartKind::TimeSeries
                | ChartKind::XyBar
                | ChartKind::Bubble
                | ChartKind::Candlestick
                | ChartKind::HighLow
        )
    }
    fn is_stacked(self) -> bool {
        matches!(self, ChartKind::StackedBar | ChartKind::StackedBar3D | ChartKind::StackedArea)
    }
    fn is_bar(self) -> bool {
        matches!(self, ChartKind::Bar | ChartKind::Bar3D | ChartKind::StackedBar | ChartKind::StackedBar3D)
    }
    fn is_3d(self) -> bool {
        matches!(self, ChartKind::Bar3D | ChartKind::StackedBar3D | ChartKind::Pie3D)
    }
}

/// When a dataset takes a new value.
#[derive(Debug, Clone, PartialEq)]
pub enum Increment {
    /// Every record (JasperReports' `incrementType="None"`).
    Record,
    /// When the group ends.
    Group(String),
    /// When the page ends.
    Page,
    /// Once, at the end of the report.
    Report,
}

#[derive(Debug, Clone, Default)]
pub struct SeriesDef {
    pub series: Option<Expr>,
    pub key: Option<Expr>,
    pub category: Option<Expr>,
    pub value: Option<Expr>,
    pub x: Option<Expr>,
    pub y: Option<Expr>,
    pub z: Option<Expr>,
    /// Time series: the date whose period the value belongs to.
    pub period: Option<Expr>,
    /// High-low data: the date and its prices.
    pub date: Option<Expr>,
    pub high: Option<Expr>,
    pub low: Option<Expr>,
    pub open: Option<Expr>,
    pub close: Option<Expr>,
    pub volume: Option<Expr>,
    /// Gantt data.
    pub task: Option<Expr>,
    pub subtask: Option<Expr>,
    /// Gantt tasks and time periods: when they start and end.
    pub start: Option<Expr>,
    pub end: Option<Expr>,
    pub percent: Option<Expr>,
}

/// What a chart's dataset holds (the element it is read from: `<pieDataset>`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataKind {
    #[default]
    Category,
    Pie,
    Xy,
    TimeSeries,
    TimePeriod,
    Xyz,
    HighLow,
    Value,
    Gantt,
}

impl DataKind {
    pub fn from_tag(tag: &str) -> DataKind {
        match tag.trim_end_matches("Dataset") {
            "pie" => DataKind::Pie,
            "xy" => DataKind::Xy,
            "timeSeries" => DataKind::TimeSeries,
            "timePeriod" => DataKind::TimePeriod,
            "xyz" => DataKind::Xyz,
            "highLow" => DataKind::HighLow,
            "value" => DataKind::Value,
            "gantt" => DataKind::Gantt,
            _ => DataKind::Category,
        }
    }
}

/// The period a time series groups its values by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimePeriod {
    Year,
    Quarter,
    Month,
    Week,
    #[default]
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
}

impl TimePeriod {
    pub fn parse(v: &str) -> TimePeriod {
        match v {
            "Year" => TimePeriod::Year,
            "Quarter" => TimePeriod::Quarter,
            "Month" => TimePeriod::Month,
            "Week" => TimePeriod::Week,
            "Hour" => TimePeriod::Hour,
            "Minute" => TimePeriod::Minute,
            "Second" => TimePeriod::Second,
            "Milisecond" | "Millisecond" => TimePeriod::Millisecond,
            _ => TimePeriod::Day,
        }
    }

    /// The period around `t`: its first and last millisecond, in milliseconds.
    pub fn bounds(self, t: chrono::NaiveDateTime) -> (f64, f64) {
        use chrono::{Datelike, Duration, NaiveDate, Timelike};
        let day = t.date();
        let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).unwrap_or(t);
        let month_start = |y: i32, m: u32| NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(day);
        let next_month = |y: i32, m: u32| if m == 12 { month_start(y + 1, 1) } else { month_start(y, m + 1) };
        let (a, b) = match self {
            TimePeriod::Year => (midnight(month_start(day.year(), 1)), midnight(month_start(day.year() + 1, 1))),
            TimePeriod::Quarter => {
                let first = (day.month() - 1) / 3 * 3 + 1;
                let start = month_start(day.year(), first);
                let end = if first == 10 { month_start(day.year() + 1, 1) } else { month_start(day.year(), first + 3) };
                (midnight(start), midnight(end))
            }
            TimePeriod::Month => (midnight(month_start(day.year(), day.month())), midnight(next_month(day.year(), day.month()))),
            TimePeriod::Week => {
                // Weeks start on Sunday (en_US, JFreeChart's default).
                let start = day - Duration::days(day.weekday().num_days_from_sunday() as i64);
                (midnight(start), midnight(start + Duration::days(7)))
            }
            TimePeriod::Day => (midnight(day), midnight(day + Duration::days(1))),
            TimePeriod::Hour => {
                let s = t.with_minute(0).and_then(|v| v.with_second(0)).and_then(|v| v.with_nanosecond(0)).unwrap_or(t);
                (s, s + Duration::hours(1))
            }
            TimePeriod::Minute => {
                let s = t.with_second(0).and_then(|v| v.with_nanosecond(0)).unwrap_or(t);
                (s, s + Duration::minutes(1))
            }
            TimePeriod::Second => {
                let s = t.with_nanosecond(0).unwrap_or(t);
                (s, s + Duration::seconds(1))
            }
            TimePeriod::Millisecond => {
                let s = t.with_nanosecond(t.nanosecond() / 1_000_000 * 1_000_000).unwrap_or(t);
                (s, s + Duration::milliseconds(1))
            }
        };
        (millis(a), millis(b) - 1.0)
    }
}

/// Milliseconds since 1970 of a date (in the report's time zone, taken as UTC).
pub fn millis(t: chrono::NaiveDateTime) -> f64 {
    t.and_utc().timestamp_millis() as f64
}

#[derive(Debug, Clone)]
pub struct DatasetDef {
    pub reset: ResetType,
    pub increment: Increment,
    pub increment_when: Option<Expr>,
    pub series: Vec<SeriesDef>,
    pub kind: DataKind,
    pub time_period: TimePeriod,
    /// The rows come from a subdataset run, not from the report.
    pub run: Option<Box<crate::model::DatasetRun>>,
}

/// A range of a meter or thermometer (`<meterInterval>`, `<lowRange>`, ...).
#[derive(Debug, Clone)]
pub struct Interval {
    pub label: String,
    pub color: Option<Color>,
    pub alpha: f32,
    pub low: Option<Expr>,
    pub high: Option<Expr>,
}

/// Where a thermometer writes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueLocation {
    None,
    Left,
    Right,
    Bulb,
}

/// The plot of a meter or thermometer chart.
#[derive(Debug, Clone)]
pub struct DialPlot {
    pub low: Option<Expr>,
    pub high: Option<Expr>,
    pub intervals: Vec<Interval>,
    pub units: String,
    pub tick_interval: f64,
    pub needle: Color,
    pub tick_color: Color,
    pub background: Option<Color>,
    /// Degrees the dial spans.
    pub angle: f64,
    pub value_color: Color,
    pub value_mask: Option<String>,
    pub value_font: FontSpec,
    pub tick_font: FontSpec,
    pub mercury: Color,
    pub value_location: ValueLocation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct ChartElement {
    /// Unique within its report: identifies the chart's dataset while filling.
    pub id: usize,
    pub kind: ChartKind,
    pub horizontal: bool,
    pub title: Option<Expr>,
    pub title_font: FontSpec,
    pub title_color: Color,
    pub subtitle: Option<Expr>,
    pub subtitle_font: FontSpec,
    pub subtitle_color: Color,
    pub show_legend: bool,
    pub legend_font: FontSpec,
    pub legend_color: Color,
    pub legend_back: Option<Color>,
    pub legend_position: Edge,
    pub font: FontSpec,
    pub evaluation_time: EvaluationTime,
    pub evaluation_group: Option<String>,
    pub dataset: DatasetDef,
    pub category_axis_label: Option<Expr>,
    pub value_axis_label: Option<Expr>,
    pub value_min: Option<Expr>,
    pub value_max: Option<Expr>,
    pub series_colors: Vec<Color>,
    pub show_labels: bool,
    pub show_lines: bool,
    pub show_shapes: bool,
    pub backcolor: Option<Color>,
    pub plot_backcolor: Option<Color>,
    pub value_format: Option<String>,
    /// Tick label pattern of a date axis (`timeAxisFormat`).
    pub time_format: Option<String>,
    /// Tick label pattern of an XY chart's domain axis (`xAxisFormat`).
    pub x_format: Option<String>,
    pub boxed: BoxStyle,
    /// Meter and thermometer charts.
    pub dial: Option<Box<DialPlot>>,
    /// Candlestick charts: draw the volume behind the candles.
    pub show_volume: bool,
    /// Multiple axis charts: the charts drawn together, each on its own value axis (`true`:
    /// the axis is on the right).
    pub axes: Vec<(bool, ChartElement)>,
    /// The color of the value axis' labels and line (multiple axis charts).
    pub axis_color: Option<Color>,
    /// Pie charts: how sections and legend items are labeled (`{0} {2}`).
    pub label_format: Option<String>,
    pub legend_label_format: Option<String>,
}

/// Settings of a chart that its public struct has no field for (adding one would break code
/// that builds a `ChartElement` with a struct literal): the parser records them here, keyed by
/// what the chart is, and drawing looks them up. A copy of a chart has the same key; a chart
/// changed after parsing gets its settings with [`copy_settings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    /// Drawn as SVG (`net.sf.jasperreports.chart.render.type=svg`) rather than straight on the
    /// page: JFreeChart then measures text glyph by glyph in whole points, and otherwise rounds
    /// a whole text's width.
    pub svg: bool,
    /// How opaque what the plot draws is (`foregroundAlpha`, 1 by default).
    pub foreground_alpha: f32,
    /// Meters: how the dial is closed (`shape`).
    pub meter_shape: MeterShape,
    /// High-low charts: draw the open and close ticks (`showOpenTicks`, `showCloseTicks`).
    pub show_open_ticks: bool,
    pub show_close_ticks: bool,
    /// Thermometers without a `mercuryColor`: the mercury takes the color of the range the
    /// value is in, and `DialPlot::mercury` (JFreeChart's light gray) outside them.
    pub mercury_by_range: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            svg: false,
            foreground_alpha: 1.0,
            meter_shape: MeterShape::Pie,
            show_open_ticks: false,
            show_close_ticks: false,
            mercury_by_range: false,
        }
    }
}

/// How a meter's dial is closed (JFreeChart's `DialShape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeterShape {
    /// Pie wedges from the center: JasperReports' default. Its default chart theme draws a
    /// `dial` meter this way too (only other chart themes use JFreeChart's `DialPlot`).
    Pie,
    /// The background of a meter of more than 180 degrees closed by a straight line.
    Chord,
    /// The background a whole circle.
    Circle,
}

/// Charts with other settings than the defaults, by [`settings_key`].
fn settings_table() -> &'static std::sync::Mutex<HashMap<u64, Settings>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<HashMap<u64, Settings>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// What identifies a chart: a hash of everything in it. Two charts alike in every public field
/// share their settings (the last one parsed gives them).
fn settings_key(ch: &ChartElement) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{ch:?}").hash(&mut h);
    h.finish()
}

/// Records `s` as the settings of `ch`.
pub(crate) fn set_settings(ch: &ChartElement, s: Settings) {
    let key = settings_key(ch);
    let mut table = settings_table().lock().unwrap_or_else(|e| e.into_inner());
    if s == Settings::default() {
        table.remove(&key);
    } else {
        table.insert(key, s);
    }
}

/// The settings of `ch` (the defaults for a chart the parser did not make).
pub(crate) fn settings(ch: &ChartElement) -> Settings {
    let key = settings_key(ch);
    settings_table().lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied().unwrap_or_default()
}

/// Gives `to`, a changed copy of `from`, the settings of `from`.
pub(crate) fn copy_settings(from: &ChartElement, to: &ChartElement) {
    set_settings(to, settings(from));
}

/// Values collected for one chart.
#[derive(Debug, Clone, Default)]
pub struct DataState {
    pub pie: Vec<(String, f64)>,
    pub series: Vec<String>,
    pub categories: Vec<String>,
    pub values: HashMap<(usize, usize), f64>,
    pub xy: Vec<(String, Vec<(f64, f64)>)>,
    /// Time series and time periods: (start, end, value), times in milliseconds.
    pub periods: Vec<(String, Vec<(f64, f64, f64)>)>,
    pub xyz: Vec<(String, Vec<(f64, f64, f64)>)>,
    /// High-low data: date, high, low, open, close, volume.
    pub ohlc: Vec<(String, Vec<[f64; 6]>)>,
    /// Gantt data, per series.
    pub gantt: Vec<(String, Vec<GanttTask>)>,
    /// Meters and thermometers.
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct GanttTask {
    pub task: String,
    pub start: f64,
    pub end: f64,
    pub percent: Option<f64>,
    /// (start, end, percent) of each subtask.
    pub subtasks: Vec<(f64, f64, Option<f64>)>,
}

fn entry<T>(list: &mut Vec<(String, Vec<T>)>, key: String) -> &mut Vec<T> {
    let i = list.iter().position(|(k, _)| *k == key).unwrap_or_else(|| {
        list.push((key, Vec::new()));
        list.len() - 1
    });
    &mut list[i].1
}

impl DataState {
    pub fn add_pie(&mut self, key: String, value: f64) {
        // JasperReports refuses duplicate keys; adding them up is more useful.
        match self.pie.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 += value,
            None => self.pie.push((key, value)),
        }
    }

    pub fn add_category(&mut self, series: String, category: String, value: f64) {
        let s = self.series.iter().position(|x| *x == series).unwrap_or_else(|| {
            self.series.push(series);
            self.series.len() - 1
        });
        let c = self.categories.iter().position(|x| *x == category).unwrap_or_else(|| {
            self.categories.push(category);
            self.categories.len() - 1
        });
        // Like JFreeChart's setValue: the last value for a (series, category) pair wins.
        self.values.insert((s, c), value);
    }

    pub fn add_xy(&mut self, series: String, x: f64, y: f64) {
        entry(&mut self.xy, series).push((x, y));
    }

    /// A value for a period: a time series keeps the last one given for each period
    /// (`replace`), time periods keep them all.
    pub fn add_period(&mut self, series: String, start: f64, end: f64, value: f64, replace: bool) {
        let list = entry(&mut self.periods, series);
        match list.iter_mut().find(|p| replace && p.0 == start) {
            Some(p) => p.2 = value,
            None => list.push((start, end, value)),
        }
    }

    pub fn add_xyz(&mut self, series: String, x: f64, y: f64, z: f64) {
        entry(&mut self.xyz, series).push((x, y, z));
    }

    pub fn add_ohlc(&mut self, series: String, item: [f64; 6]) {
        entry(&mut self.ohlc, series).push(item);
    }

    /// A task, or a subtask of `task` when `subtask` is given.
    pub fn add_gantt(&mut self, series: String, task: String, subtask: bool, start: f64, end: f64, percent: Option<f64>) {
        let tasks = entry(&mut self.gantt, series);
        let t = match tasks.iter().position(|t| t.task == task) {
            Some(i) => &mut tasks[i],
            None => {
                tasks.push(GanttTask { task, start, end, percent, subtasks: Vec::new() });
                tasks.last_mut().unwrap_or_else(|| unreachable!())
            }
        };
        if subtask {
            t.subtasks.push((start, end, percent));
            t.start = t.start.min(start);
            t.end = t.end.max(end);
        } else {
            (t.start, t.end, t.percent) = (start, end, percent);
        }
    }
}

/// Texts evaluated when the chart is drawn.
#[derive(Debug, Clone, Default)]
pub struct ChartTexts {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub category_label: Option<String>,
    pub value_label: Option<String>,
    pub value_min: Option<f64>,
    pub value_max: Option<f64>,
    /// Meters and thermometers: the data range and the range of each interval.
    pub dial: Option<(f64, f64)>,
    pub intervals: Vec<Option<(f64, f64)>>,
    /// Multiple axis charts: the texts of each chart.
    pub axes: Vec<ChartTexts>,
}

/// JFreeChart's default series colors (`ChartColor.createDefaultPaintArray`).
const PALETTE: [u32; 34] = [
    0xFF5555, 0x5555FF, 0x55FF55, 0xFFFF55, 0xFF55FF, 0x55FFFF, 0xFFAFAF, 0x808080, 0xC00000, 0x0000C0, 0x00C000, 0xC0C000,
    0xC000C0, 0x00C0C0, 0x404040, 0xFF4040, 0x4040FF, 0x40FF40, 0xFFFF40, 0xFF40FF, 0x40FFFF, 0xC0C0C0, 0x800000, 0x000080,
    0x008000, 0x808000, 0x800080, 0x008080, 0xFF8080, 0x8080FF, 0x80FF80, 0xFFFF80, 0xFF80FF, 0x80FFFF,
];

/// The color of series `i`: JasperReports gives JFreeChart the template's series colors as
/// its whole palette, or, with a single one, in place of the first default color.
fn series_color(colors: &[Color], i: usize) -> Color {
    match colors.len() {
        0 => rgb(PALETTE[i % PALETTE.len()]),
        1 if i > 0 => rgb(PALETTE[i % PALETTE.len()]),
        n => colors[i % n],
    }
}

fn rgb(n: u32) -> Color {
    Color { r: (n >> 16) as u8, g: (n >> 8) as u8, b: n as u8 }
}

fn darker(c: Color, f: f32) -> Color {
    Color { r: (c.r as f32 * f) as u8, g: (c.g as f32 * f) as u8, b: (c.b as f32 * f) as u8 }
}

const GRID: Color = Color { r: 0xC0, g: 0xC0, b: 0xC0 };
const OUTLINE: Color = Color { r: 0x80, g: 0x80, b: 0x80 };

struct Canvas<'a> {
    fonts: &'a mut FontSet,
    items: Vec<PrintItem>,
    /// Text is measured as JFreeChart measures it drawing SVG (see [`ChartElement::svg`]).
    svg: bool,
    /// The chart's top left corner: the origin of the bitmaps of SVG charts.
    origin: (f64, f64),
}

#[derive(Clone, Copy)]
enum Anchor {
    Left,
    Center,
    Right,
}

impl Canvas<'_> {
    fn face(&mut self, font: &FontSpec) -> FaceId {
        self.fonts.resolve(font)
    }
    fn line_height(&mut self, font: &FontSpec) -> f32 {
        let id = self.face(font);
        self.fonts.face(id).line_height(font.size)
    }
    fn width(&mut self, font: &FontSpec, text: &str) -> f32 {
        let id = self.face(font);
        let face = self.fonts.face(id);
        match crate::shape::line(face, text, crate::shape::is_rtl(text), font.size) {
            Some((_, width)) => width,
            None => face.text_width(text, font.size),
        }
    }
    /// Text with its line box top at `top`.
    fn text(&mut self, font: &FontSpec, color: Color, text: &str, x: f32, top: f32, anchor: Anchor) {
        if text.is_empty() {
            return;
        }
        let id = self.face(font);
        let face = self.fonts.face(id);
        let shaped = crate::shape::line(face, text, crate::shape::is_rtl(text), font.size);
        let width = shaped.as_ref().map(|s| s.1).unwrap_or_else(|| face.text_width(text, font.size));
        let ascent = face.ascent(font.size);
        let x = match anchor {
            Anchor::Left => x,
            Anchor::Center => x - width / 2.0,
            Anchor::Right => x - width,
        };
        self.items.push(PrintItem::Text(PrintText {
            face: id,
            size: font.size,
            color,
            underline: false,
            strike: false,
            lines: vec![PrintLine {
                text: text.to_string(),
                x,
                baseline: top + ascent,
                width,
                word_spacing: 0.0,
                shaped: shaped.map(|s| s.0),
            }],
            standard: None,
            halign: crate::model::HAlign::Left,
        }));
    }
    /// Text with its baseline at `baseline`, as JasperReports puts JFreeChart's text on the
    /// page: the glyphs' outlines, each at its exact advance.
    fn text_baseline(&mut self, font: &FontSpec, color: Color, text: &str, x: f32, baseline: f32) {
        if text.is_empty() {
            return;
        }
        let id = self.face(font);
        let face = self.fonts.face(id);
        // JasperReports draws chart text as glyph outlines (its PDF graphics write shapes).
        let k = font.size as f64 / face.units_per_em as f64;
        let mut ops = Vec::new();
        match crate::shape::line(face, text, crate::shape::is_rtl(text), font.size) {
            Some((shaped, _)) => {
                let m = font.size as f64 / 1000.0;
                let mut pen = x as f64;
                for g in &shaped.glyphs {
                    face.glyph_id_outline(
                        g.id,
                        font.size as f64,
                        pen + g.dx as f64 * m,
                        baseline as f64 - g.dy as f64 * m,
                        &mut ops,
                    );
                    pen += g.exact as f64 * m;
                }
            }
            None => {
                let mut pen = x as f64;
                for c in text.chars().filter(|c| !c.is_control()) {
                    face.glyph_outline(c, font.size as f64, pen, baseline as f64, &mut ops);
                    pen += face.advance(face.glyph(c)) as f64 * k;
                }
            }
        }
        if !ops.is_empty() {
            // JasperReports' PDF graphics (iText's PdfGraphics2D) write a glyph's quadratic
            // segments as cubic ones whose first control point is the current point (`v`).
            let mut cur = (0.0, 0.0);
            let mut start = (0.0, 0.0);
            for op in ops.iter_mut() {
                match *op {
                    PathOp::Move(a, b) => {
                        cur = (a, b);
                        start = cur;
                    }
                    PathOp::Line(a, b) => cur = (a, b),
                    PathOp::Quad(qx, qy, ex, ey) => {
                        *op = PathOp::Cubic(cur.0, cur.1, qx, qy, ex, ey);
                        cur = (ex, ey);
                    }
                    PathOp::Cubic(_, _, _, _, ex, ey) => cur = (ex, ey),
                    PathOp::Close => cur = start,
                }
            }
            self.items.push(PrintItem::Path { ops: Arc::new(ops), fill: Some(color), stroke: None, dash: None });
        }
    }
    /// What `draw` draws, clipped to `r`.
    fn clipped(&mut self, r: jfree::Rect, draw: impl FnOnce(&mut Canvas)) {
        self.clipped_path(jfree::rect_ops(r.x, r.y, r.w, r.h), draw);
    }
    /// What `draw` draws, clipped to the inside of `clip`.
    fn clipped_path(&mut self, clip: Vec<PathOp>, draw: impl FnOnce(&mut Canvas)) {
        let mut inner = Canvas { fonts: &mut *self.fonts, items: Vec::new(), svg: self.svg, origin: self.origin };
        inner.path(clip, None, None, None);
        draw(&mut inner);
        let items = inner.items;
        self.items.push(PrintItem::Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], items));
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Option<Color>, stroke: Option<Pen>) {
        self.items.push(PrintItem::Rect { x, y, w, h, fill, stroke, radius: 0.0, path: false });
    }
    fn path(&mut self, ops: Vec<PathOp>, fill: Option<Color>, stroke: Option<(Color, f32)>, dash: Option<[f32; 2]>) {
        self.items.push(PrintItem::Path { ops: Arc::new(ops), fill, stroke, dash });
    }
    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: Color, width: f32, dash: Option<[f32; 2]>) {
        self.path(vec![PathOp::Move(x1 as f64, y1 as f64), PathOp::Line(x2 as f64, y2 as f64)], None, Some((color, width)), dash);
    }
}

fn pen(color: Color, width: f32) -> Pen {
    Pen { width, color, style: crate::model::LineStyle::Solid }
}

/// An elliptical arc from angle `a0` to `a1` (radians, y down) as cubic curves.
fn arc(ops: &mut Vec<PathOp>, cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, a1: f64, first: bool) {
    let segments = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = (a1 - a0) / segments as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let p = |a: f64| (cx + rx * a.cos(), cy + ry * a.sin());
    if first {
        let (x, y) = p(a0);
        ops.push(PathOp::Move(x, y));
    }
    for i in 0..segments {
        let (t0, t1) = (a0 + step * i as f64, a0 + step * (i + 1) as f64);
        let (x0, y0) = p(t0);
        let (x3, y3) = p(t1);
        let c1 = (x0 - k * rx * t0.sin(), y0 + k * ry * t0.cos());
        let c2 = (x3 + k * rx * t1.sin(), y3 - k * ry * t1.cos());
        ops.push(PathOp::Cubic(c1.0, c1.1, c2.0, c2.1, x3, y3));
    }
}

/// JFreeChart's standard tick units: 1, 2.5 and 5 times powers of ten.
fn tick_candidates() -> impl Iterator<Item = f64> {
    (-8..=12).flat_map(|e| [1.0, 2.5, 5.0].into_iter().map(move |m| m * 10f64.powi(e)))
}

struct Axis {
    lower: f64,
    upper: f64,
    unit: f64,
}

impl Axis {
    fn ticks(&self) -> Vec<f64> {
        let mut out = Vec::new();
        let mut v = (self.lower / self.unit).ceil() * self.unit;
        while v <= self.upper + self.unit * 1e-9 {
            out.push(if v.abs() < self.unit * 1e-9 { 0.0 } else { v });
            v += self.unit;
        }
        out
    }
}

fn auto_range(min: f64, max: f64, include_zero: bool, fixed: (Option<f64>, Option<f64>)) -> (f64, f64) {
    let (mut lo, mut hi) = (min, max);
    if !lo.is_finite() || !hi.is_finite() {
        lo = 0.0;
        hi = 1.0;
    }
    if include_zero {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    let range = hi - lo;
    let (lo, hi) = if range <= 0.0 {
        (lo - 0.5, hi + 0.5)
    } else {
        let lo2 = if include_zero && lo >= 0.0 { lo } else { lo - 0.05 * range };
        let hi2 = if include_zero && hi <= 0.0 { hi } else { hi + 0.05 * range };
        (lo2, hi2)
    };
    (fixed.0.unwrap_or(lo), fixed.1.unwrap_or(hi))
}

/// Width of the widest tick label for a tick unit (as JFreeChart estimates it, from the
/// range bounds rounded to the unit).
#[allow(clippy::too_many_arguments)]
fn widest_tick(cv: &mut Canvas, lower: f64, upper: f64, unit: f64, fmt: Option<&str>, locale: &Locale, font: &FontSpec) -> f32 {
    let hi = (upper / unit).floor() * unit;
    let lo = (lower / unit).ceil() * unit;
    cv.width(font, &tick_label(hi, unit, fmt, locale)).max(cv.width(font, &tick_label(lo, unit, fmt, locale)))
}

fn choose_unit(lower: f64, upper: f64, length: f32, mut min_spacing: impl FnMut(f64) -> f32) -> f64 {
    let range = (upper - lower).max(f64::MIN_POSITIVE);
    for unit in tick_candidates() {
        let px = (unit / range) as f32 * length;
        if px >= min_spacing(unit) {
            return unit;
        }
    }
    range
}

fn fmt_number(v: f64, pattern: Option<&str>, locale: &Locale) -> String {
    format_value(&Value::Double(v), Some(pattern.unwrap_or("#,##0.###")), locale)
}

/// Tick labels show as many decimals as the tick unit has (2.5 → "5.0"), like JFreeChart's
/// standard tick units.
fn tick_label(v: f64, unit: f64, pattern: Option<&str>, locale: &Locale) -> String {
    if pattern.is_some() {
        return fmt_number(v, pattern, locale);
    }
    let mut decimals = 0;
    while decimals < 8 && ((unit * 10f64.powi(decimals)).round() - unit * 10f64.powi(decimals)).abs() > 1e-9 {
        decimals += 1;
    }
    let p = if decimals == 0 { "#,##0".to_string() } else { format!("#,##0.{}", "0".repeat(decimals as usize)) };
    format_value(&Value::Double(v), Some(&p), locale)
}

/// Draws a chart in the box `x, y, w, h`.
#[allow(clippy::too_many_arguments)]
pub fn render(
    ch: &ChartElement,
    all: &HashMap<usize, DataState>,
    texts: &ChartTexts,
    fonts: &mut FontSet,
    locale: &Locale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> Vec<PrintItem> {
    let empty = DataState::default();
    let data = all.get(&ch.id).unwrap_or(&empty);
    let mut cv = Canvas { fonts, items: Vec::new(), svg: settings(ch).svg, origin: (0.0, 0.0) };
    let p = ch.boxed.padding;
    let (x, y, w, h) = (x + p.left, y + p.top, (w - p.left - p.right).max(0.0), (h - p.top - p.bottom).max(0.0));
    cv.origin = (x as f64, y as f64);
    if let Some(bg) = ch.backcolor {
        cv.rect(x, y, w, h, Some(bg), None);
    }
    let color_of = |i: usize| series_color(&ch.series_colors, i);

    if ch.kind == ChartKind::Meter {
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        titles_and_legend(&mut cv, ch, texts, &mut area, meter::legend_items(ch));
        meter::draw(&mut cv, ch, data, texts, locale, area);
        return clip_to(cv.items, x, y, w, h);
    }
    if matches!(
        ch.kind,
        ChartKind::Bar
            | ChartKind::Bar3D
            | ChartKind::StackedBar
            | ChartKind::StackedBar3D
            | ChartKind::Line
            | ChartKind::Area
            | ChartKind::StackedArea
    ) && ch.dataset.kind == DataKind::Category
    {
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        let items = data
            .series
            .iter()
            .enumerate()
            .map(|(i, label)| jfree::LegendItem { label: label.clone(), graphic: category::legend_graphic(ch, i, color_of(i)) })
            .collect();
        titles_and_legend(&mut cv, ch, texts, &mut area, items);
        category::draw(&mut cv, ch, data, texts, locale, area, &color_of);
        return clip_to(cv.items, x, y, w, h);
    }
    if ch.kind == ChartKind::MultiAxis
        && !ch.axes.is_empty()
        && ch.axes.iter().all(|(_, a)| {
            a.dataset.kind == DataKind::Category
                && matches!(
                    a.kind,
                    ChartKind::Bar | ChartKind::Bar3D | ChartKind::StackedBar | ChartKind::StackedBar3D | ChartKind::Line
                )
        })
    {
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        let parts: Vec<category::AxisPart> = ch
            .axes
            .iter()
            .enumerate()
            .map(|(i, (right, a))| category::AxisPart {
                chart: a,
                data: all.get(&a.id).unwrap_or(&empty),
                texts: texts.axes.get(i),
                right: *right,
            })
            .collect();
        let bases = category::part_bases(&parts);
        let mut items = Vec::new();
        for (p, &(shape_base, color_base)) in parts.iter().zip(&bases) {
            for (s, label) in p.data.series.iter().enumerate() {
                let color = category::part_color(p.chart, s, color_base);
                items.push(jfree::LegendItem {
                    label: label.clone(),
                    graphic: category::part_legend_graphic(p.chart, s, shape_base, color),
                });
            }
        }
        titles_and_legend(&mut cv, ch, texts, &mut area, items);
        category::draw_multi(&mut cv, ch, &parts, locale, area);
        return clip_to(cv.items, x, y, w, h);
    }
    if ch.kind == ChartKind::Gantt && ch.dataset.kind == DataKind::Gantt {
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        let items = data
            .gantt
            .iter()
            .enumerate()
            .map(|(i, (label, _))| jfree::LegendItem {
                label: label.clone(),
                graphic: jfree::LegendGraphic::square(color_of(i), None),
            })
            .collect();
        titles_and_legend(&mut cv, ch, texts, &mut area, items);
        category::draw_gantt(&mut cv, ch, data, texts, locale, area, &color_of);
        return clip_to(cv.items, x, y, w, h);
    }
    if matches!(
        ch.kind,
        ChartKind::XyLine
            | ChartKind::XyArea
            | ChartKind::Scatter
            | ChartKind::TimeSeries
            | ChartKind::XyBar
            | ChartKind::Bubble
            | ChartKind::Candlestick
            | ChartKind::HighLow
    ) && matches!(
        ch.dataset.kind,
        DataKind::Xy | DataKind::TimeSeries | DataKind::TimePeriod | DataKind::Xyz | DataKind::HighLow
    ) {
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        let names: Vec<String> = match ch.dataset.kind {
            DataKind::Xy => data.xy.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::Xyz => data.xyz.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::HighLow => data.ohlc.iter().map(|(k, _)| k.clone()).collect(),
            _ => data.periods.iter().map(|(k, _)| k.clone()).collect(),
        };
        let items = names
            .into_iter()
            .enumerate()
            .map(|(i, label)| jfree::LegendItem { label, graphic: xy::legend_graphic(ch, i, color_of(i)) })
            .collect();
        titles_and_legend(&mut cv, ch, texts, &mut area, items);
        xy::draw(&mut cv, ch, data, texts, locale, area, &color_of);
        return clip_to(cv.items, x, y, w, h);
    }
    if ch.kind.is_pie() && ch.dataset.kind == DataKind::Pie {
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        let section_color = |i: usize| color_of(pie::color_index(ch, data, i));
        let total: f64 = data.pie.iter().map(|(_, v)| v.max(0.0)).sum();
        let items = pie::legend_sections(data)
            .into_iter()
            .map(|i| {
                let (key, v) = &data.pie[i];
                let label = match &ch.legend_label_format {
                    Some(f) => pie_label(f, key, *v, total, locale),
                    None => key.clone(),
                };
                jfree::LegendItem { label, graphic: pie::legend_graphic(section_color(i)) }
            })
            .collect();
        titles_and_legend(&mut cv, ch, texts, &mut area, items);
        pie::draw(&mut cv, ch, data, locale, area, &section_color);
        return clip_to(cv.items, x, y, w, h);
    }
    if ch.kind == ChartKind::Thermometer {
        // ThermometerPlot has no legend items.
        let mut area = jfree::Rect::new(x as f64, y as f64, w as f64, h as f64);
        titles_and_legend(&mut cv, ch, texts, &mut area, Vec::new());
        thermometer::draw(&mut cv, ch, data, texts, locale, area);
        return clip_to(cv.items, x, y, w, h);
    }

    // Title and subtitle on top.
    let mut top = y;
    for (text, font, color) in
        [(&texts.title, &ch.title_font, ch.title_color), (&texts.subtitle, &ch.subtitle_font, ch.subtitle_color)]
    {
        if let Some(t) = text.as_deref().filter(|t| !t.is_empty()) {
            let lh = cv.line_height(font);
            cv.text(font, color, t, x + w / 2.0, top + 1.0, Anchor::Center);
            top += lh + 2.0;
        }
    }

    // Legend items: a label and its color.
    let names = |ch: &ChartElement, data: &DataState| -> Vec<String> {
        match ch.dataset.kind {
            DataKind::Pie => data.pie.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::Xy => data.xy.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::TimeSeries | DataKind::TimePeriod => data.periods.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::Xyz => data.xyz.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::HighLow => data.ohlc.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::Gantt => data.gantt.iter().map(|(k, _)| k.clone()).collect(),
            DataKind::Value => Vec::new(),
            DataKind::Category => data.series.clone(),
        }
    };
    let legend: Vec<(String, Color)> = if !ch.show_legend || matches!(ch.kind, ChartKind::Meter | ChartKind::Thermometer) {
        Vec::new()
    } else if ch.kind == ChartKind::MultiAxis {
        // Every chart's series, each chart going on with the palette where the last stopped.
        let mut out = Vec::new();
        for (_, a) in &ch.axes {
            let d = all.get(&a.id).unwrap_or(&empty);
            let base = out.len();
            for (i, n) in names(a, d).into_iter().enumerate() {
                out.push((n, a.series_colors.get(i).copied().unwrap_or_else(|| rgb(PALETTE[(base + i) % PALETTE.len()]))));
            }
        }
        out
    } else if let (DataKind::Pie, Some(f)) = (ch.dataset.kind, &ch.legend_label_format) {
        let total: f64 = data.pie.iter().map(|(_, v)| v.max(0.0)).sum();
        data.pie.iter().enumerate().map(|(i, (k, v))| (pie_label(f, k, *v, total, locale), color_of(i))).collect()
    } else {
        names(ch, data).into_iter().enumerate().map(|(i, k)| (k, color_of(i))).collect()
    };
    let mut bottom = y + h;
    if !legend.is_empty() {
        let lf = ch.legend_font.clone();
        let lh = cv.line_height(&lf);
        let widths: Vec<f32> = legend.iter().map(|(k, _)| cv.width(&lf, k)).collect();
        let line_chart =
            matches!(ch.kind, ChartKind::Line | ChartKind::XyLine | ChartKind::Scatter | ChartKind::TimeSeries) && ch.show_lines;
        let shape_w = if line_chart { 14.0 } else { 8.0 };
        let item_w = |i: usize| shape_w + 4.6 + widths[i];
        let total: f32 = (0..legend.len()).map(item_w).sum::<f32>() + 6.0 * (legend.len() as f32 - 1.0) + 7.0;
        let box_w = total.min(w - 2.0);
        let box_h = lh + 7.0;
        let (bx, by) = match ch.legend_position {
            Edge::Top => (x + (w - box_w) / 2.0, top + 1.0),
            _ => (x + (w - box_w) / 2.0, y + h - 1.5 - box_h),
        };
        if let Some(back) = ch.legend_back {
            cv.rect(bx, by, box_w, box_h, Some(back), None);
        }
        cv.rect(bx, by, box_w, box_h, None, Some(pen(Color::BLACK, 1.0)));
        let mut ix = bx + 3.5;
        let cy = by + box_h / 2.0;
        for (i, (label, color)) in legend.iter().enumerate() {
            let (color, series) = (*color, i);
            if ch.kind.is_pie() {
                let mut ops = Vec::new();
                arc(&mut ops, (ix + 4.0) as f64, cy as f64, 4.0, 4.0, 0.0, 2.0 * PI, true);
                ops.push(PathOp::Close);
                cv.path(ops, Some(color), Some((OUTLINE, 0.5)), None);
            } else if line_chart {
                cv.line(ix, cy, ix + 14.0, cy, color, 1.0, None);
                if ch.show_shapes {
                    shape(&mut cv, series, ix + 7.0, cy, color);
                }
            } else if matches!(ch.kind, ChartKind::Scatter | ChartKind::Bubble) {
                shape(&mut cv, if ch.kind == ChartKind::Bubble { 1 } else { series }, ix + 4.0, cy, color);
            } else {
                cv.rect(ix, cy - 4.0, 8.0, 8.0, Some(color), None);
            }
            cv.text(&lf, ch.legend_color, label, ix + shape_w + 4.6, cy - lh / 2.0, Anchor::Left);
            ix += item_w(i) + 6.0;
            if ix > bx + box_w {
                break;
            }
        }
        match ch.legend_position {
            Edge::Top => top = by + box_h + 2.0,
            _ => bottom = by - 2.0,
        }
    }

    // Plot area (JFreeChart's default plot insets: 4 top/bottom, 8 left/right).
    let (px, py, pw, ph) = (x + 8.0, top + 4.0, (w - 16.0).max(1.0), (bottom - top - 8.0).max(1.0));
    match ch.kind {
        _ if ch.kind.is_pie() => pie(&mut cv, ch, data, locale, px, py, pw, ph, &color_of),
        ChartKind::Gantt => {}
        ChartKind::MultiAxis => multi_axis(&mut cv, ch, all, texts, locale, px, py, pw, ph),
        ChartKind::XyLine | ChartKind::XyArea | ChartKind::Scatter if ch.dataset.kind == DataKind::Xy => {
            xy_or_category(&mut cv, ch, data, texts, locale, px, py, pw, ph, &color_of)
        }
        _ if ch.kind.is_xy() => xy_plot(&mut cv, ch, data, texts, locale, px, py, pw, ph, &color_of),
        _ => xy_or_category(&mut cv, ch, data, texts, locale, px, py, pw, ph, &color_of),
    }
    cv.items
}

/// `items` clipped to the chart's box, as JasperReports clips what JFreeChart draws.
fn clip_to(items: Vec<PrintItem>, x: f32, y: f32, w: f32, h: f32) -> Vec<PrintItem> {
    let mut all = vec![PrintItem::Path {
        ops: Arc::new(jfree::rect_ops(x as f64, y as f64, w as f64, h as f64)),
        fill: None,
        stroke: None,
        dash: None,
    }];
    all.extend(items);
    vec![PrintItem::Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], all)]
}

/// JFreeChart's titles around the chart: the title, then the legend, then the subtitle, each
/// taking its room from `area`.
fn titles_and_legend(
    cv: &mut Canvas,
    ch: &ChartElement,
    texts: &ChartTexts,
    area: &mut jfree::Rect,
    items: Vec<jfree::LegendItem>,
) {
    if let Some(t) = texts.title.as_deref() {
        jfree::text_title(cv, area, t, &ch.title_font, ch.title_color, jfree::Side::Top);
    }
    if ch.show_legend {
        let side = match ch.legend_position {
            Edge::Top => jfree::Side::Top,
            Edge::Left => jfree::Side::Left,
            Edge::Right => jfree::Side::Right,
            Edge::Bottom => jfree::Side::Bottom,
        };
        jfree::legend(cv, area, &items, &ch.legend_font, ch.legend_color, ch.legend_back, side);
    }
    if let Some(t) = texts.subtitle.as_deref() {
        jfree::text_title(cv, area, t, &ch.subtitle_font, ch.subtitle_color, jfree::Side::Top);
    }
}

/// The shape JFreeChart's default drawing supplier gives series `series` (size 6), around
/// the origin, with its bounds.
fn series_shape(series: usize) -> (Vec<PathOp>, (f64, f64, f64, f64)) {
    let mut ops = Vec::new();
    let poly = |pts: &[(f64, f64)], ops: &mut Vec<PathOp>| {
        ops.push(PathOp::Move(pts[0].0, pts[0].1));
        for p in &pts[1..] {
            ops.push(PathOp::Line(p.0, p.1));
        }
        ops.push(PathOp::Close);
    };
    let bounds = match series % 10 {
        0 => {
            ops = jfree::rect_ops(-3.0, -3.0, 6.0, 6.0);
            (-3.0, -3.0, 6.0, 6.0)
        }
        1 => {
            arc(&mut ops, 0.0, 0.0, 3.0, 3.0, 0.0, 2.0 * PI, true);
            ops.push(PathOp::Close);
            (-3.0, -3.0, 6.0, 6.0)
        }
        2 => {
            poly(&[(0.0, -3.0), (3.0, 3.0), (-3.0, 3.0)], &mut ops);
            (-3.0, -3.0, 6.0, 6.0)
        }
        3 => {
            poly(&[(0.0, -3.0), (3.0, 0.0), (0.0, 3.0), (-3.0, 0.0)], &mut ops);
            (-3.0, -3.0, 6.0, 6.0)
        }
        4 => {
            ops = jfree::rect_ops(-3.0, -1.5, 6.0, 3.0);
            (-3.0, -1.5, 6.0, 3.0)
        }
        5 => {
            poly(&[(-3.0, -3.0), (3.0, -3.0), (0.0, 3.0)], &mut ops);
            (-3.0, -3.0, 6.0, 6.0)
        }
        6 => {
            arc(&mut ops, 0.0, 0.0, 3.0, 1.5, 0.0, 2.0 * PI, true);
            ops.push(PathOp::Close);
            (-3.0, -1.5, 6.0, 3.0)
        }
        7 => {
            poly(&[(-3.0, -3.0), (3.0, 0.0), (-3.0, 3.0)], &mut ops);
            (-3.0, -3.0, 6.0, 6.0)
        }
        8 => {
            ops = jfree::rect_ops(-1.5, -3.0, 3.0, 6.0);
            (-1.5, -3.0, 3.0, 6.0)
        }
        _ => {
            poly(&[(-3.0, 0.0), (3.0, -3.0), (3.0, 3.0)], &mut ops);
            (-3.0, -3.0, 6.0, 6.0)
        }
    };
    (ops, bounds)
}

/// A pie section's label from a `labelFormat` such as `{0} {2}`: `{0}` the key, `{1}` the
/// value, `{2}` its share (JFreeChart's StandardPieSectionLabelGenerator).
fn pie_label(format: &str, key: &str, value: f64, total: f64, locale: &Locale) -> String {
    let share = if total > 0.0 { value / total } else { 0.0 };
    format
        .replace("{0}", key)
        .replace("{1}", &fmt_number(value, Some("#,##0.###"), locale))
        .replace("{2}", &format_value(&Value::Double(share), Some("#,##0%"), locale))
}

/// The shape JFreeChart's default drawing supplier gives a series (size 6).
fn shape(cv: &mut Canvas, series: usize, cx: f32, cy: f32, color: Color) {
    let (cx, cy) = (cx as f64, cy as f64);
    let mut ops = Vec::new();
    let poly = |pts: &[(f64, f64)], ops: &mut Vec<PathOp>| {
        ops.push(PathOp::Move(cx + pts[0].0, cy + pts[0].1));
        for p in &pts[1..] {
            ops.push(PathOp::Line(cx + p.0, cy + p.1));
        }
        ops.push(PathOp::Close);
    };
    match series % 10 {
        0 => poly(&[(-3.0, -3.0), (3.0, -3.0), (3.0, 3.0), (-3.0, 3.0)], &mut ops),
        1 => {
            arc(&mut ops, cx, cy, 3.0, 3.0, 0.0, 2.0 * PI, true);
            ops.push(PathOp::Close);
        }
        2 => poly(&[(0.0, -3.0), (3.0, 3.0), (-3.0, 3.0)], &mut ops),
        3 => poly(&[(0.0, -3.0), (3.0, 0.0), (0.0, 3.0), (-3.0, 0.0)], &mut ops),
        4 => poly(&[(-3.0, -1.5), (3.0, -1.5), (3.0, 1.5), (-3.0, 1.5)], &mut ops),
        5 => poly(&[(-3.0, -3.0), (3.0, -3.0), (0.0, 3.0)], &mut ops),
        6 => {
            arc(&mut ops, cx, cy, 3.0, 1.5, 0.0, 2.0 * PI, true);
            ops.push(PathOp::Close);
        }
        7 => poly(&[(-3.0, -3.0), (3.0, 0.0), (-3.0, 3.0)], &mut ops),
        8 => poly(&[(-1.5, -3.0), (1.5, -3.0), (1.5, 3.0), (-1.5, 3.0)], &mut ops),
        _ => poly(&[(3.0, -3.0), (3.0, 3.0), (-3.0, 0.0)], &mut ops),
    }
    cv.path(ops, Some(color), None, None);
}

#[allow(clippy::too_many_arguments)]
fn pie(
    cv: &mut Canvas,
    ch: &ChartElement,
    data: &DataState,
    locale: &Locale,
    px: f32,
    py: f32,
    pw: f32,
    ph: f32,
    color_of: &dyn Fn(usize) -> Color,
) {
    let total: f64 = data.pie.iter().map(|(_, v)| v.max(0.0)).sum();
    if total <= 0.0 {
        return;
    }
    let three_d = ch.kind.is_3d();
    let r = (pw.min(ph) as f64) * 0.84 / 2.0;
    let (cx, cy) = ((px + pw / 2.0) as f64, (py + ph / 2.0) as f64);
    let (rx, ry, depth) = if three_d { (r, r * 0.75, r * 0.2) } else { (r, r, 0.0) };
    let cy = cy - depth / 2.0;
    // Slices clockwise from twelve o'clock.
    let mut slices = Vec::new();
    let mut a = -PI / 2.0;
    for (i, (key, v)) in data.pie.iter().enumerate() {
        let sweep = 2.0 * PI * v.max(0.0) / total;
        let label = match &ch.label_format {
            Some(f) => pie_label(f, key, *v, total, locale),
            None => key.clone(),
        };
        slices.push((i, label, a, a + sweep));
        a += sweep;
    }
    let wedge = |cx: f64, cy: f64, a0: f64, a1: f64| {
        let mut ops = vec![PathOp::Move(cx, cy)];
        let (sx, sy) = (cx + rx * a0.cos(), cy + ry * a0.sin());
        ops.push(PathOp::Line(sx, sy));
        arc(&mut ops, cx, cy, rx, ry, a0, a1, false);
        ops.push(PathOp::Close);
        ops
    };
    if three_d {
        // The visible side of the pie, front half.
        for &(i, _, a0, a1) in &slices {
            let (s0, s1) = (a0.max(0.0), a1.min(PI));
            if s1 > s0 {
                let mut ops = Vec::new();
                arc(&mut ops, cx, cy, rx, ry, s0, s1, true);
                ops.push(PathOp::Line(cx + rx * s1.cos(), cy + depth + ry * s1.sin()));
                arc(&mut ops, cx, cy + depth, rx, ry, s1, s0, false);
                ops.push(PathOp::Close);
                cv.path(ops, Some(darker(color_of(i), 0.7)), Some((OUTLINE, 0.5)), None);
            }
            // Slices that wrap past 2π.
            let (s0, s1) = ((a0 - 2.0 * PI).max(0.0), (a1 - 2.0 * PI).min(PI));
            if s1 > s0 {
                let mut ops = Vec::new();
                arc(&mut ops, cx, cy, rx, ry, s0, s1, true);
                ops.push(PathOp::Line(cx + rx * s1.cos(), cy + depth + ry * s1.sin()));
                arc(&mut ops, cx, cy + depth, rx, ry, s1, s0, false);
                ops.push(PathOp::Close);
                cv.path(ops, Some(darker(color_of(i), 0.7)), Some((OUTLINE, 0.5)), None);
            }
        }
    } else {
        // JFreeChart's shadow: the pie offset by 4 points in gray.
        for &(_, _, a0, a1) in &slices {
            cv.path(wedge(cx + 4.0, cy + 4.0, a0, a1), Some(OUTLINE), None, None);
        }
    }
    for &(i, _, a0, a1) in &slices {
        cv.path(wedge(cx, cy, a0, a1), Some(color_of(i)), Some((OUTLINE, 0.5)), None);
    }

    if !ch.show_labels {
        return;
    }
    // Labels in boxes on both sides, linked to their slices.
    let font = ch.font.clone();
    let lh = cv.line_height(&font);
    let box_h = lh + 4.0;
    let mut sides: [Vec<(f64, f64, f64, String, f64)>; 2] = [Vec::new(), Vec::new()];
    for (_, key, a0, a1) in &slices {
        let mid = (a0 + a1) / 2.0;
        let right = mid.cos() >= 0.0;
        let start = (cx + 0.88 * rx * mid.cos(), cy + 0.88 * ry * mid.sin());
        let elbow =
            (cx + 1.03 * rx * mid.cos(), cy + 1.03 * ry * mid.sin() + if three_d && mid.sin() > 0.0 { depth } else { 0.0 });
        sides[right as usize].push((elbow.1, start.0, start.1, key.clone(), elbow.0));
    }
    for (side, labels) in sides.iter_mut().enumerate() {
        labels.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        // Spread labels so their boxes do not overlap.
        let min_gap = box_h as f64 + 2.0;
        for i in 1..labels.len() {
            if labels[i].0 - labels[i - 1].0 < min_gap {
                labels[i].0 = labels[i - 1].0 + min_gap;
            }
        }
        for (ey, sx, sy, key, ex) in labels.iter() {
            let tw = cv.width(&font, key) as f64;
            let bw = tw + 3.0;
            let edge = if side == 1 { cx + 1.083 * rx } else { cx - 1.083 * rx };
            let bx = if side == 1 { edge } else { edge - bw };
            let by = ey - box_h as f64 / 2.0;
            cv.path(
                vec![PathOp::Move(*sx, *sy), PathOp::Line(*ex, *ey), PathOp::Line(edge, *ey)],
                None,
                Some((Color::BLACK, 0.5)),
                None,
            );
            cv.rect((bx + 2.0) as f32, (by + 2.0) as f32, bw as f32, box_h, Some(Color { r: 0x97, g: 0x97, b: 0x97 }), None);
            cv.rect(bx as f32, by as f32, bw as f32, box_h, Some(Color::WHITE), Some(pen(Color::BLACK, 0.5)));
            cv.text(&font, Color::BLACK, key, (bx + 1.5) as f32, (by + 2.0) as f32, Anchor::Left);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn xy_or_category(
    cv: &mut Canvas,
    ch: &ChartElement,
    data: &DataState,
    texts: &ChartTexts,
    locale: &Locale,
    px: f32,
    py: f32,
    pw: f32,
    ph: f32,
    color_of: &dyn Fn(usize) -> Color,
) {
    let font = ch.font.clone();
    let lh = cv.line_height(&font);
    let horizontal = ch.horizontal && !ch.kind.is_xy();
    let fmt = ch.value_format.as_deref();

    // Value range (y for category and XY charts).
    let (mut vmin, mut vmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let ncat = data.categories.len();
    let nser = data.series.len();
    if ch.kind.is_xy() {
        for (_, pts) in &data.xy {
            for &(_, yv) in pts {
                vmin = vmin.min(yv);
                vmax = vmax.max(yv);
            }
        }
    } else if ch.kind.is_stacked() {
        for c in 0..ncat {
            let (mut pos, mut neg) = (0.0, 0.0);
            for s in 0..nser {
                let v = data.values.get(&(s, c)).copied().unwrap_or(0.0);
                if v >= 0.0 {
                    pos += v
                } else {
                    neg += v
                }
            }
            vmin = vmin.min(neg);
            vmax = vmax.max(pos);
        }
    } else {
        for v in data.values.values() {
            vmin = vmin.min(*v);
            vmax = vmax.max(*v);
        }
    }
    // Scatter plots do not stretch their range to zero (JFreeChart's createScatterPlot).
    let (vlo, vhi) = auto_range(vmin, vmax, ch.kind != ChartKind::Scatter, (texts.value_min, texts.value_max));

    // Domain range for XY charts.
    let (mut xmin, mut xmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for (_, pts) in &data.xy {
        for &(xv, _) in pts {
            xmin = xmin.min(xv);
            xmax = xmax.max(xv);
        }
    }
    let (xlo, xhi) = auto_range(xmin, xmax, false, (None, None));

    // Space taken by the axes around the data area.
    let value_label = texts.value_label.clone().filter(|s| !s.is_empty());
    let cat_label = texts.category_label.clone().filter(|s| !s.is_empty());
    let axis_label_h = |present: bool| if present { lh + 6.0 } else { 0.0 };

    let (dx, dy, dw, dh);
    let value_axis;
    if !horizontal {
        // Vertical value axis on the left, domain axis at the bottom.
        let bottom_h = 4.0 + (lh + 4.0) + axis_label_h(cat_label.is_some());
        dh = (ph - bottom_h).max(1.0);
        let unit = choose_unit(vlo, vhi, dh, |_| lh + 4.0);
        value_axis = Axis { lower: vlo, upper: vhi, unit };
        let widest = value_axis
            .ticks()
            .into_iter()
            .map(|t| {
                let l = tick_label(t, value_axis.unit, fmt, locale);
                cv.width(&font, &l)
            })
            .fold(0.0f32, f32::max);
        let left_w = axis_label_h(value_label.is_some()) + 4.0 + widest + 4.0 + 2.0;
        dx = px + left_w;
        dy = py;
        dw = (pw - left_w).max(1.0);
    } else {
        // Horizontal bars: categories on the left, value axis on top.
        let widest_cat = data.categories.iter().map(|c| cv.width(&font, c)).fold(0.0f32, f32::max);
        let left_w = axis_label_h(cat_label.is_some()) + 4.0 + widest_cat + 4.0;
        let top_h = axis_label_h(value_label.is_some()) + (lh + 4.0) + 2.0;
        dx = px + left_w;
        dy = py + top_h;
        dw = (pw - left_w).max(1.0);
        dh = (ph - top_h).max(1.0);
        let unit = choose_unit(vlo, vhi, dw, |u| widest_tick(cv, vlo, vhi, u, fmt, locale, &font) + 8.0);
        value_axis = Axis { lower: vlo, upper: vhi, unit };
    }
    let xaxis = if ch.kind.is_xy() {
        Some(Axis {
            lower: xlo,
            upper: xhi,
            unit: choose_unit(xlo, xhi, dw, |u| widest_tick(cv, xlo, xhi, u, fmt, locale, &font) + 8.0),
        })
    } else {
        None
    };

    if let Some(bg) = ch.plot_backcolor {
        cv.rect(dx, dy, dw, dh, Some(bg), None);
    }
    // Value position along its axis.
    let vpos = |v: f64| -> f32 {
        let t = ((v - value_axis.lower) / (value_axis.upper - value_axis.lower)) as f32;
        if horizontal {
            dx + t * dw
        } else {
            dy + dh - t * dh
        }
    };
    let xpos = |v: f64| -> f32 {
        let a = xaxis.as_ref().expect("xy axis");
        dx + ((v - a.lower) / (a.upper - a.lower)) as f32 * dw
    };

    // Gridlines and value axis.
    let dash = Some([2.0, 2.0]);
    for t in value_axis.ticks() {
        let p = vpos(t);
        let label = tick_label(t, value_axis.unit, fmt, locale);
        if horizontal {
            cv.line(p, dy, p, dy + dh, GRID, 0.5, dash);
            cv.line(p, dy - 2.0, p, dy, Color::BLACK, 0.5, None);
            cv.text(&font, Color::BLACK, &label, p, dy - 2.0 - 4.0 - lh + 2.0, Anchor::Center);
        } else {
            cv.line(dx, p, dx + dw, p, GRID, 0.5, dash);
            cv.line(dx - 2.0, p, dx, p, Color::BLACK, 0.5, None);
            cv.text(&font, Color::BLACK, &label, dx - 6.0, p - lh / 2.0, Anchor::Right);
        }
    }
    if let Some(a) = &xaxis {
        for t in a.ticks() {
            let p = xpos(t);
            cv.line(p, dy, p, dy + dh, GRID, 0.5, dash);
            cv.line(p, dy + dh, p, dy + dh + 2.0, Color::BLACK, 0.5, None);
            cv.text(&font, Color::BLACK, &tick_label(t, a.unit, fmt, locale), p, dy + dh + 4.0 + 2.0, Anchor::Center);
        }
    }
    // Axis lines.
    if horizontal {
        cv.line(dx, dy, dx + dw, dy, Color::BLACK, 0.5, None);
        cv.line(dx, dy, dx, dy + dh, Color::BLACK, 0.5, None);
    } else {
        cv.line(dx, dy, dx, dy + dh, Color::BLACK, 0.5, None);
        cv.line(dx, dy + dh, dx + dw, dy + dh, Color::BLACK, 0.5, None);
    }

    // Axis labels.
    if let Some(l) = &value_label {
        if horizontal {
            cv.text(&font, Color::BLACK, l, dx + dw / 2.0, py + 3.0, Anchor::Center);
        } else {
            rotated_label(cv, &font, l, px + 3.0, dy + dh / 2.0);
        }
    }
    if let Some(l) = &cat_label {
        if horizontal {
            rotated_label(cv, &font, l, px + 3.0, dy + dh / 2.0);
        } else {
            cv.text(&font, Color::BLACK, l, dx + dw / 2.0, dy + dh + 4.0 + lh + 4.0 + 3.0, Anchor::Center);
        }
    }

    if ch.kind.is_xy() {
        draw_xy(cv, ch, data, &xpos, &vpos, value_axis.lower.max(0.0).min(value_axis.upper), color_of);
        return;
    }

    // Category bands: 5% margins at both ends, 20% between categories.
    let span = if horizontal { dh } else { dw };
    let start = if horizontal { dy } else { dx };
    let cat_margin = if ncat > 1 { 0.2 } else { 0.0 };
    let cat_w = span * (1.0 - 0.1 - cat_margin) / ncat.max(1) as f32;
    let cat_gap = if ncat > 1 { span * cat_margin / (ncat - 1) as f32 } else { 0.0 };
    let cat_start = |c: usize| start + 0.05 * span + c as f32 * (cat_w + cat_gap);
    for (c, name) in data.categories.iter().enumerate() {
        let mid = cat_start(c) + cat_w / 2.0;
        if horizontal {
            cv.text(&font, Color::BLACK, name, dx - 4.0, mid - lh / 2.0, Anchor::Right);
        } else {
            cv.text(&font, Color::BLACK, name, mid, dy + dh + 4.0 + 2.0, Anchor::Center);
        }
    }
    let base = vpos(0.0f64.max(value_axis.lower).min(value_axis.upper));
    let value = |s: usize, c: usize| data.values.get(&(s, c)).copied();

    match ch.kind {
        ChartKind::Bar | ChartKind::Bar3D => {
            let item_margin = if nser > 1 { 0.2 } else { 0.0 };
            let used = span * (1.0 - 0.1 - cat_margin - item_margin);
            let bar_w = used / (nser * ncat).max(1) as f32;
            let item_gap = if nser > 1 { span * item_margin / (ncat * (nser - 1)).max(1) as f32 } else { 0.0 };
            for c in 0..ncat {
                for s in 0..nser {
                    let Some(v) = value(s, c) else { continue };
                    let a = cat_start(c) + s as f32 * (bar_w + item_gap);
                    let (p0, p1) = (base, vpos(v));
                    bar(cv, horizontal, a, bar_w, p0, p1, color_of(s), ch.kind.is_3d());
                }
            }
        }
        ChartKind::StackedBar | ChartKind::StackedBar3D => {
            for c in 0..ncat {
                let (mut pos, mut neg) = (0.0, 0.0);
                for s in 0..nser {
                    let Some(v) = value(s, c) else { continue };
                    let from = if v >= 0.0 { pos } else { neg };
                    let to = from + v;
                    if v >= 0.0 {
                        pos = to
                    } else {
                        neg = to
                    }
                    bar(cv, horizontal, cat_start(c), cat_w, vpos(from), vpos(to), color_of(s), ch.kind.is_3d());
                }
            }
        }
        ChartKind::Line => {
            for s in 0..nser {
                let pts: Vec<(f32, f32)> = (0..ncat)
                    .filter_map(|c| value(s, c).map(|v| (cat_start(c) + cat_w / 2.0, vpos(v))))
                    .map(|(a, b)| if horizontal { (b, a) } else { (a, b) })
                    .collect();
                if ch.show_lines && pts.len() > 1 {
                    let mut ops = vec![PathOp::Move(pts[0].0 as f64, pts[0].1 as f64)];
                    ops.extend(pts[1..].iter().map(|p| PathOp::Line(p.0 as f64, p.1 as f64)));
                    cv.path(ops, None, Some((color_of(s), 1.0)), None);
                }
                if ch.show_shapes {
                    for p in &pts {
                        shape(cv, s, p.0, p.1, color_of(s));
                    }
                }
            }
        }
        ChartKind::Area | ChartKind::StackedArea => {
            let mut acc = vec![0.0f64; ncat];
            for s in 0..nser {
                let mut top: Vec<(f32, f32)> = Vec::new();
                let mut bottom: Vec<(f32, f32)> = Vec::new();
                for (c, acc_c) in acc.iter_mut().enumerate() {
                    let v = value(s, c).unwrap_or(0.0);
                    let lo = if ch.kind == ChartKind::StackedArea { *acc_c } else { 0.0 };
                    let hi = lo + v;
                    if ch.kind == ChartKind::StackedArea {
                        *acc_c = hi;
                    }
                    top.push((cat_start(c) + cat_w / 2.0, vpos(hi)));
                    bottom.push((cat_start(c) + cat_w / 2.0, vpos(lo)));
                }
                if top.is_empty() {
                    continue;
                }
                // Taper to the base at both ends, like JFreeChart's AreaRenderer.
                let first_x = cat_start(0);
                let last_x = cat_start(ncat - 1) + cat_w;
                let mut pts = vec![(first_x, *bottom.first().map(|p| &p.1).unwrap_or(&base))];
                pts.extend(top.iter().copied());
                pts.push((last_x, bottom.last().map(|p| p.1).unwrap_or(base)));
                pts.extend(bottom.iter().rev().copied());
                let pts: Vec<(f32, f32)> = pts.into_iter().map(|(a, b)| if horizontal { (b, a) } else { (a, b) }).collect();
                let mut ops = vec![PathOp::Move(pts[0].0 as f64, pts[0].1 as f64)];
                ops.extend(pts[1..].iter().map(|p| PathOp::Line(p.0 as f64, p.1 as f64)));
                ops.push(PathOp::Close);
                cv.path(ops, Some(color_of(s)), None, None);
            }
        }
        _ => {}
    }
    let _ = (nser, ch.kind.is_bar());
}

#[allow(clippy::too_many_arguments)]
fn bar(cv: &mut Canvas, horizontal: bool, a: f32, width: f32, p0: f32, p1: f32, color: Color, three_d: bool) {
    let (lo, hi) = (p0.min(p1), p0.max(p1));
    let (x, y, w, h) = if horizontal { (lo, a, hi - lo, width) } else { (a, lo, width, hi - lo) };
    if three_d {
        let d = (width * 0.35).min(8.0);
        let (x0, y0, x1, y1) = (x as f64, y as f64, (x + w) as f64, (y + h) as f64);
        let d = d as f64;
        // Top and side faces, offset up and to the right.
        let top = vec![
            PathOp::Move(x0, y0),
            PathOp::Line(x0 + d, y0 - d),
            PathOp::Line(x1 + d, y0 - d),
            PathOp::Line(x1, y0),
            PathOp::Close,
        ];
        let side = vec![
            PathOp::Move(x1, y0),
            PathOp::Line(x1 + d, y0 - d),
            PathOp::Line(x1 + d, y1 - d),
            PathOp::Line(x1, y1),
            PathOp::Close,
        ];
        cv.path(top, Some(darker(color, 0.85)), None, None);
        cv.path(side, Some(darker(color, 0.7)), None, None);
    }
    cv.rect(x, y, w, h, Some(color), None);
}

fn rotated_label(cv: &mut Canvas, font: &FontSpec, text: &str, left: f32, center_y: f32) {
    let w = cv.width(font, text);
    let mut inner = Canvas { fonts: cv.fonts, items: Vec::new(), svg: cv.svg, origin: cv.origin };
    inner.text(font, Color::BLACK, text, 0.0, 0.0, Anchor::Left);
    let items = inner.items;
    // Turn 90° counter-clockwise: local (u, v) lands at (left + v, center_y + w/2 - u).
    cv.items.push(PrintItem::Transform([0.0, -1.0, 1.0, 0.0, left, center_y + w / 2.0], items));
}

#[allow(clippy::too_many_arguments)]
fn draw_xy(
    cv: &mut Canvas,
    ch: &ChartElement,
    data: &DataState,
    xpos: &dyn Fn(f64) -> f32,
    vpos: &dyn Fn(f64) -> f32,
    base_value: f64,
    color_of: &dyn Fn(usize) -> Color,
) {
    for (s, (_, pts)) in data.xy.iter().enumerate() {
        let mut pts = pts.clone();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let screen: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (xpos(x), vpos(y))).collect();
        if screen.is_empty() {
            continue;
        }
        match ch.kind {
            ChartKind::XyArea => {
                let base = vpos(base_value);
                let mut ops = vec![PathOp::Move(screen[0].0 as f64, base as f64)];
                ops.extend(screen.iter().map(|p| PathOp::Line(p.0 as f64, p.1 as f64)));
                ops.push(PathOp::Line(screen[screen.len() - 1].0 as f64, base as f64));
                ops.push(PathOp::Close);
                cv.path(ops, Some(color_of(s)), None, None);
            }
            _ => {
                if ch.show_lines && screen.len() > 1 {
                    let mut ops = vec![PathOp::Move(screen[0].0 as f64, screen[0].1 as f64)];
                    ops.extend(screen[1..].iter().map(|p| PathOp::Line(p.0 as f64, p.1 as f64)));
                    cv.path(ops, None, Some((color_of(s), 1.0)), None);
                }
                if ch.show_shapes {
                    for p in &screen {
                        shape(cv, s, p.0, p.1, color_of(s));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Axes shared by the charts below: numbers or dates, with their ticks and labels.

/// An axis: its range and the ticks it labels.
struct Scale {
    lower: f64,
    upper: f64,
    ticks: Vec<(f64, String)>,
}

impl Scale {
    /// Position of `v` along `length` points from `start` (upwards when `vertical`).
    fn at(&self, v: f64, start: f32, length: f32, vertical: bool) -> f32 {
        let t = ((v - self.lower) / (self.upper - self.lower)) as f32;
        if vertical {
            start + length - t * length
        } else {
            start + t * length
        }
    }
}

/// A number axis over `lo..hi` drawn `length` points long.
fn number_scale(
    cv: &mut Canvas,
    lo: f64,
    hi: f64,
    length: f32,
    vertical: bool,
    fmt: Option<&str>,
    locale: &Locale,
    font: &FontSpec,
) -> Scale {
    let lh = cv.line_height(font);
    let unit = if vertical {
        choose_unit(lo, hi, length, |_| lh + 4.0)
    } else {
        choose_unit(lo, hi, length, |u| widest_tick(cv, lo, hi, u, fmt, locale, font) + 8.0)
    };
    let axis = Axis { lower: lo, upper: hi, unit };
    let ticks = axis.ticks().into_iter().map(|t| (t, tick_label(t, unit, fmt, locale))).collect();
    Scale { lower: lo, upper: hi, ticks }
}

/// JFreeChart's standard date tick units: the calendar field, how many of it, the label pattern.
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

#[derive(Clone, Copy, PartialEq)]
enum DateField {
    Milli,
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

impl DateField {
    /// Rough length in milliseconds, to pick a unit.
    fn millis(self) -> f64 {
        match self {
            DateField::Milli => 1.0,
            DateField::Second => 1e3,
            DateField::Minute => 6e4,
            DateField::Hour => 3.6e6,
            DateField::Day => 8.64e7,
            DateField::Month => 2.6298e9,
            DateField::Year => 3.15576e10,
        }
    }
}

fn to_date(ms: f64) -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp_millis(ms.round() as i64).map(|d| d.naive_utc()).unwrap_or_default()
}

fn date_label(ms: f64, pattern: &str, locale: &Locale) -> String {
    format_value(&Value::Date(to_date(ms)), Some(pattern), locale)
}

/// The ticks of a date unit from `lo` to `hi`, aligned the way JFreeChart's DateAxis aligns
/// them: the field rounded down to a multiple of the count.
fn date_ticks(lo: f64, hi: f64, field: DateField, count: i64) -> Vec<f64> {
    use chrono::{Datelike, Duration, NaiveDate, Timelike};
    let t = to_date(lo);
    let date = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d).and_then(|d| d.and_hms_opt(0, 0, 0));
    let c = count as u32;
    let start = match field {
        DateField::Milli => Some(to_date((lo / count as f64).floor() * count as f64)),
        DateField::Second => t.with_nanosecond(0).and_then(|v| v.with_second(t.second() / c * c)),
        DateField::Minute => t.with_nanosecond(0).and_then(|v| v.with_second(0)).and_then(|v| v.with_minute(t.minute() / c * c)),
        DateField::Hour => t
            .with_nanosecond(0)
            .and_then(|v| v.with_second(0))
            .and_then(|v| v.with_minute(0))
            .and_then(|v| v.with_hour(t.hour() / c * c)),
        DateField::Day => date(t.year(), t.month(), 1).map(|d| d + Duration::days((t.day() as i64 / count) * count - 1)),
        DateField::Month => date(t.year(), (t.month0() / c * c) + 1, 1),
        DateField::Year => date((t.year() / count as i32) * count as i32, 1, 1),
    };
    let Some(mut cur) = start else { return Vec::new() };
    let mut out = Vec::new();
    for _ in 0..1000 {
        let ms = millis(cur);
        if ms > hi {
            break;
        }
        if ms >= lo {
            out.push(ms);
        }
        cur = match field {
            DateField::Milli => cur + Duration::milliseconds(count),
            DateField::Second => cur + Duration::seconds(count),
            DateField::Minute => cur + Duration::minutes(count),
            DateField::Hour => cur + Duration::hours(count),
            DateField::Day => cur + Duration::days(count),
            DateField::Month => {
                let m = cur.month0() as i64 + count;
                match date(cur.year() + (m / 12) as i32, (m % 12) as u32 + 1, 1) {
                    Some(d) => d,
                    None => break,
                }
            }
            DateField::Year => match date(cur.year() + count as i32, 1, 1) {
                Some(d) => d,
                None => break,
            },
        };
    }
    out
}

/// A date axis over `lo..hi` (milliseconds) drawn `length` points long: the smallest of
/// JFreeChart's standard units whose labels do not overlap.
fn date_scale(
    cv: &mut Canvas,
    lo: f64,
    hi: f64,
    length: f32,
    vertical: bool,
    fmt: Option<&str>,
    locale: &Locale,
    font: &FontSpec,
) -> Scale {
    let lh = cv.line_height(font);
    let range = (hi - lo).max(1.0);
    let mut chosen = DATE_UNITS[DATE_UNITS.len() - 1];
    for &(field, count, pattern) in DATE_UNITS {
        let pattern = fmt.unwrap_or(pattern);
        let need = if vertical {
            lh + 4.0
        } else {
            cv.width(font, &date_label(lo, pattern, locale)).max(cv.width(font, &date_label(hi, pattern, locale))) + 2.0
        };
        if (field.millis() * count as f64 / range) as f32 * length >= need {
            chosen = (field, count, pattern);
            break;
        }
    }
    let (field, count, pattern) = chosen;
    let pattern = fmt.unwrap_or(pattern);
    let ticks = date_ticks(lo, hi, field, count).into_iter().map(|t| (t, date_label(t, pattern, locale))).collect();
    Scale { lower: lo, upper: hi, ticks }
}

/// A date range with JFreeChart's DateAxis margins (5% each side, at least 2 ms wide).
fn date_range(min: f64, max: f64) -> (f64, f64) {
    if !min.is_finite() || !max.is_finite() {
        // JFreeChart's empty DateAxis: the first millisecond of 1970.
        return (0.0, 1.0);
    }
    if max - min < 2.0 {
        let mid = (min + max) / 2.0;
        return (mid - 1.0, mid + 1.0);
    }
    let m = (max - min) * 0.05;
    (min - m, max + m)
}

/// Draws the value axis on the left (or right) of a data area, with grid lines when `grid`.
#[allow(clippy::too_many_arguments)]
fn vertical_axis(
    cv: &mut Canvas,
    scale: &Scale,
    font: &FontSpec,
    color: Color,
    x: f32,
    dy: f32,
    dh: f32,
    right: bool,
    grid: Option<(f32, f32)>,
) {
    let lh = cv.line_height(font);
    for (t, label) in &scale.ticks {
        let p = scale.at(*t, dy, dh, true);
        if let Some((gx, gw)) = grid {
            cv.line(gx, p, gx + gw, p, GRID, 0.5, Some([2.0, 2.0]));
        }
        if right {
            cv.line(x, p, x + 2.0, p, color, 0.5, None);
            cv.text(font, color, label, x + 6.0, p - lh / 2.0, Anchor::Left);
        } else {
            cv.line(x - 2.0, p, x, p, color, 0.5, None);
            cv.text(font, color, label, x - 6.0, p - lh / 2.0, Anchor::Right);
        }
    }
    cv.line(x, dy, x, dy + dh, color, 0.5, None);
}

fn widest_label(cv: &mut Canvas, scale: &Scale, font: &FontSpec) -> f32 {
    scale.ticks.iter().map(|(_, l)| cv.width(font, l)).fold(0.0f32, f32::max)
}

/// `color` seen through `alpha` over `under`.
fn blend(color: Color, alpha: f32, under: Color) -> Color {
    let mix = |a: u8, b: u8| (a as f32 * alpha + b as f32 * (1.0 - alpha)).round() as u8;
    Color { r: mix(color.r, under.r), g: mix(color.g, under.g), b: mix(color.b, under.b) }
}

// ---------------------------------------------------------------------------------------------
// XY charts over intervals, dates, bubbles and prices.

#[allow(clippy::too_many_arguments)]
fn xy_plot(
    cv: &mut Canvas,
    ch: &ChartElement,
    data: &DataState,
    texts: &ChartTexts,
    locale: &Locale,
    px: f32,
    py: f32,
    pw: f32,
    ph: f32,
    color_of: &dyn Fn(usize) -> Color,
) {
    let font = ch.font.clone();
    let lh = cv.line_height(&font);
    let kind = ch.dataset.kind;
    let dates = matches!(kind, DataKind::TimeSeries | DataKind::TimePeriod | DataKind::HighLow);
    let bars = ch.kind == ChartKind::XyBar;

    // The items as (series, x from, x to, y from, y to) in data units.
    let mut x_lo = f64::INFINITY;
    let mut x_hi = f64::NEG_INFINITY;
    let mut y_lo = f64::INFINITY;
    let mut y_hi = f64::NEG_INFINITY;
    let mut see = |x0: f64, x1: f64, y0: f64, y1: f64| {
        if x0.is_finite() && x1.is_finite() {
            x_lo = x_lo.min(x0);
            x_hi = x_hi.max(x1);
        }
        if y0.is_finite() && y1.is_finite() {
            y_lo = y_lo.min(y0);
            y_hi = y_hi.max(y1);
        }
    };
    // Bars over plain XY data are one unit of the smallest step between x values wide.
    let step = {
        let mut xs: Vec<f64> = data.xy.iter().flat_map(|(_, p)| p.iter().map(|q| q.0)).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        xs.dedup();
        xs.windows(2).map(|w| w[1] - w[0]).fold(f64::INFINITY, f64::min)
    };
    let step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
    for (_, pts) in &data.xy {
        for &(x, y) in pts {
            if bars {
                see(x - step / 2.0, x + step / 2.0, y, y);
            } else {
                see(x, x, y, y);
            }
        }
    }
    for (_, pts) in &data.periods {
        for &(a, b, v) in pts {
            if bars {
                see(a, b, v, v);
            } else {
                see(a, a, v, v);
            }
        }
    }
    for (_, pts) in &data.xyz {
        for &(x, y, z) in pts {
            see(x, x, y - z / 2.0, y + z / 2.0);
        }
    }
    for (_, items) in &data.ohlc {
        for it in items {
            see(it[0], it[0], it[2].min(it[3]).min(it[4]), it[1].max(it[3]).max(it[4]));
        }
    }
    let include_zero = bars || (ch.kind == ChartKind::Candlestick && ch.show_volume) || ch.kind == ChartKind::Bubble;
    let (vlo, vhi) = auto_range(y_lo, y_hi, include_zero, (texts.value_min, texts.value_max));
    let (xlo, xhi) = if dates { date_range(x_lo, x_hi) } else { auto_range(x_lo, x_hi, false, (None, None)) };

    let value_label = texts.value_label.clone().filter(|s| !s.is_empty());
    let x_label = texts.category_label.clone().filter(|s| !s.is_empty());
    let label_h = |present: bool| if present { lh + 6.0 } else { 0.0 };
    let bottom_h = 4.0 + (lh + 4.0) + label_h(x_label.is_some());
    let dh = (ph - bottom_h).max(1.0);
    let yscale = number_scale(cv, vlo, vhi, dh, true, ch.value_format.as_deref(), locale, &font);
    let left_w = label_h(value_label.is_some()) + 4.0 + widest_label(cv, &yscale, &font) + 4.0 + 2.0;
    let (dx, dy, dw) = (px + left_w, py, (pw - left_w).max(1.0));
    let xscale = if dates {
        date_scale(cv, xlo, xhi, dw, false, ch.time_format.as_deref(), locale, &font)
    } else {
        number_scale(cv, xlo, xhi, dw, false, ch.x_format.as_deref(), locale, &font)
    };
    if let Some(bg) = ch.plot_backcolor {
        cv.rect(dx, dy, dw, dh, Some(bg), None);
    }
    let xpos = |v: f64| xscale.at(v, dx, dw, false);
    let vpos = |v: f64| yscale.at(v, dy, dh, true);
    vertical_axis(cv, &yscale, &font, Color::BLACK, dx, dy, dh, false, Some((dx, dw)));
    for (t, label) in &xscale.ticks {
        let p = xpos(*t);
        cv.line(p, dy, p, dy + dh, GRID, 0.5, Some([2.0, 2.0]));
        cv.line(p, dy + dh, p, dy + dh + 2.0, Color::BLACK, 0.5, None);
        cv.text(&font, Color::BLACK, label, p, dy + dh + 6.0, Anchor::Center);
    }
    cv.line(dx, dy + dh, dx + dw, dy + dh, Color::BLACK, 0.5, None);
    if let Some(l) = &value_label {
        rotated_label(cv, &font, l, px + 3.0, dy + dh / 2.0);
    }
    if let Some(l) = &x_label {
        cv.text(&font, Color::BLACK, l, dx + dw / 2.0, dy + dh + 4.0 + lh + 4.0 + 3.0, Anchor::Center);
    }
    let base = vpos(0.0f64.max(yscale.lower).min(yscale.upper));

    // JFreeChart's XY plots draw their series last to first.
    match ch.kind {
        ChartKind::XyBar => {
            let series: Vec<Vec<(f64, f64, f64)>> = if kind == DataKind::Xy {
                data.xy.iter().map(|(_, p)| p.iter().map(|&(x, y)| (x - step / 2.0, x + step / 2.0, y)).collect()).collect()
            } else {
                data.periods.iter().map(|(_, p)| p.clone()).collect()
            };
            for (s, items) in series.iter().enumerate().rev() {
                for &(a, b, v) in items {
                    let (x0, x1) = (xpos(a), xpos(b));
                    let (y0, y1) = (vpos(v).min(base), vpos(v).max(base));
                    cv.rect(x0, y0, (x1 - x0).max(0.5), y1 - y0, Some(color_of(s)), None);
                }
            }
        }
        ChartKind::Bubble => {
            for (s, (_, pts)) in data.xyz.iter().enumerate().rev() {
                for &(x, y, z) in pts {
                    // The bubble's size is measured on the range axis.
                    let r = (z / (yscale.upper - yscale.lower)) as f32 * dh / 2.0;
                    let mut ops = Vec::new();
                    arc(&mut ops, xpos(x) as f64, vpos(y) as f64, r as f64, r as f64, 0.0, 2.0 * PI, true);
                    ops.push(PathOp::Close);
                    cv.path(ops, Some(color_of(s)), Some((OUTLINE, 0.5)), None);
                }
            }
        }
        ChartKind::Candlestick | ChartKind::HighLow => {
            let candle = ch.kind == ChartKind::Candlestick;
            // Candles are 4.5/7 of the narrowest gap between two dates wide.
            let mut xs: Vec<f32> = data.ohlc.iter().flat_map(|(_, i)| i.iter().map(|it| xpos(it[0]))).collect();
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let gap = xs.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0).fold(dw, f32::min);
            let width = gap * 4.5 / 7.0;
            let max_volume =
                data.ohlc.iter().flat_map(|(_, i)| i.iter().map(|it| it[5])).filter(|v| v.is_finite()).fold(0.0, f64::max);
            for (s, (_, items)) in data.ohlc.iter().enumerate().rev() {
                let color = color_of(s);
                for it in items {
                    let [t, high, low, open, close, volume] = *it;
                    let x = xpos(t);
                    if candle && ch.show_volume && max_volume > 0.0 && volume.is_finite() {
                        let h = (volume / max_volume) as f32 * dh;
                        cv.rect(x - width / 2.0, dy + dh - h, width, h, Some(Color { r: 0x80, g: 0x80, b: 0x80 }), None);
                    }
                    if high.is_finite() && low.is_finite() {
                        cv.line(x, vpos(high), x, vpos(low), color, 1.0, None);
                    }
                    if candle {
                        if open.is_finite() && close.is_finite() {
                            let (top, bottom) = (vpos(open.max(close)), vpos(open.min(close)));
                            let fill = if close > open { Color { r: 0, g: 0xFF, b: 0 } } else { Color { r: 0xFF, g: 0, b: 0 } };
                            cv.rect(x - width / 2.0, top, width, (bottom - top).max(0.5), Some(fill), Some(pen(color, 1.0)));
                        }
                    } else {
                        if open.is_finite() {
                            cv.line(x - 2.0, vpos(open), x, vpos(open), color, 1.0, None);
                        }
                        if close.is_finite() {
                            cv.line(x, vpos(close), x + 2.0, vpos(close), color, 1.0, None);
                        }
                    }
                }
            }
        }
        _ => {
            // Time series (and plain XY data drawn the same way).
            let series: Vec<Vec<(f64, f64)>> = if kind == DataKind::Xy {
                data.xy.iter().map(|(_, p)| p.clone()).collect()
            } else {
                data.periods.iter().map(|(_, p)| p.iter().map(|&(a, _, v)| (a, v)).collect()).collect()
            };
            for (s, pts) in series.iter().enumerate().rev() {
                let mut pts = pts.clone();
                pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                let screen: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (xpos(x), vpos(y))).collect();
                if ch.show_lines && screen.len() > 1 {
                    let mut ops = vec![PathOp::Move(screen[0].0 as f64, screen[0].1 as f64)];
                    ops.extend(screen[1..].iter().map(|p| PathOp::Line(p.0 as f64, p.1 as f64)));
                    cv.path(ops, None, Some((color_of(s), 1.0)), None);
                }
                if ch.show_shapes {
                    for p in &screen {
                        shape(cv, s, p.0, p.1, color_of(s));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Gantt charts: tasks down the left, dates across the top.

// ---------------------------------------------------------------------------------------------
// Multiple axis charts: category charts drawn together, each with its own value axis.

#[allow(clippy::too_many_arguments)]
fn multi_axis(
    cv: &mut Canvas,
    ch: &ChartElement,
    all: &HashMap<usize, DataState>,
    texts: &ChartTexts,
    locale: &Locale,
    px: f32,
    py: f32,
    pw: f32,
    ph: f32,
) {
    let empty = DataState::default();
    let font = ch.font.clone();
    let lh = cv.line_height(&font);
    // The categories of every chart, in the order they first appear.
    let mut cats: Vec<String> = Vec::new();
    for (_, a) in &ch.axes {
        for c in &all.get(&a.id).unwrap_or(&empty).categories {
            if !cats.contains(c) {
                cats.push(c.clone());
            }
        }
    }
    let bottom_h = 4.0 + (lh + 4.0);
    let dh = (ph - bottom_h).max(1.0);
    // Each chart's value range and axis.
    let mut scales = Vec::new();
    for (i, (_, a)) in ch.axes.iter().enumerate() {
        let d = all.get(&a.id).unwrap_or(&empty);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for v in d.values.values() {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        let t = texts.axes.get(i);
        let (lo, hi) = auto_range(lo, hi, true, (t.and_then(|t| t.value_min), t.and_then(|t| t.value_max)));
        scales.push(number_scale(cv, lo, hi, dh, true, a.value_format.as_deref(), locale, &font));
    }
    let widths: Vec<f32> = ch
        .axes
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let label = texts.axes.get(i).and_then(|t| t.value_label.as_deref()).is_some_and(|l| !l.is_empty());
            widest_label(cv, &scales[i], &font) + 10.0 + if label { lh + 6.0 } else { 0.0 }
        })
        .collect();
    let left: f32 = ch.axes.iter().zip(&widths).filter(|((r, _), _)| !*r).map(|(_, w)| w).sum();
    let right: f32 = ch.axes.iter().zip(&widths).filter(|((r, _), _)| *r).map(|(_, w)| w).sum();
    let (dx, dy, dw) = (px + left, py, (pw - left - right).max(1.0));
    if let Some(bg) = ch.plot_backcolor {
        cv.rect(dx, dy, dw, dh, Some(bg), None);
    }
    let (mut at_left, mut at_right) = (dx, dx + dw);
    let mut palette_base = 0;
    let mut bases = Vec::new();
    for (i, (right_side, a)) in ch.axes.iter().enumerate() {
        let color = a.axis_color.unwrap_or(Color::BLACK);
        let x = if *right_side { at_right } else { at_left };
        vertical_axis(cv, &scales[i], &font, color, x, dy, dh, *right_side, if i == 0 { Some((dx, dw)) } else { None });
        if let Some(l) = texts.axes.get(i).and_then(|t| t.value_label.as_deref()).filter(|l| !l.is_empty()) {
            let lx = if *right_side { x + widths[i] - lh - 3.0 } else { x - widths[i] + 3.0 };
            rotated_label(cv, &font, l, lx, dy + dh / 2.0);
        }
        if *right_side {
            at_right += widths[i];
        } else {
            at_left -= widths[i];
        }
        bases.push(palette_base);
        palette_base += all.get(&a.id).map(|d| d.series.len()).unwrap_or(0);
    }
    cv.line(dx, dy + dh, dx + dw, dy + dh, Color::BLACK, 0.5, None);
    let ncat = cats.len();
    let cat_margin = if ncat > 1 { 0.2 } else { 0.0 };
    let cat_w = dw * (1.0 - 0.1 - cat_margin) / ncat.max(1) as f32;
    let cat_gap = if ncat > 1 { dw * cat_margin / (ncat - 1) as f32 } else { 0.0 };
    let cat_start = |c: usize| dx + 0.05 * dw + c as f32 * (cat_w + cat_gap);
    for (c, name) in cats.iter().enumerate() {
        cv.text(&font, Color::BLACK, name, cat_start(c) + cat_w / 2.0, dy + dh + 6.0, Anchor::Center);
    }
    // The last chart is drawn first.
    for (i, (_, a)) in ch.axes.iter().enumerate().rev() {
        let d = all.get(&a.id).unwrap_or(&empty);
        let scale = &scales[i];
        let vpos = |v: f64| scale.at(v, dy, dh, true);
        let color = |s: usize| a.series_colors.get(s).copied().unwrap_or_else(|| rgb(PALETTE[(bases[i] + s) % PALETTE.len()]));
        let col = |name: &String| cats.iter().position(|c| c == name);
        let nser = d.series.len();
        match a.kind {
            ChartKind::Line | ChartKind::Area | ChartKind::StackedArea | ChartKind::XyLine => {
                for s in 0..nser {
                    let pts: Vec<(f32, f32)> = d
                        .categories
                        .iter()
                        .enumerate()
                        .filter_map(|(k, name)| Some((cat_start(col(name)?) + cat_w / 2.0, vpos(*d.values.get(&(s, k))?))))
                        .collect();
                    if a.show_lines && pts.len() > 1 {
                        let mut ops = vec![PathOp::Move(pts[0].0 as f64, pts[0].1 as f64)];
                        ops.extend(pts[1..].iter().map(|p| PathOp::Line(p.0 as f64, p.1 as f64)));
                        cv.path(ops, None, Some((color(s), 1.0)), None);
                    }
                    if a.show_shapes {
                        for p in &pts {
                            shape(cv, s, p.0, p.1, color(s));
                        }
                    }
                }
            }
            _ => {
                let base = vpos(0.0f64.max(scale.lower).min(scale.upper));
                let item_margin = if nser > 1 { 0.2 } else { 0.0 };
                let bar_w = dw * (1.0 - 0.1 - cat_margin - item_margin) / (nser * ncat).max(1) as f32;
                let item_gap = if nser > 1 { dw * item_margin / (ncat * (nser - 1)).max(1) as f32 } else { 0.0 };
                for (k, name) in d.categories.iter().enumerate() {
                    let Some(c) = col(name) else { continue };
                    for s in 0..nser {
                        let Some(v) = d.values.get(&(s, k)) else { continue };
                        bar(cv, false, cat_start(c) + s as f32 * (bar_w + item_gap), bar_w, base, vpos(*v), color(s), false);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_periods_and_date_ticks() {
        let t = chrono::NaiveDate::from_ymd_opt(2017, 5, 17).unwrap().and_hms_opt(13, 45, 0).unwrap();
        let (a, b) = TimePeriod::Month.bounds(t);
        assert_eq!(to_date(a), chrono::NaiveDate::from_ymd_opt(2017, 5, 1).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!(b + 1.0, millis(chrono::NaiveDate::from_ymd_opt(2017, 6, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()));
        // Four-month ticks start in January, May and September, as JFreeChart's do.
        let lo = millis(chrono::NaiveDate::from_ymd_opt(2016, 11, 3).unwrap().and_hms_opt(0, 0, 0).unwrap());
        let hi = millis(chrono::NaiveDate::from_ymd_opt(2018, 2, 1).unwrap().and_hms_opt(0, 0, 0).unwrap());
        let months: Vec<u32> =
            date_ticks(lo, hi, DateField::Month, 4).into_iter().map(|t| chrono::Datelike::month(&to_date(t))).collect();
        assert_eq!(months, vec![1, 5, 9, 1]);
    }

    #[test]
    fn tick_units_follow_jfreechart() {
        // 90,000 over 143 points with 14.5-point labels: steps of 10,000.
        assert_eq!(choose_unit(0.0, 90_334.0, 143.0, |_| 14.48), 10_000.0);
        assert_eq!(choose_unit(0.0, 14.65, 200.0, |_| 30.0), 2.5);
        let en = crate::format::locale::EN_US.clone();
        assert_eq!(tick_label(5.0, 2.5, None, &en), "5.0");
        assert_eq!(tick_label(30_000.0, 5_000.0, None, &en), "30,000");
    }

    #[test]
    fn datasets_accumulate() {
        let mut d = DataState::default();
        d.add_pie("a".into(), 1.0);
        d.add_pie("a".into(), 2.0);
        assert_eq!(d.pie, vec![("a".to_string(), 3.0)]);
        d.add_category("s".into(), "c".into(), 1.0);
        d.add_category("s".into(), "c".into(), 5.0);
        assert_eq!(d.values[&(0, 0)], 5.0);
        let (lo, hi) = auto_range(0.0, 100.0, true, (None, None));
        assert_eq!((lo, hi), (0.0, 105.0));
    }
}

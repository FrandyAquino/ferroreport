//! Thermometer charts: JFreeChart 1.5's `ThermometerPlot.draw` with the plot JasperReports 7
//! sets up (`DefaultChartTheme.createThermometerChart`), measured against JasperReports'
//! thermometer sample.
//!
//! The thermometer has a fixed size, whatever the chart's: a bulb 80 across and a stem 40
//! wide, with a second outline 5 inside the first, centered in the plot. The plot takes
//! JFreeChart's insets (4 above and below, 8 at the sides), then 5% of its width and height
//! on every side. The stem runs from the top of that area; the bulb sits at its bottom. The
//! values are on a stretch of the stem from 40 below its top to the top of the bulb, and the
//! axis labels them on the left of the stem with JasperReports' integer tick units
//! (SansSerif 10, tick marks but no axis line). Every position JFreeChart casts to `int` is
//! cut to a whole point of the chart.

use super::jfree::{NumberAxis, Rect, Side};
use super::{fmt_number, settings, Canvas, ChartElement, ChartTexts, DataState, ValueLocation};
use crate::format::Locale;
use crate::model::{Color, FontSpec};
use crate::print::PathOp;
use std::f64::consts::PI;

const BULB_RADIUS: f64 = 40.0;
const COLUMN_RADIUS: f64 = 20.0;
/// Between the outer and the inner outline, and around the value.
const GAP: f64 = 5.0;
/// The width of the area the axis is drawn for.
const AXIS_GAP: f64 = 10.0;

/// JFreeChart's subranges (normal, warning, critical) where the template leaves one out.
const DEFAULT_SUBRANGES: [(f64, f64); 3] = [(0.0, 50.0), (50.0, 75.0), (75.0, 100.0)];
/// Their colors: green, orange and red.
const SUBRANGE_COLORS: [Color; 3] =
    [Color { r: 0, g: 0xFF, b: 0 }, Color { r: 0xFF, g: 0xC8, b: 0 }, Color { r: 0xFF, g: 0, b: 0 }];
const GRAY: Color = Color { r: 0x80, g: 0x80, b: 0x80 };

/// Draws the plot in `area`, what is left of the chart after its titles.
pub(crate) fn draw(cv: &mut Canvas, ch: &ChartElement, data: &DataState, texts: &ChartTexts, locale: &Locale, area: Rect) {
    let Some(dial) = ch.dial.as_deref() else { return };
    let (lower, upper) = texts.dial.unwrap_or((DEFAULT_SUBRANGES[0].0, DEFAULT_SUBRANGES[2].1));
    if lower.is_nan() || upper.is_nan() || upper <= lower {
        return;
    }
    let area = area.trim(4.0, 8.0, 4.0, 8.0);
    if let Some(bg) = ch.plot_backcolor {
        cv.fill_rect(area, bg);
    }
    let interior = area.trim(area.h * 0.05, area.w * 0.05, area.h * 0.05, area.w * 0.05);
    // `(int)` of a position of the chart, and `Math.round`.
    let (ox, oy) = cv.origin;
    let int_x = |v: f64| (v - ox).trunc() + ox;
    let int_y = |v: f64| (v - oy).trunc() + oy;
    let round_y = |v: f64| (v - oy + 0.5).floor() + oy;
    let mid_x = int_x(interior.center_x());
    let mid_y = int_y(interior.center_y());
    let stem_top = int_y(interior.y + BULB_RADIUS);
    let stem_bottom = int_y(interior.max_y() - 2.0 * BULB_RADIUS);
    let bulb_y = stem_bottom + BULB_RADIUS;
    // Values are placed on this area (the mercury, the range marks); the axis's ticks on one
    // a point taller.
    let data_area = Rect::new(mid_x - COLUMN_RADIUS, stem_top, COLUMN_RADIUS, stem_bottom - stem_top);

    let outer = stem_and_bulb(
        Stem {
            x: mid_x - COLUMN_RADIUS,
            y: interior.y,
            w: 2.0 * COLUMN_RADIUS,
            h: stem_bottom + 2.0 * BULB_RADIUS - stem_top,
            arc: 2.0 * COLUMN_RADIUS,
        },
        mid_x,
        bulb_y,
        BULB_RADIUS,
    );
    let inner_w = 2.0 * COLUMN_RADIUS - 2.0 * GAP;
    let inner = stem_and_bulb(
        Stem {
            x: mid_x - COLUMN_RADIUS + GAP,
            y: interior.y + GAP,
            w: inner_w,
            h: stem_bottom + 2.0 * BULB_RADIUS - 2.0 * GAP - stem_top,
            arc: inner_w,
        },
        mid_x,
        bulb_y,
        BULB_RADIUS - GAP,
    );

    if let Some(current) = data.value {
        let font = FontSpec {
            name: "SansSerif".into(),
            size: 10.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            pdf_font: None,
        };
        let mut axis = NumberAxis::new(lower, upper, font).integer_units();
        axis.line_visible = false;
        axis.line_color = GRAY;
        // The mercury: the inner bulb, and the inner stem up to the value with corners half
        // as round. Above the top of the inner stem it stops there, fully rounded.
        let mut level = round_y(axis.java2d(current, data_area, Side::Left));
        let mut arc = inner_w / 2.0;
        if level < GAP + interior.y {
            level = int_y(GAP + interior.y);
            arc = BULB_RADIUS;
        }
        let mercury = if level < bulb_y {
            let column = Stem { x: mid_x - inner_w / 2.0, y: level, w: inner_w, h: bulb_y - level, arc };
            stem_and_bulb(column, mid_x, bulb_y, BULB_RADIUS - GAP)
        } else {
            circle(mid_x, bulb_y, BULB_RADIUS - GAP)
        };
        // JasperReports' low, medium and high ranges are JFreeChart's critical, warning and
        // normal subranges.
        let subranges: Vec<((f64, f64), Color)> = (0..3)
            .map(|n| {
                let i = 2 - n;
                let range = texts.intervals.get(i).copied().flatten().unwrap_or(DEFAULT_SUBRANGES[n]);
                let color = dial.intervals.get(i).and_then(|r| r.color).unwrap_or(SUBRANGE_COLORS[n]);
                (range, color)
            })
            .collect();
        // Without a mercuryColor: the color of the first subrange that has the value (above
        // its low end, up to its high end).
        let paint = if settings(ch).mercury_by_range {
            subranges.iter().find(|((lo, hi), _)| current > *lo && current <= *hi).map(|(_, c)| *c).unwrap_or(dial.mercury)
        } else {
            dial.mercury
        };
        cv.path(mercury, Some(paint), None, None);
        // A mark 10 long, 2 wide, 2 right of the stem, where each subrange starts (when it is
        // on the axis).
        for ((lo, _), color) in &subranges {
            if (lower..=upper).contains(lo) {
                let x = mid_x + COLUMN_RADIUS + 2.0;
                let y = axis.java2d(*lo, data_area, Side::Left);
                cv.line_square(x, y, x + 10.0, y, *color, 2.0);
            }
        }
        let cursor = mid_x - COLUMN_RADIUS;
        let draw_area = Rect::new(cursor, stem_top, AXIS_GAP, stem_bottom - stem_top + 1.0);
        axis.draw(cv, cursor, area, draw_area, Side::Left, locale);
        // The value: centered in the bulb, its baseline 5 below the bulb's center; or beside
        // the stem, on the plot's middle line.
        let text = fmt_number(current, dial.value_mask.as_deref(), locale);
        let font = &dial.value_font;
        let width = cv.awt_width(font, &text);
        let at = match dial.value_location {
            ValueLocation::Bulb => Some((mid_x - (width / 2.0).floor(), bulb_y + GAP)),
            ValueLocation::Right => Some((mid_x + COLUMN_RADIUS + GAP, mid_y)),
            ValueLocation::Left => Some((mid_x - COLUMN_RADIUS - GAP - width, mid_y)),
            ValueLocation::None => None,
        };
        if let Some((x, baseline)) = at {
            cv.draw_string(font, dial.value_color, &text, x, baseline);
        }
    }
    cv.stroke_closed(outer, Color::BLACK, 1.0);
    cv.stroke_closed(inner, Color::BLACK, 1.0);
}

/// A `RoundRectangle2D`: corners `arc` across, but no wider and no taller than the rectangle.
#[derive(Debug, Clone, Copy)]
struct Stem {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    arc: f64,
}

fn circle(cx: f64, cy: f64, r: f64) -> Vec<PathOp> {
    let mut ops = Vec::new();
    super::arc(&mut ops, cx, cy, r, r, 0.0, 2.0 * PI, true);
    ops.push(PathOp::Close);
    ops
}

/// The outline of a stem standing in a bulb (`Area.add`), both centered on `cx`: the stem
/// from where its sides leave the circle, over its top, and the circle round the bottom. The
/// stem's bottom is inside the bulb.
fn stem_and_bulb(stem: Stem, cx: f64, cy: f64, r: f64) -> Vec<PathOp> {
    if stem.w < 0.0 || stem.h < 0.0 {
        return circle(cx, cy, r);
    }
    let (rx, ry) = (stem.arc.min(stem.w) / 2.0, stem.arc.min(stem.h) / 2.0);
    let half = stem.w / 2.0;
    // The top right corner's center.
    let (kx, ky) = (cx + half - rx, stem.y + ry);
    // Half the width of the stem and of the circle at `y`.
    let stem_half = |y: f64| {
        if y < ky && ry > 0.0 {
            let t = ((ky - y) / ry).min(1.0);
            half - rx + rx * (1.0 - t * t).max(0.0).sqrt()
        } else {
            half
        }
    };
    let circle_half = |y: f64| {
        let d = r * r - (y - cy) * (y - cy);
        if d >= 0.0 {
            d.sqrt()
        } else {
            f64::NEG_INFINITY
        }
    };
    let outside = |y: f64| stem_half(y) > circle_half(y);
    let end = (stem.y + stem.h - ry).max(stem.y).min(cy);
    if !outside(stem.y) {
        return circle(cx, cy, r);
    }
    if outside(end) {
        // The stem is wider than the bulb: both outlines.
        let mut ops = rounded(stem, rx, ry);
        ops.extend(circle(cx, cy, r));
        return ops;
    }
    // Where the stem's sides go into the circle.
    let (mut a, mut b) = (stem.y, end);
    for _ in 0..60 {
        let m = (a + b) / 2.0;
        if outside(m) {
            a = m;
        } else {
            b = m;
        }
    }
    let y0 = (a + b) / 2.0;
    let s = circle_half(y0).max(0.0);
    let mut ops = vec![PathOp::Move(cx + s, y0)];
    // The corner's angle where the side meets the circle: 0 below the corner.
    let t0 = if y0 < ky && ry > 0.0 { ((y0 - ky) / ry).atan2((cx + s - kx) / rx.max(f64::MIN_POSITIVE)) } else { 0.0 };
    if y0 >= ky {
        ops.push(PathOp::Line(cx + half, ky));
    }
    super::arc(&mut ops, kx, ky, rx, ry, t0, -PI / 2.0, false);
    if rx < half {
        ops.push(PathOp::Line(cx - half + rx, stem.y));
    }
    super::arc(&mut ops, cx - half + rx, ky, rx, ry, -PI / 2.0, -PI - t0, false);
    if y0 >= ky {
        ops.push(PathOp::Line(cx - half, y0));
    }
    // Round the bottom of the bulb, from the left back to the right.
    let beta = (cy - y0).atan2(s);
    super::arc(&mut ops, cx, cy, r, r, PI + beta, -beta, false);
    ops.push(PathOp::Close);
    ops
}

/// The outline of a whole round rectangle.
fn rounded(s: Stem, rx: f64, ry: f64) -> Vec<PathOp> {
    let (x0, y0, x1, y1) = (s.x, s.y, s.x + s.w, s.y + s.h);
    let mut ops = vec![PathOp::Move(x0 + rx, y0), PathOp::Line(x1 - rx, y0)];
    super::arc(&mut ops, x1 - rx, y0 + ry, rx, ry, -PI / 2.0, 0.0, false);
    ops.push(PathOp::Line(x1, y1 - ry));
    super::arc(&mut ops, x1 - rx, y1 - ry, rx, ry, 0.0, PI / 2.0, false);
    ops.push(PathOp::Line(x0 + rx, y1));
    super::arc(&mut ops, x0 + rx, y1 - ry, rx, ry, PI / 2.0, PI, false);
    ops.push(PathOp::Line(x0, y0 + ry));
    super::arc(&mut ops, x0 + rx, y0 + ry, rx, ry, PI, 1.5 * PI, false);
    ops.push(PathOp::Close);
    ops
}

#[cfg(test)]
mod tests {
    use super::super::{render, DataState};
    use super::*;
    use crate::print::PrintItem;
    use std::collections::HashMap;

    /// The first thermometer of JasperReports' ThermometerChartReport sample (JRXML 7, whose
    /// ranges have no `<dataRange>`), as JasperReports 7.1.1 draws it on page 11.
    const TEMPLATE: &str = r##"<jasperReport name="T" pageWidth="595" pageHeight="842" columnWidth="555" leftMargin="20" rightMargin="20" topMargin="30" bottomMargin="30">
	<title height="175">
		<element kind="chart" chartType="thermometer" x="0" y="0" width="270" height="175">
			<dataset kind="value"><valueExpression><![CDATA[819.27d]]></valueExpression></dataset>
			<plot mercuryColor="#0000CC" valueLocation="bulb">
				<dataRange><lowExpression><![CDATA[0.0d]]></lowExpression><highExpression><![CDATA[17500.0d]]></highExpression></dataRange>
				<valueDisplay color="#000000" mask="$#,###"><font fontSize="12.0"/></valueDisplay>
				<lowRange><lowExpression><![CDATA[0.0d]]></lowExpression><highExpression><![CDATA[2000.0d]]></highExpression></lowRange>
				<mediumRange><lowExpression><![CDATA[2000.0d]]></lowExpression><highExpression><![CDATA[12000.0d]]></highExpression></mediumRange>
				<highRange><lowExpression><![CDATA[12000.0d]]></lowExpression><highExpression><![CDATA[17500d]]></highExpression></highRange>
			</plot>
		</element>
	</title>
</jasperReport>"##;

    fn paths(items: &[PrintItem], out: &mut Vec<(Vec<PathOp>, Option<Color>, Option<(Color, f32)>)>) {
        for item in items {
            match item {
                PrintItem::Path { ops, fill, stroke, .. } => out.push((ops.to_vec(), *fill, *stroke)),
                PrintItem::Transform(_, inner) | PrintItem::Group(inner) => paths(inner, out),
                _ => {}
            }
        }
    }

    fn ys(ops: &[PathOp]) -> Vec<f64> {
        ops.iter()
            .filter_map(|op| match *op {
                PathOp::Move(_, y) | PathOp::Line(_, y) | PathOp::Cubic(_, _, _, _, _, y) => Some(y),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn thermometer_follows_jfreechart() {
        let report = crate::parse(TEMPLATE).unwrap();
        let ch = report.title.as_ref().unwrap().elements.iter().find_map(|e| match &e.kind {
            crate::model::ElementKind::Chart(c) => Some(c.as_ref().clone()),
            _ => None,
        });
        let ch = ch.expect("a chart");
        let dial = ch.dial.as_deref().unwrap();
        // Low, medium and high ranges, each with both ends.
        assert_eq!(dial.intervals.len(), 3);
        assert!(dial.intervals.iter().all(|i| i.low.is_some() && i.high.is_some()));
        let texts = ChartTexts {
            dial: Some((0.0, 17500.0)),
            intervals: vec![Some((0.0, 2000.0)), Some((2000.0, 12000.0)), Some((12000.0, 17500.0))],
            ..Default::default()
        };
        let data = DataState { value: Some(819.27), ..Default::default() };
        let all = HashMap::from([(ch.id, data)]);
        let locale = crate::format::locale::EN_US.clone();
        let mut fonts = crate::fonts::FontSet::new();
        let items = render(&ch, &all, &texts, &mut fonts, &locale, 0.0, 0.0, 270.0, 175.0);
        let mut found = Vec::new();
        paths(&items, &mut found);
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        // The mercury reaches y 81: the value at 80.6 on a 30-point stretch above y 82, rounded.
        let blue = Color { r: 0, g: 0, b: 0xCC };
        let mercury = found.iter().find(|p| p.1 == Some(blue)).expect("mercury");
        assert!(near(ys(&mercury.0).into_iter().fold(f64::MAX, f64::min), 81.0));
        // Where the high, medium and low ranges start, green, orange and red, right of the stem.
        let marks: Vec<(f64, Color)> =
            found.iter().filter(|p| p.2.is_some_and(|s| s.1 == 2.0)).map(|p| (ys(&p.0)[0], p.2.unwrap().0)).collect();
        assert_eq!(marks.len(), 3);
        for ((y, c), (want_y, want_c)) in marks.iter().zip([(61.43, 0x00FF00), (78.57, 0xFFC800), (82.0, 0xFF0000)]) {
            assert!((y - want_y).abs() < 0.01, "{y} {want_y}");
            assert_eq!(*c, Color { r: (want_c >> 16) as u8, g: (want_c >> 8) as u8, b: want_c as u8 });
        }
        // Gray tick marks at 0 and 10,000 (integer units) on a 31-point stretch above y 83.
        let gray = Color { r: 0x80, g: 0x80, b: 0x80 };
        let ticks: Vec<f64> = found.iter().filter(|p| p.2.is_some_and(|s| s.0 == gray)).map(|p| ys(&p.0)[0]).collect();
        assert_eq!(ticks.len(), 2);
        assert!(near(ticks[0], 83.0) && near(ticks[1], 83.0 - 31.0 * 10_000.0 / 17_500.0), "{ticks:?}");
        // Two black outlines, the outer one from the top of the padded plot.
        let outlines: Vec<_> = found.iter().filter(|p| p.2 == Some((Color::BLACK, 1.0)) && p.1.is_none()).collect();
        assert_eq!(outlines.len(), 2);
        assert!(near(ys(&outlines[0].0).into_iter().fold(f64::MAX, f64::min), 4.0 + 167.0 * 0.05));
        assert!(near(ys(&outlines[1].0).into_iter().fold(f64::MAX, f64::min), 4.0 + 167.0 * 0.05 + 5.0));
    }
}

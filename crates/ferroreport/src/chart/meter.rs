//! Meter charts: JFreeChart 1.5's `MeterPlot.draw` with the plot JasperReports 7 sets up
//! (`DefaultChartTheme.createMeterChart`), measured against JasperReports' meter sample.
//!
//! The dial is a circle as wide as fits the plot after its insets (4 above and below, 8 at
//! the sides), 4 more on every side and a gap of 3. A meter of 180 degrees or less that is not
//! a circle counts the plot 1.8 times as tall (the lower half of its circle is not drawn), so
//! it fills a wide plot. The dial's background reaches 4 past the circle, which leaves a rim
//! where the intervals, filled over the circle itself, do not reach. Everything is clipped
//! to the square around that background.
//!
//! In order: the background, a tick every `tickInterval` from the low end, the intervals,
//! each with a labeled tick at both ends, the value under the center, and the needle. Ticks
//! are 2 wide and run from 3 outside the circle to 12 inside it; an interval's translucent
//! color covers the ticks under it. Only the ends of intervals are labeled (JFreeChart's
//! bold SansSerif 10, black), on the inner end of their tick. The value is written with the
//! units (`Units` when the template has none) 10 below the center, and the needle, a
//! triangle from 2.5 either side of the center to 18 outside the circle with a dot 10
//! across, is drawn only for a value within the range. Where the template leaves a color out,
//! JFreeChart's is used (the parser sets them): a black dial, white ticks, a green needle and
//! a yellow value. Drawn as SVG, arcs go through Batik's SVG arcs, which move a half
//! circle's center (see [`svg_arc`]).

use super::jfree::{self, Rect, TOP_CENTER};
use super::{arc, blend, fmt_number, settings, Canvas, ChartElement, ChartTexts, DataState, MeterShape};
use crate::format::Locale;
use crate::model::Color;
use crate::print::{PathOp, PrintItem};
use std::sync::Arc;

/// Between the meter's area and its circle, and the ticks' reach past the circle.
const BORDER: f64 = 3.0;
/// The dot at the center of the needle.
const CIRCLE: f64 = 10.0;
/// More ticks than this are not drawn (JFreeChart would draw them one by one).
const MAX_TICKS: f64 = 10_000.0;

/// `MeterPlot`'s legend: one item per interval, an 8 by 8 square in its color.
pub(crate) fn legend_items(ch: &ChartElement) -> Vec<jfree::LegendItem> {
    let Some(dial) = ch.dial.as_deref() else { return Vec::new() };
    let back = ch.legend_back.or(ch.backcolor).unwrap_or(Color::WHITE);
    dial.intervals
        .iter()
        .map(|i| jfree::LegendItem {
            label: i.label.clone(),
            graphic: jfree::LegendGraphic::square(blend(interval_color(ch, i), alpha(i.alpha), back), None),
        })
        .collect()
}

/// An interval's color; JasperReports gives one without its chart's background color.
fn interval_color(ch: &ChartElement, i: &super::Interval) -> Color {
    i.color.or(ch.backcolor).unwrap_or(Color::WHITE)
}

/// An alpha as the color carries it: in 255ths.
fn alpha(a: f32) -> f32 {
    (a.clamp(0.0, 1.0) * 255.0 + 0.5).floor() / 255.0
}

/// Draws the plot in `area`, what is left of the chart after its titles and legend.
pub(crate) fn draw(cv: &mut Canvas, ch: &ChartElement, data: &DataState, texts: &ChartTexts, locale: &Locale, area: Rect) {
    let Some(dial) = ch.dial.as_deref() else { return };
    let (lower, upper) = texts.dial.unwrap_or((0.0, 100.0));
    if lower.is_nan() || upper.is_nan() || upper <= lower {
        return;
    }
    let shape = settings(ch).meter_shape;
    // MeterPlot.setMeterAngle takes whole degrees from 1 to 360.
    let angle = dial.angle.trunc().clamp(1.0, 360.0);
    let area = area.trim(4.0, 8.0, 4.0, 8.0).trim(4.0, 4.0, 4.0, 4.0);
    let w = area.w - 2.0 * BORDER;
    let h = area.h - 2.0 * BORDER + if angle <= 180.0 && shape != MeterShape::Circle { area.h / 1.25 } else { 0.0 };
    let r = w.min(h) / 2.0;
    if r.is_nan() || r <= 0.0 {
        return;
    }
    let (cx, cy) = (area.x + BORDER + w / 2.0, area.y + BORDER + h / 2.0);
    let meter = Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r);
    let outer = Rect::new(meter.x - 4.0, meter.y - 4.0, meter.w + 8.0, meter.h + 8.0);
    let geometry = Dial { cx, cy, r, lower, upper, angle, shape, svg: cv.svg, origin: cv.origin };
    let base = ch.backcolor.unwrap_or(Color::WHITE);
    let format = |v: f64| fmt_number(v, dial.value_mask.as_deref(), locale);

    cv.clipped(outer, |cv| {
        // What is drawn so far, after the clip, each with the interval whose wedge it is (an
        // interval's translucent color covers what is under it, which only overlapping
        // intervals can be).
        let mut drawn: Vec<Option<(f64, f64)>> = vec![None; cv.items.len()];
        let tag = |cv: &Canvas, drawn: &mut Vec<Option<(f64, f64)>>, wedge| drawn.resize(cv.items.len(), wedge);
        let start = drawn.len();

        cv.path(geometry.fill_arc(outer, lower, upper, true), Some(dial.background.unwrap_or(Color::BLACK)), None, None);
        let ticks = (upper - lower) / dial.tick_interval;
        if dial.tick_interval > 0.0 && ticks <= MAX_TICKS {
            let mut v = lower;
            while v <= upper {
                geometry.tick(cv, dial, v, None);
                v += dial.tick_interval;
            }
        }
        tag(cv, &mut drawn, None);

        for (i, interval) in dial.intervals.iter().enumerate() {
            let Some(Some((lo, hi))) = texts.intervals.get(i).copied() else { continue };
            let color = interval_color(ch, interval);
            let a = alpha(interval.alpha);
            let wedge = geometry.fill_arc(meter, lo, hi, false);
            if a >= 1.0 {
                cv.path(wedge, Some(color), None, None);
            } else {
                // The wedge over the chart's background, and what is already drawn there seen
                // through it.
                let mut over = vec![PrintItem::Path {
                    ops: Arc::new(wedge.clone()),
                    fill: Some(blend(color, a, base)),
                    stroke: None,
                    dash: None,
                }];
                for (item, under) in cv.items[start..].iter().zip(&drawn[start..]) {
                    if under.is_some_and(|(a0, b0)| lo.max(hi).min(a0.max(b0)) <= lo.min(hi).max(a0.min(b0))) {
                        continue;
                    }
                    over.push(tint(item, color, a));
                }
                cv.clipped_path(wedge, |cv| cv.items.extend(over));
            }
            tag(cv, &mut drawn, Some((lo, hi)));
            geometry.tick(cv, dial, lo, Some(&format(lo)));
            geometry.tick(cv, dial, hi, Some(&format(hi)));
            tag(cv, &mut drawn, None);
        }

        let Some(value) = data.value else { return };
        let units = if dial.units.is_empty() { "Units" } else { &dial.units };
        let text = format!("{} {units}", format(value));
        jfree::draw_aligned(
            cv,
            &dial.value_font,
            dial.value_color,
            &text,
            cx as f32 as f64,
            (cy as f32 + CIRCLE as f32) as f64,
            TOP_CENTER,
        );
        if value < lower || value > upper {
            return;
        }
        let degrees = geometry.angle_of(value);
        let reach = r + BORDER + 15.0;
        let tip = (cx + reach * degrees.to_radians().cos(), cy - reach * degrees.to_radians().sin());
        // `(int)` of a position of the chart.
        let (ox, oy) = cv.origin;
        let int = |x: f64, y: f64| ((x - ox).trunc() + ox, (y - oy).trunc() + oy);
        let base = if (degrees > 135.0 && degrees < 225.0) || (degrees < 45.0 && degrees > -45.0) {
            [int(cx, cy - CIRCLE / 4.0), int(cx, cy + CIRCLE / 4.0)]
        } else {
            [int(cx - CIRCLE / 4.0, cy), int(cx + CIRCLE / 4.0, cy)]
        };
        cv.fill_poly(&[base[0], base[1], int(tip.0, tip.1)], dial.needle);
        let mut dot = Vec::new();
        arc(&mut dot, cx, cy, CIRCLE / 2.0, CIRCLE / 2.0, 0.0, 2.0 * std::f64::consts::PI, true);
        dot.push(PathOp::Close);
        cv.path(dot, Some(dial.needle), None, None);
    });
}

/// An `Arc2D` of `area` (from `start`, `extent` degrees, closed as a pie wedge or a chord)
/// as an SVG chart draws it. Batik writes the arc as an SVG arc from its start point to its
/// end point, both to 4 decimals of the chart's coordinates, and draws it back as the SVG
/// specification finds an arc's center (`ExtendedGeneralPath.computeArc`), in floats. When
/// the two points are a diameter apart, as the ends of a 180 degree meter are, it first makes
/// the radii 1.00001 times as long, which moves the center 0.00447 radii off the line between
/// them: down for an arc drawn counter-clockwise over the top.
fn svg_arc(area: Rect, start: f64, extent: f64, pie: bool, origin: (f64, f64)) -> Vec<PathOp> {
    let (ox, oy) = origin;
    let r4 = |v: f64| ((v * 1e4).round() / 1e4) as f32 as f64;
    // Arc2D.getStartPoint and getEndPoint.
    let point = |deg: f64| {
        let a = (-deg).to_radians();
        (r4(area.x - ox + (a.cos() * 0.5 + 0.5) * area.w), r4(area.y - oy + (a.sin() * 0.5 + 0.5) * area.h))
    };
    let (x0, y0) = point(start);
    let (x, y) = point(start + extent);
    let (mut rx, mut ry) = (r4(area.w / 2.0), r4(area.h / 2.0));
    let (large, sweep) = if extent > 0.0 { (extent > 180.0, false) } else { (extent < -180.0, true) };
    let (x1, y1) = ((x0 - x) / 2.0, (y0 - y) / 2.0);
    let check = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if check > 0.99999 {
        let scale = check.sqrt() * 1.00001;
        rx *= scale;
        ry *= scale;
    }
    let (prx, pry, px1, py1) = (rx * rx, ry * ry, x1 * x1, y1 * y1);
    let sq = ((prx * pry - prx * py1 - pry * px1) / (prx * py1 + pry * px1)).max(0.0);
    let coef = if large == sweep { -1.0 } else { 1.0 } * sq.sqrt();
    let (cx1, cy1) = (coef * (rx * y1 / ry), coef * -(ry * x1 / rx));
    let (cx, cy) = ((x0 + x) / 2.0 + cx1, (y0 + y) / 2.0 + cy1);
    let (ux, uy) = ((x1 - cx1) / rx, (y1 - cy1) / ry);
    let (vx, vy) = ((-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let a0 = if uy < 0.0 { -1.0 } else { 1.0 } * (ux / (ux * ux + uy * uy).sqrt()).acos();
    let cos = ((ux * vx + uy * vy) / ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt()).clamp(-1.0, 1.0);
    let mut da = if ux * vy - uy * vx < 0.0 { -1.0 } else { 1.0 } * cos.acos();
    if !sweep && da > 0.0 {
        da -= 2.0 * std::f64::consts::PI;
    } else if sweep && da < 0.0 {
        da += 2.0 * std::f64::consts::PI;
    }
    // computeArc's angles are the SVG's own, y down.
    let (cx, cy) = (cx + ox, cy + oy);
    // The path goes on from the written start point to where the arc starts.
    let mut ops = vec![PathOp::Move(x0 + ox, y0 + oy), PathOp::Line(cx + rx * a0.cos(), cy + ry * a0.sin())];
    arc(&mut ops, cx, cy, rx, ry, a0, a0 + da, false);
    if pie {
        let center = area.center_x() - ox;
        ops.push(PathOp::Line(r4(center) + ox, r4(area.center_y() - oy) + oy));
    }
    ops.push(PathOp::Close);
    ops
}

/// `item` seen through `color` at `alpha`. Clipping paths stay as they are.
fn tint(item: &PrintItem, color: Color, alpha: f32) -> PrintItem {
    match item {
        PrintItem::Path { ops, fill, stroke, dash } => PrintItem::Path {
            ops: ops.clone(),
            fill: fill.map(|f| blend(color, alpha, f)),
            stroke: stroke.map(|(s, w)| (blend(color, alpha, s), w)),
            dash: *dash,
        },
        PrintItem::Transform(m, inner) => PrintItem::Transform(*m, inner.iter().map(|i| tint(i, color, alpha)).collect()),
        PrintItem::Group(inner) => PrintItem::Group(inner.iter().map(|i| tint(i, color, alpha)).collect()),
        other => other.clone(),
    }
}

/// Where the meter is and how values map to angles on it.
struct Dial {
    cx: f64,
    cy: f64,
    r: f64,
    lower: f64,
    upper: f64,
    angle: f64,
    shape: MeterShape,
    /// Drawn as SVG, with the chart's top left corner (see [`svg_arc`]).
    svg: bool,
    origin: (f64, f64),
}

impl Dial {
    /// `MeterPlot.valueToAngle`: degrees counter-clockwise from three o'clock, the low end
    /// as far left as the meter's angle reaches.
    fn angle_of(&self, v: f64) -> f64 {
        let base = 180.0 + (self.angle - 180.0) / 2.0;
        base - (v - self.lower) / (self.upper - self.lower) * self.angle
    }

    /// `MeterPlot.fillArc`: the arc of `area` from `min` to `max`, closed as a pie wedge; the
    /// whole dial (`dial`) is a whole circle for a circle meter, and closed as a chord for a
    /// chord meter of more than 180 degrees.
    fn fill_arc(&self, area: Rect, min: f64, max: f64, dial: bool) -> Vec<PathOp> {
        let start = self.angle_of(max);
        let mut extent = self.angle_of(min) - start;
        let mut pie = true;
        match self.shape {
            MeterShape::Pie => {}
            MeterShape::Chord => pie = !(dial && self.angle > 180.0),
            MeterShape::Circle if dial => extent = 360.0,
            MeterShape::Circle => {}
        }
        if self.svg && extent.abs() < 360.0 {
            return svg_arc(area, start, extent, pie, self.origin);
        }
        // Arc2D: angles counter-clockwise on the screen, y down.
        let (cx, cy, rx, ry) = (area.center_x(), area.center_y(), area.w / 2.0, area.h / 2.0);
        let mut ops = Vec::new();
        arc(&mut ops, cx, cy, rx, ry, -start.to_radians(), -(start + extent).to_radians(), true);
        if pie {
            ops.push(PathOp::Line(cx, cy));
        }
        ops.push(PathOp::Close);
        ops
    }

    /// `MeterPlot.drawTick`: a tick at `v`, and its label when there is one. The label starts
    /// at the tick's inner end on the left half, ends there on the right half and is centered
    /// on it straight up or down; it sits above that point within 45 degrees of the
    /// horizontal, below it elsewhere.
    fn tick(&self, cv: &mut Canvas, dial: &super::DialPlot, v: f64, label: Option<&str>) {
        let degrees = self.angle_of(v);
        let (c, s) = (degrees.to_radians().cos(), degrees.to_radians().sin());
        let outer = self.r + BORDER;
        let inner = outer - 15.0;
        let (x1, y1) = (self.cx + outer * c, self.cy - outer * s);
        let (x2, y2) = (self.cx + inner * c, self.cy - inner * s);
        cv.line_square(x1, y1, x2, y2, dial.tick_color, 2.0);
        let Some(text) = label else { return };
        let font = &dial.tick_font;
        let width = cv.awt_width(font, text);
        let height = cv.metrics(font).height;
        let mut x = x2;
        if degrees == 90.0 || degrees == 270.0 {
            x -= width / 2.0;
        } else if !(90.0..=270.0).contains(&degrees) {
            x -= width;
        }
        let y = if (degrees > 135.0 && degrees < 225.0) || !(45.0..=315.0).contains(&degrees) {
            y2 - height / 2.0
        } else {
            y2 + height / 2.0
        };
        cv.draw_string(font, Color::BLACK, text, x as f32 as f64, y as f32 as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{render, DataState};
    use super::*;
    use std::collections::HashMap;

    /// The meter of JasperReports' MeterChartReport sample (JRXML 7), as JasperReports 7.1.1
    /// draws it for the countries starting with A.
    const TEMPLATE: &str = r##"<jasperReport name="M" pageWidth="595" pageHeight="842" columnWidth="555" leftMargin="20" rightMargin="20" topMargin="30" bottomMargin="30">
	<title height="175">
		<element kind="chart" chartType="meter" x="0" y="0" width="270" height="175" showLegend="false">
			<dataset kind="value"><valueExpression><![CDATA[7990.08d]]></valueExpression></dataset>
			<plot units="Freight" tickInterval="1000.0" needleColor="#000066" tickColor="#0000FF" meterColor="#777777">
				<dataRange><lowExpression><![CDATA[0.0d]]></lowExpression><highExpression><![CDATA[17500.0d]]></highExpression></dataRange>
				<valueDisplay color="#000000"><font fontSize="12.0"/></valueDisplay>
				<interval label="Small" backgroundColor="#FF6666"><dataRange><lowExpression><![CDATA[0.0d]]></lowExpression><highExpression><![CDATA[2000.0d]]></highExpression></dataRange></interval>
				<interval label="Med" backgroundColor="#FFFF00" alpha="0.4"><dataRange><lowExpression><![CDATA[2000.0d]]></lowExpression><highExpression><![CDATA[12000.0d]]></highExpression></dataRange></interval>
				<interval label="Large" backgroundColor="#66CC66" alpha="0.5"><dataRange><lowExpression><![CDATA[12000.0d]]></lowExpression><highExpression><![CDATA[17500.0d]]></highExpression></dataRange></interval>
			</plot>
		</element>
	</title>
</jasperReport>"##;

    fn paths(items: &[PrintItem], depth: usize, out: &mut Vec<(usize, Vec<PathOp>, Option<Color>, Option<(Color, f32)>)>) {
        for item in items {
            match item {
                PrintItem::Path { ops, fill, stroke, .. } => out.push((depth, ops.to_vec(), *fill, *stroke)),
                PrintItem::Transform(_, inner) | PrintItem::Group(inner) => paths(inner, depth + 1, out),
                _ => {}
            }
        }
    }

    fn points(ops: &[PathOp]) -> Vec<(f64, f64)> {
        ops.iter()
            .filter_map(|op| match *op {
                PathOp::Move(x, y) | PathOp::Line(x, y) | PathOp::Cubic(_, _, _, _, x, y) => Some((x, y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn meter_follows_jfreechart() {
        let report = crate::parse(TEMPLATE).unwrap();
        let ch = report.title.as_ref().unwrap().elements.iter().find_map(|e| match &e.kind {
            crate::model::ElementKind::Chart(c) => Some(c.as_ref().clone()),
            _ => None,
        });
        let ch = ch.expect("a chart");
        let texts = ChartTexts {
            dial: Some((0.0, 17500.0)),
            intervals: vec![Some((0.0, 2000.0)), Some((2000.0, 12000.0)), Some((12000.0, 17500.0))],
            ..Default::default()
        };
        let data = DataState { value: Some(7990.08), ..Default::default() };
        let all = HashMap::from([(ch.id, data)]);
        let locale = crate::format::locale::EN_US.clone();
        let mut fonts = crate::fonts::FontSet::new();
        let items = render(&ch, &all, &texts, &mut fonts, &locale, 0.0, 0.0, 270.0, 175.0);
        let mut found = Vec::new();
        paths(&items, 0, &mut found);
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        // The plot is 246 by 159 after its insets; a half circle counts it 1.8 times as tall:
        // a circle of radius 120 around (135, 151.1).
        let (cx, cy, r) = (135.0, 8.0 + 3.0 + (153.0 + 159.0 / 1.25) / 2.0, 120.0);
        // The dial's background, 4 past the circle, from the right end over the top.
        let gray = Color { r: 0x77, g: 0x77, b: 0x77 };
        let back = found.iter().find(|p| p.2 == Some(gray)).expect("dial background");
        let pts = points(&back.1);
        assert!(near(pts[0].0, cx + r + 4.0) && near(pts[0].1, cy), "{:?}", pts[0]);
        assert!(pts.iter().any(|p| near(p.1, cy - r - 4.0)));
        // 18 ticks every 1,000 and two at the ends of each interval, blue and 2 wide, from 3
        // outside the circle to 12 inside it (square caps: 1 further each way).
        let blue = Color { r: 0, g: 0, b: 0xFF };
        let ticks: Vec<_> = found.iter().filter(|p| p.3 == Some((blue, 2.0))).collect();
        assert_eq!(ticks.len(), 18 + 6);
        let zero = points(&ticks[0].1);
        assert!(near(zero[0].0, cx - r - 4.0) && near(zero[1].0, cx - r + 13.0) && near(zero[0].1, cy), "{zero:?}");
        // The translucent medium interval: yellow at 40% over white, and the ticks under it
        // seen through it.
        let med = Color { r: 0xFF, g: 0xFF, b: 0x99 };
        assert!(found.iter().any(|p| p.2 == Some(med)));
        let under = blend(Color { r: 0xFF, g: 0xFF, b: 0 }, 0.4, blue);
        assert!(found.iter().any(|p| p.3 == Some((under, 2.0))));
        // The needle: dark blue, to whole points, its base 5 wide across the center.
        let needle = Color { r: 0, g: 0, b: 0x66 };
        let arrow = found.iter().find(|p| p.2 == Some(needle)).expect("needle");
        let pts = points(&arrow.1);
        assert_eq!(pts.len(), 3);
        assert!(pts.iter().all(|p| p.0.fract() == 0.0 && p.1.fract() == 0.0), "{pts:?}");
        assert_eq!((pts[0], pts[1]), ((132.0, 151.0), (137.0, 151.0)));
        // Everything is clipped to the square around the dial's background.
        let clip = found.iter().find(|p| p.0 == 2 && p.2.is_none() && p.3.is_none()).expect("clip");
        let pts = points(&clip.1);
        assert!(near(pts[0].0, cx - r - 4.0) && near(pts[0].1, cy - r - 4.0), "{pts:?}");
    }

    /// A 180 degree dial background drawn as SVG (one of the meter sample's): Batik finds the
    /// arc's center 0.398 below where it is, as JasperReports 7.1.1's output shows.
    #[test]
    fn half_circle_drawn_as_svg_moves_down() {
        let area = Rect::new(11.0, 174.6, 178.0, 178.0);
        let pts = points(&svg_arc(area, 0.0, 180.0, true, (0.0, 0.0)));
        let top = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        assert!((top - (263.6 - 89.0 + 0.398)).abs() < 0.002, "{top}");
        // From the right end, over the top, back to the center.
        assert!((pts[0].0 - 189.0).abs() < 1e-4 && (pts[pts.len() - 1].0 - 100.0).abs() < 1e-4, "{pts:?}");
        // A quarter circle stays where it is.
        let pts = points(&svg_arc(area, 90.0, 90.0, true, (0.0, 0.0)));
        let top = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        assert!((top - 174.6).abs() < 1e-3, "{top}");
    }
}

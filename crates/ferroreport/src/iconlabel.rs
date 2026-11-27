//! The icon label component (`iconLabel`): a label and an icon, two text fields, side by side
//! in a frame, laid out the way JasperReports 7 lays them out
//! (`net.sf.jasperreports.components.iconlabel.IconLabelComponentFill`).
//!
//! The parser reads the component as a frame of its two text fields, the label then the icon,
//! with a hidden third element that carries the component's settings ([`marker`]); the public
//! model has no place for them in 1.x. The filler measures both texts ([`prepare`]) and places
//! them ([`place`]) where JasperReports does:
//!
//! - The frame is the component element, its box the component's: its padding is the room
//!   around the label and the icon, which are drawn inside it (exporters offset a frame's
//!   elements by its left and top padding).
//! - Between the label and the icon goes a "middle padding" of half the icon's font size, in
//!   whole points.
//! - The icon is as wide as its text and its padding; the label gets the rest of the width.
//!   When the icon does not fit beside the label, they go one above the other ("vertical"),
//!   both as wide as the element.
//! - `labelFill` (`None` by default): with `Horizontal` or `Both` the label keeps all the
//!   width it got, otherwise it shrinks to its text (its widest line, padding and 3 points
//!   more). With `Vertical` or `Both` it is as tall as the element inside the padding. The
//!   label and the icon then take the taller of their two heights.
//! - `iconPosition`: `Start` puts the icon before the label (left of it, or above), anything
//!   else (`End`, the default) after it.

use crate::expr::Expr;
use crate::model::{BoxStyle, Color, Element, ElementKind, Mode, Padding, PositionType, StretchType};
use crate::value::Value;

/// Where the icon goes (`iconPosition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconPosition {
    Start,
    /// Also when not given: JasperReports tests for `Start` only.
    End,
}

/// How the label fills the element (`labelFill`, `ContainerFillEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelFill {
    None,
    Horizontal,
    Vertical,
    Both,
}

impl LabelFill {
    fn horizontal(self) -> bool {
        matches!(self, LabelFill::Horizontal | LabelFill::Both)
    }
    fn vertical(self) -> bool {
        matches!(self, LabelFill::Vertical | LabelFill::Both)
    }
}

/// The settings of an icon label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    pub icon_position: IconPosition,
    pub label_fill: LabelFill,
}

impl Settings {
    /// From the component's attributes.
    pub fn read(attr: &dyn Fn(&str) -> Option<String>) -> Settings {
        Settings {
            icon_position: match attr("iconPosition").as_deref() {
                Some("Start") => IconPosition::Start,
                _ => IconPosition::End,
            },
            label_fill: match attr("labelFill").as_deref() {
                Some("Horizontal") => LabelFill::Horizontal,
                Some("Vertical") => LabelFill::Vertical,
                Some("Both") => LabelFill::Both,
                _ => LabelFill::None,
            },
        }
    }

    fn write(self) -> String {
        format!("iconPosition={:?} labelFill={:?}", self.icon_position, self.label_fill)
    }

    fn parse(s: &str) -> Settings {
        let words: Vec<(&str, &str)> = s.split(' ').filter_map(|w| w.split_once('=')).collect();
        let attr = |name: &str| words.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
        Settings::read(&attr)
    }
}

/// The key of the hidden element that carries an icon label's settings.
const MARKER_KEY: &str = "net.sf.jasperreports.components.iconlabel";

/// The hidden last element of an icon label's frame: never printed, its key names the
/// component and its settings.
pub(crate) fn marker(settings: Settings) -> Element {
    Element {
        key: Some(format!("{MARKER_KEY} {}", settings.write())),
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        position_type: PositionType::Fix,
        stretch_type: StretchType::NoStretch,
        mode: Mode::Transparent,
        forecolor: Color::BLACK,
        backcolor: Color { r: 255, g: 255, b: 255 },
        print_when: Some(Expr::Lit(Value::Bool(false))),
        remove_line_when_blank: false,
        print_when_detail_overflows: false,
        print_repeated_values: true,
        print_in_first_whole_band: false,
        print_when_group_changes: None,
        conditional: None,
        tag: None,
        link: None,
        kind: ElementKind::Frame { children: Vec::new(), boxed: BoxStyle::plain(Color::BLACK) },
    }
}

/// The settings of the icon label whose frame has `children`, with its label and icon (`None`:
/// not an icon label).
pub(crate) fn of(children: &[Element]) -> Option<(Settings, &Element, &Element)> {
    let [label, icon, marker] = children else { return None };
    let rest = marker.key.as_deref()?.strip_prefix(MARKER_KEY)?;
    let text = |e: &Element| matches!(e.kind, ElementKind::Text(_));
    (text(label) && text(icon)).then(|| (Settings::parse(rest.trim()), label, icon))
}

/// What measuring a text field gave: the width of its widest line (`getTextWidth`) and its
/// height once stretched (`getStretchHeight`), padding included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Measured {
    pub text_width: f32,
    pub height: f32,
}

/// What [`prepare`] works out before the element's final height is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Prepared {
    /// The component's left and right padding (narrowed when the element is narrower).
    pub left: f32,
    pub right: f32,
    /// The label above or below the icon rather than beside it.
    pub vertical: bool,
    pub middle: f32,
    pub icon_visible: bool,
    /// The widths the two text fields were measured in.
    pub label_width: f32,
    pub icon_width: f32,
    pub label: Measured,
    pub icon: Measured,
    /// Left plus right padding of the label's and the icon's text field.
    pub label_padding: f32,
    pub icon_padding: f32,
    /// The height the component asks for, padding included.
    pub height: f32,
}

/// What [`prepare`] needs to know of the two text fields, besides measuring them.
pub(crate) struct Fields {
    /// Left plus right padding of each.
    pub label_padding: f32,
    pub icon_padding: f32,
    pub icon_font_size: f32,
    /// The label stretches with its text (`textAdjust="StretchHeight"`).
    pub label_stretches: bool,
}

/// Measures an icon label `width` wide and `height` tall (as designed) with the `padding` of
/// its box, with `available` points of height left for it: `IconLabelComponentFill.prepare`.
/// `measure(icon, width)` measures the icon's text field (`true`) or the label's `width` wide.
pub(crate) fn prepare(
    width: f32,
    height: f32,
    padding: Padding,
    available: f32,
    f: &Fields,
    measure: &mut dyn FnMut(bool, f32) -> Measured,
) -> Prepared {
    // An element narrower than its padding (and a point) narrows its left and right padding,
    // each by half of what is missing.
    let (mut left, mut right) = (padding.left, padding.right);
    let missing = width - left - right - 1.0;
    if missing < 0.0 {
        left = (left + (missing / 2.0).floor()).max(0.0);
        right = (right + (missing / 2.0).ceil()).max(0.0);
    }
    let inner = width - left - right;
    // Half the icon's font size between them, if the width leaves room for it.
    let half = (f.icon_font_size / 2.0).trunc();
    let mut middle = if inner <= half { 0.0 } else { (inner - half).min(half) };
    let available_width = inner - middle;
    let text_height = available - padding.top - padding.bottom;

    let mut icon = measure(true, available_width);
    let mut icon_width = available_width;
    // Beside each other when the icon's text fits on its lines and leaves room for the label.
    let mut used = 0.0;
    let mut vertical = icon.text_width > available_width - f.icon_padding;
    if !vertical {
        used = icon.text_width.trunc() + f.icon_padding;
        vertical = available_width <= used;
    }
    let label_width = if vertical { available_width + middle } else { available_width - used };
    let label = measure(false, label_width);

    let mut icon_visible = true;
    if vertical {
        // One above the other: the icon is printed if there is room for it under the label,
        // as wide as the label.
        middle = half;
        let room = if f.label_stretches { text_height - label.height } else { height - label.height } - middle;
        icon_visible = room > 0.0;
        if icon_visible {
            icon_width = label_width;
            icon = measure(true, icon_width);
            icon_visible = icon.height <= room;
        }
    }
    let icon_height = if icon_visible { icon.height } else { 0.0 };
    let content = if vertical {
        label.height + if icon_visible { icon_height + middle } else { 0.0 }
    } else {
        label.height.max(icon_height)
    };
    Prepared {
        left,
        right,
        vertical,
        middle,
        icon_visible,
        label_width,
        icon_width,
        label,
        icon,
        label_padding: f.label_padding,
        icon_padding: f.icon_padding,
        height: content + padding.top + padding.bottom,
    }
}

/// Where the label and the icon go: x, y, width and height in the frame, inside its padding
/// (`None`: the icon is not printed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    pub label: [f32; 4],
    pub icon: Option<[f32; 4]>,
}

/// Places the label and the icon of an icon label `width` wide and finally `height` tall (its
/// height after stretching, which JasperReports gives the component before filling it), with
/// the `padding` of its box: `IconLabelComponentFill.fillHorizontal` and `fillVertical`.
pub(crate) fn place(s: Settings, p: &Prepared, padding: Padding, height: f32) -> Placed {
    let start = s.icon_position == IconPosition::Start;
    let inner_height = height - padding.top - padding.bottom;
    let (label, icon) = if !p.vertical {
        let mut lw = p.label_width;
        if !s.label_fill.horizontal() {
            // The label shrinks to its text, never wider than it was measured.
            lw = lw.min(p.label.text_width.trunc() + p.label_padding + 3.0);
        }
        let mut lh = p.label.height;
        if s.label_fill.vertical() {
            lh = lh.max(inner_height);
        }
        let iw = (p.icon.text_width + p.icon_padding).ceil();
        let common = lh.max(p.icon.height);
        let (lx, ix) = if start { (iw + p.middle, 0.0) } else { (0.0, lw + p.middle) };
        ([lx, 0.0, lw, common], [ix, 0.0, iw, common])
    } else {
        let (lw, iw) = (p.label_width, p.icon_width);
        let mut lh = p.label.height;
        if s.label_fill.vertical() {
            lh = lh.max(inner_height - (p.icon.height + p.middle));
        }
        let ih = p.icon.height;
        let (ly, iy) = if start { (ih + p.middle, 0.0) } else { (0.0, lh + p.middle) };
        ([0.0, ly, lw, lh], [0.0, iy, iw, ih])
    };
    Placed { label, icon: p.icon_visible.then_some(icon) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PADDING: Padding = Padding { top: 10.0, left: 10.0, bottom: 10.0, right: 10.0 };
    const FIELDS: Fields = Fields { label_padding: 10.0, icon_padding: 20.0, icon_font_size: 12.0, label_stretches: true };

    /// The icon's text 36.4 wide on one line, the label's 40.2: the fields of JasperReports'
    /// iconlabel sample, with its padding.
    fn measure(icon: bool, width: f32) -> Measured {
        if icon {
            Measured { text_width: 36.4_f32.min(width - 20.0), height: 34.0 }
        } else {
            Measured { text_width: 40.2_f32.min(width - 10.0), height: 20.0 }
        }
    }

    fn settings(icon_position: IconPosition, label_fill: LabelFill) -> Settings {
        Settings { icon_position, label_fill }
    }

    #[test]
    fn the_settings_go_with_the_frame() {
        let s = settings(IconPosition::Start, LabelFill::Both);
        assert_eq!(Settings::parse(&s.write()), s);
        let defaults = Settings::read(&|_| None);
        assert_eq!(defaults, settings(IconPosition::End, LabelFill::None));
    }

    #[test]
    fn side_by_side_the_label_gets_the_width_the_icon_leaves() {
        // 180 inside the padding, 6 between them (half the icon's 12 points), the icon as wide as
        // its text and padding (36 + 20 = 56 when deciding, 57 when printed).
        let p = prepare(200.0, 10.0, PADDING, f32::INFINITY, &FIELDS, &mut measure);
        assert!(!p.vertical && p.icon_visible);
        assert_eq!((p.middle, p.label_width), (6.0, 118.0));
        // As tall as the taller field, and the padding.
        assert_eq!(p.height, 54.0);

        // labelFill="None": the label shrinks to its text (40 + 10 + 3), the icon follows it.
        let placed = place(settings(IconPosition::End, LabelFill::None), &p, PADDING, 54.0);
        assert_eq!(placed.label, [0.0, 0.0, 53.0, 34.0]);
        assert_eq!(placed.icon, Some([59.0, 0.0, 57.0, 34.0]));
        // labelFill="Horizontal": the label keeps its 118, after the icon at the start.
        let placed = place(settings(IconPosition::Start, LabelFill::Horizontal), &p, PADDING, 54.0);
        assert_eq!(placed.label, [63.0, 0.0, 118.0, 34.0]);
        assert_eq!(placed.icon, Some([0.0, 0.0, 57.0, 34.0]));
        // labelFill="Vertical" in an element stretched to 100: both as tall as its inside.
        let placed = place(settings(IconPosition::End, LabelFill::Vertical), &p, PADDING, 100.0);
        assert_eq!(placed.label, [0.0, 0.0, 53.0, 80.0]);
        assert_eq!(placed.icon, Some([59.0, 0.0, 57.0, 80.0]));
    }

    #[test]
    fn without_room_beside_the_label_the_icon_goes_under_it() {
        // 40 inside the padding: 34 for the icon, which needs 56.
        let p = prepare(60.0, 10.0, PADDING, f32::INFINITY, &FIELDS, &mut measure);
        assert!(p.vertical && p.icon_visible);
        assert_eq!((p.label_width, p.icon_width, p.middle), (40.0, 40.0, 6.0));
        assert_eq!(p.height, 20.0 + 6.0 + 34.0 + 20.0);
        let placed = place(settings(IconPosition::End, LabelFill::None), &p, PADDING, p.height);
        assert_eq!(placed.label, [0.0, 0.0, 40.0, 20.0]);
        assert_eq!(placed.icon, Some([0.0, 26.0, 40.0, 34.0]));
        let placed = place(settings(IconPosition::Start, LabelFill::None), &p, PADDING, p.height);
        assert_eq!(placed.label, [0.0, 40.0, 40.0, 20.0]);
        assert_eq!(placed.icon, Some([0.0, 0.0, 40.0, 34.0]));
        // A label that does not stretch leaves the icon the element's own height: none here.
        let fixed = Fields { label_stretches: false, ..FIELDS };
        let p = prepare(60.0, 10.0, PADDING, f32::INFINITY, &fixed, &mut measure);
        assert!(p.vertical && !p.icon_visible);
        assert_eq!(p.height, 40.0);
        assert_eq!(place(settings(IconPosition::End, LabelFill::None), &p, PADDING, 40.0).icon, None);
    }

    #[test]
    fn an_element_narrower_than_its_padding_narrows_it() {
        // 15 wide with 10 on each side: 6 points short, 3 off each side.
        let p = prepare(15.0, 10.0, PADDING, f32::INFINITY, &FIELDS, &mut measure);
        assert_eq!((p.left, p.right), (7.0, 7.0));
    }
}

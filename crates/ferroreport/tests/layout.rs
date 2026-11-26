//! Band layout rules checked against JasperReports 7.1.1 on small templates: blank elements
//! that give their space back, and elements printed again where a band continues.

use ferroreport::print::{PrintItem, PrintText};
use ferroreport::Renderer;

/// Every line of text in the document: (page, baseline, text).
fn lines(template: &str, rows: usize) -> Vec<(usize, f32, String)> {
    let report = ferroreport::parse(template).unwrap();
    let data = serde_json::Value::Array((1..=rows).map(|n| serde_json::json!({ "n": n })).collect());
    let records = ferroreport::data::records_from_json(&report, &data).unwrap();
    let doc = Renderer::new().fill(&report, &records, Default::default()).unwrap();
    fn walk(item: &PrintItem, page: usize, out: &mut Vec<(usize, f32, String)>) {
        match item {
            PrintItem::Text(PrintText { lines, .. }) => out.extend(lines.iter().map(|l| (page, l.baseline, l.text.clone()))),
            PrintItem::Cell(_, inner) => walk(inner, page, out),
            PrintItem::Group(items) | PrintItem::Paragraphs(_, items) => items.iter().for_each(|i| walk(i, page, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (page, p) in doc.pages.iter().enumerate() {
        p.items.iter().for_each(|i| walk(i, page + 1, &mut out));
    }
    out
}

fn first_lines(all: &[(usize, f32, String)], prefix: &str) -> Vec<(usize, f32)> {
    all.iter().filter(|l| l.2.starts_with(prefix)).map(|l| (l.0, l.1)).collect()
}

#[test]
fn a_blank_element_keeps_its_space_where_a_stretched_neighbour_overlaps_it() {
    // The static text never prints (isPrintRepeatedValues="false") and has
    // isRemoveLineWhenBlank. Nothing overlaps it in the design, but once the text field above
    // it has stretched and it has floated down, the frame beside it, which stretched too, does:
    // JasperReports keeps its 47 points, and the bands are 130 points apart (three to a page).
    let template = r#"<jasperReport name="keep" pageWidth="300" pageHeight="400" columnWidth="260" leftMargin="20" rightMargin="20" topMargin="20" bottomMargin="20">
        <field name="n" class="java.lang.Integer"/>
        <detail><band height="60">
            <element kind="textField" x="0" y="5" width="120" height="0" fontName="DejaVu Sans" fontSize="10.0" textAdjust="StretchHeight"><expression><![CDATA["Row " + $F{n} + ": a long text that stretches its field over several lines and moves the elements below it down."]]></expression></element>
            <element kind="line" positionType="Float" x="0" y="8" width="120" height="1"/>
            <element kind="frame" positionType="Float" x="130" y="4" width="130" height="4">
                <element kind="textField" x="5" y="1" width="120" height="0" fontName="DejaVu Sans" fontSize="10.0" textAdjust="StretchHeight"><expression><![CDATA["Framed " + $F{n} + ": a long text that stretches the frame around it over several lines, and splits with it at the end of a page."]]></expression></element>
            </element>
            <element kind="staticText" positionType="Float" x="0" y="9" width="120" height="47" printRepeatedValues="false" removeLineWhenBlank="true"><text><![CDATA[Space remover]]></text></element>
        </band></detail>
    </jasperReport>"#;
    let rows = first_lines(&lines(template, 6), "Row ");
    assert_eq!(rows.iter().filter(|r| r.0 == 1).count(), 3, "{rows:?}");
    assert!((rows[1].1 - rows[0].1 - 130.0).abs() < 0.01, "{rows:?}");
}

#[test]
fn a_blank_element_gives_its_space_back_where_it_has_floated_to() {
    // The field beside the static text overlaps it in the design, but not where the static text
    // has floated to, under the stretched field: its 47 points go, and the bands are 25 points
    // apart, as in JasperReports 7.1.1. (JasperReports' stretch sample uses such a "space
    // remover"; keeping its space made that sample 21 pages long instead of 14.)
    let template = r#"<jasperReport name="remove" pageWidth="300" pageHeight="400" columnWidth="260" leftMargin="20" rightMargin="20" topMargin="20" bottomMargin="20">
        <field name="n" class="java.lang.Integer"/>
        <detail><band height="60">
            <element kind="textField" x="0" y="5" width="120" height="0" fontName="DejaVu Sans" fontSize="10.0" textAdjust="StretchHeight"><expression><![CDATA["Row " + $F{n} + ": a text."]]></expression></element>
            <element kind="line" positionType="Float" x="0" y="8" width="120" height="1"/>
            <element kind="staticText" positionType="Float" x="0" y="9" width="120" height="47" printRepeatedValues="false" removeLineWhenBlank="true"><text><![CDATA[Space remover]]></text></element>
            <element kind="textField" x="130" y="5" width="120" height="16" fontName="DejaVu Sans" fontSize="10.0"><expression><![CDATA["Beside " + $F{n}]]></expression></element>
        </band></detail>
    </jasperReport>"#;
    let rows = first_lines(&lines(template, 2), "Row ");
    assert!((rows[1].1 - rows[0].1 - 25.0).abs() < 0.01, "{rows:?}");
}

#[test]
fn an_element_that_continues_is_not_printed_again_in_full() {
    // A frame with isPrintWhenDetailOverflows whose text goes on to the next page continues
    // there; it is not printed again from its start on top of its continuation (which made
    // JasperReports' stretch sample 21 pages long instead of 14). The line printed again keeps
    // the band's top: the continuation starts where it does in JasperReports 7.1.1, a point
    // above the text's first line on the first page.
    let words: Vec<String> = (1..140).map(|i| format!("w{i}")).collect();
    let template = format!(
        r#"<jasperReport name="reprint" pageWidth="300" pageHeight="400" columnWidth="260" leftMargin="20" rightMargin="20" topMargin="20" bottomMargin="20">
        <field name="n" class="java.lang.Integer"/>
        <detail><band height="30">
            <element kind="line" positionType="Float" x="0" y="4" width="120" height="1" printWhenDetailOverflows="true"/>
            <element kind="frame" positionType="Float" x="130" y="4" width="130" height="4" printWhenDetailOverflows="true">
                <element kind="textField" x="5" y="1" width="120" height="0" fontName="DejaVu Sans" fontSize="10.0" textAdjust="StretchHeight"><expression><![CDATA["Framed " + $F{{n}} + " {} end"]]></expression></element>
                <box><pen lineWidth="1.0"/></box>
            </element>
        </band></detail>
    </jasperReport>"#,
        words.join(" ")
    );
    let all = lines(&template, 1);
    let framed = first_lines(&all, "Framed");
    assert_eq!(framed.len(), 1, "{framed:?}");
    let next = all.iter().filter(|l| l.0 == 2).map(|l| l.1).fold(f32::INFINITY, f32::min);
    assert!(all.iter().any(|l| l.0 == 2 && l.2.ends_with("end")), "{all:?}");
    assert!((next - framed[0].1 - (25.7 - 26.7)).abs() < 0.05, "{next} {framed:?}");
}

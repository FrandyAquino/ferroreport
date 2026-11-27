//! Every template in examples/ renders: the flat `*.ferro` reports, with the `*.json` rows
//! named like them, and each folder's `report.jrxml` or `report.ferro`, with the `data.json`
//! and `params.json` beside it. A `.ferro` file that is not a report (one that only holds
//! styles and parts for others to import) is left out.

use std::path::{Path, PathBuf};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// The rows of a flat example: `<name>.json`, or else the rows of the example its name starts
/// with (`invoices-short.ferro` takes `invoices.json`).
fn flat_rows(dir: &Path, stem: &str) -> Option<serde_json::Value> {
    let mut stem = stem;
    loop {
        if let Some(rows) = json(&dir.join(format!("{stem}.json"))) {
            return Some(rows);
        }
        stem = &stem[..stem.rfind('-')?];
    }
}

/// Renders one template to PDF, with its folder for imports, images and subreports; the
/// number of pages.
fn render(template: &Path, rows: Option<serde_json::Value>, params: Option<serde_json::Value>) -> usize {
    let name = template.display();
    let source = std::fs::read_to_string(template).unwrap();
    let dir = template.parent().unwrap();
    let mut renderer = ferroreport::Renderer::new().resource_dir(dir);
    let report = renderer.parse(&source).unwrap_or_else(|e| panic!("{name}: {e}"));
    let rows = rows.unwrap_or_else(|| serde_json::json!([]));
    let records = ferroreport::data::records_from_json(&report, &rows).unwrap_or_else(|e| panic!("{name}: {e}"));
    let params = ferroreport::params_from_json(&report, &params.unwrap_or_else(|| serde_json::json!({})))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let doc = renderer.fill(&report, &records, params).unwrap_or_else(|e| panic!("{name}: {e}"));
    let pdf = renderer.to_pdf(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));
    let end = pdf.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(0, |i| i + 1);
    assert!(pdf.starts_with(b"%PDF-") && pdf[..end].ends_with(b"%%EOF"), "{name}: not a whole PDF");
    doc.pages.len()
}

#[test]
fn every_example_renders() {
    let dir = examples();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    let mut rendered = Vec::new();
    for path in entries {
        let template = if path.is_dir() {
            match ["report.jrxml", "report.ferro"].iter().map(|f| path.join(f)).find(|p| p.exists()) {
                Some(t) => t,
                None => continue,
            }
        } else if path.extension().is_some_and(|e| e == "ferro") {
            path.clone()
        } else {
            continue;
        };
        let (rows, params) = if path.is_dir() {
            (json(&path.join("data.json")), json(&path.join("params.json")))
        } else {
            let source = std::fs::read_to_string(&path).unwrap();
            // A file of styles and parts to import, not a report.
            if !source.lines().any(|l| l.trim_start().starts_with("report ")) {
                continue;
            }
            (flat_rows(&dir, path.file_stem().unwrap().to_str().unwrap()), None)
        };
        let pages = render(&template, rows, params);
        assert!(pages > 0, "{}: no pages", template.display());
        rendered.push(path.file_name().unwrap().to_string_lossy().into_owned());
    }
    // The examples the README links to are among them.
    for name in ["employees.ferro", "invoices.ferro", "invoices-short.ferro"] {
        assert!(rendered.iter().any(|r| r == name), "{name} was not rendered: {rendered:?}");
    }
}

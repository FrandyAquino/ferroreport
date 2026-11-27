//! Renders every fixture and checks the page count recorded from JasperReports.
//! Pixel-level comparison against Jasper lives in `tools/compare.py`.

use std::path::PathBuf;

use ferroreport::sql::{Connection, SqlRow, SqlRows, SqlValue};

/// A fixture with `data.sql` is filled from that database, as JasperReports fills a report
/// from a JDBC connection.
struct Sqlite(std::sync::Mutex<rusqlite::Connection>);

impl Connection for Sqlite {
    fn query<'a>(&'a self, sql: &str, params: &[SqlValue]) -> ferroreport::Result<SqlRows<'a>> {
        use rusqlite::types::{Value as V, ValueRef as R};
        let bad = |e: rusqlite::Error| ferroreport::Error::Data(e.to_string());
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(sql).map_err(bad)?;
        let names: Vec<String> = stmt.column_names().map(str::to_string).collect();
        let params: Vec<V> = params
            .iter()
            .map(|p| match p {
                SqlValue::Null => V::Null,
                SqlValue::Bool(b) => V::Integer(*b as i64),
                SqlValue::Int(i) => V::Integer(*i),
                SqlValue::Float(f) => V::Real(*f),
                SqlValue::Decimal(s) | SqlValue::Text(s) => V::Text(s.clone()),
                SqlValue::DateTime(d) => V::Text(d.to_string()),
                SqlValue::Bytes(b) => V::Blob(b.clone()),
            })
            .collect();
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(bad)?;
        let mut out: Vec<ferroreport::Result<SqlRow>> = Vec::new();
        while let Some(row) = rows.next().map_err(bad)? {
            let mut r = Vec::new();
            for (i, name) in names.iter().enumerate() {
                let v = match row.get_ref(i).map_err(bad)? {
                    R::Null => serde_json::Value::Null,
                    R::Integer(n) => n.into(),
                    R::Real(f) => serde_json::json!(f),
                    R::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
                    R::Blob(_) => serde_json::Value::Null,
                };
                r.push((name.clone(), v));
            }
            out.push(Ok(r));
        }
        Ok(Box::new(out.into_iter()))
    }
}

/// Registers the fonts a fixture names in `fonts.txt` (`Family=file.ttf[:bold][:italic]`, the
/// syntax of the command line's `--font`), those that are installed: a fixture whose font is
/// missing still renders, in the substitute font, with the same pages.
fn register_fonts(renderer: &mut ferroreport::Renderer, dir: &std::path::Path) {
    let Ok(list) = std::fs::read_to_string(dir.join("fonts.txt")) else { return };
    for spec in list.split_whitespace() {
        let Some((family, rest)) = spec.split_once('=') else { continue };
        let mut parts = rest.split(':');
        let path = parts.next().unwrap_or_default();
        let flags: Vec<&str> = parts.collect();
        if let Ok(bytes) = std::fs::read(path) {
            renderer.register_font(family, flags.contains(&"bold"), flags.contains(&"italic"), bytes).unwrap();
        }
    }
    // Font sets (`fontsets.txt`, a `Name=Family,Family` per line, as `--font-set`).
    let Ok(sets) = std::fs::read_to_string(dir.join("fontsets.txt")) else { return };
    for line in sets.lines().filter(|l| !l.trim().is_empty()) {
        let (name, families) = line.split_once('=').unwrap();
        let families: Vec<&str> = families.split(',').map(str::trim).collect();
        renderer.register_font_set(name, &families).unwrap();
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

#[test]
fn fixtures_render_with_expected_page_counts() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures_dir()).unwrap() {
        let dir = entry.unwrap().path();
        let expected = dir.join("expected.json");
        if !expected.exists() {
            continue;
        }
        let template = std::fs::read_to_string(dir.join("report.jrxml")).unwrap();
        let read = |name: &str, default: &str| -> serde_json::Value {
            let p = dir.join(name);
            serde_json::from_str(&if p.exists() { std::fs::read_to_string(p).unwrap() } else { default.to_string() }).unwrap()
        };
        let mut renderer = ferroreport::Renderer::new().resource_dir(&dir);
        // A fixture's JasperReports font extension, which the reference run has on its class path.
        let extension = dir.join("jasper-extension");
        if extension.is_dir() {
            renderer = renderer.resource_dir(&extension);
            let problems = renderer.load_font_extensions();
            assert!(problems.is_empty(), "{}: {problems:?}", dir.display());
        }
        register_fonts(&mut renderer, &dir);
        let report = renderer.parse(&template).unwrap();
        assert!(!report.warnings.iter().any(|w| w.contains("style template")), "{}: {:?}", dir.display(), report.warnings);
        let records = ferroreport::data::records_from_json(&report, &read("data.json", "[]")).unwrap();
        let params = ferroreport::params_from_json(&report, &read("params.json", "{}")).unwrap();
        let want = read("expected.json", "{}")["pages"].as_u64().unwrap() as usize;
        let script = dir.join("data.sql");
        if script.exists() {
            let db = rusqlite::Connection::open_in_memory().unwrap();
            db.execute_batch(&std::fs::read_to_string(script).unwrap()).unwrap();
            let mut renderer = renderer.connection(Some(std::sync::Arc::new(Sqlite(std::sync::Mutex::new(db)))));
            let mut pdf = Vec::new();
            let stats = renderer
                .render_query_to(&report, params, ferroreport::Format::Pdf, &mut pdf)
                .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
            assert_eq!(stats.pages, want, "{}", dir.display());
            assert!(pdf.starts_with(b"%PDF-"));
            checked += 1;
            continue;
        }
        let doc = renderer.fill(&report, &records, params).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        assert_eq!(doc.pages.len(), want, "{}", dir.display());
        let pdf = renderer.to_pdf(&doc).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} fixtures checked");
}

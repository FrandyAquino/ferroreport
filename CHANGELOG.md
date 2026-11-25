# Changelog

Every release of ferroreport, newest first. From 1.0.0 on, versions follow
[semantic versioning](https://semver.org); what that covers is in the README under
[Stability](README.md#stability).

## Unreleased

- ODS: per-mille patterns such as `#,##0.0‰` stay formatted text, as in JasperReports, even
  when cell type detection is enabled. This preserves `12.3‰` for `0.0123` in LibreOffice,
  which ignores the numeric display factor needed to scale it. A quoted `‰` stays literal
  and does not prevent a cell from being numeric.

## 1.2.0

PowerPoint export, charts drawn as JasperReports 7 draws them, large OpenDocument files in the
memory of a small one, fonts that fall back for missing characters, hyphenation, a designer for
people who do not program, and text shaped with harfrust. Every 1.1 template renders as before
and the Rust API keeps 1.1's shape (`cargo semver-checks` against 1.1.0).

### Fixes

- `FaceData` and `FontSet` are again `UnwindSafe` and `RefUnwindSafe`, as in 1.1.0 (the move to
  harfrust had dropped both).
- README: the PyPI package is listed as available, and the two Contributing sections are one.
- Benchmarks: ferroreport's ODS and ODT at 100,000 rows measured again now that they are written
  as they are made: 68 MiB and 38 MiB (were 246 and 264), in 5.3 s and 4.4 s (were 3.5 and 2.5;
  `docs/benchmarks-odf-streaming.json`).

### OpenDocument: large reports in the memory of a small one

- ODS and ODT no longer keep the document's body in memory until it is written. Their styles
  come before the body in the file, so a large report is now filled once more first to find
  them, and its rows are then compressed and written as they are made, like a large XLSX
  workbook. A spreadsheet is filled three times (its columns, its styles, its rows), a text
  twice; reports under about 100,000 elements are still filled once. Peak memory of
  `ferroreport render` for `fixtures/09-large`: 100,000 rows as ODS 68 MiB (was 253 MiB), as
  ODT 37 MiB (was 265 MiB); a million rows as ODT 37 MiB (was 1,672 MiB).
- This needs rows that can be read again, as for XLSX: a data file, the rows the command line
  and the server put aside (now also for `.odt` from standard input or sent one per line),
  Python, and `Renderer::render_rows_to` in Rust, which now also reads them twice for ODT.
  `Renderer::render_to`, `OdsStream::new` and `OdtStream::new` keep the body in memory as
  before.
- Small documents are the same bytes as in 1.1.0. A large one has the same files inside; only
  `content.xml` is stored with its sizes after it, as the XLSX sheet already is.
- If the rows read the second time are not the ones read the first (a table that changed
  between the queries) and need styles the first pass did not see, those styles go to
  `styles.xml`, so the file still opens; LibreOffice then shows their rows at its default
  height.

### PowerPoint: PPTX

- `-o report.pptx` or `--format pptx` on the command line, `"format": "pptx"` in the HTTP API
  (answered as `application/vnd.openxmlformats-officedocument.presentationml.presentation`),
  `Format::Pptx` in Rust, `format="pptx"` in Python, `{ format: "pptx" }` in JavaScript,
  `Format.PPTX` in Java, and "Report as PowerPoint" in the designer's Save menu.
- Laid out like JasperReports' PPTX exporter: each page is a slide of the report's page size
  and each element a shape where it is on the page. Texts are text boxes with their fonts,
  sizes, bold, italic, underline, strike-through, colors, highlight, superscript and
  subscript, alignment, padding, background, line spacing and rotation; rectangles (rounded
  too), ellipses and lines are shapes with their fill and pen; frames are rectangles under
  their elements; images are pictures (each stored once) cut to their element as in the PDF;
  charts and barcodes are drawn as pictures.
- Where Jasper's exporter leaves things out: borders that differ by side are drawn as lines,
  and links to a page or an anchor of the report jump to its slide (links to web pages and
  other files work in both).
- Slides and pictures are written as the pages are filled, so a long report does not stay in
  memory. A report without pages is a presentation without slides.
- The `.pptx` of 54 of the 55 fixtures opens in LibreOffice Impress with a slide for each page
  of its PDF; 09-large (477 slides) needs more memory than the machine it was checked on had,
  and was checked from its first 2,000 rows. Against the `.pptx` JasperReports writes for 53
  fixtures: the same slides, and all but one of 88,506 text boxes at the same place and size
  with the same font size.
- Not yet: `net.sf.jasperreports.export.pptx.*` properties and embedded fonts.
- For contributors: `tools/oracle` writes JasperReports' own `.pptx` (`out.pptx`).

### Benchmarks of 1.1.0

- Every published number is measured again with 1.1.0, against JasperReports 6.20.6 and 7.1.1
  on the same 2-CPU machine (`docs/benchmarks.json`, `docs/benchmarks-7.1.1.json`,
  `docs/benchmarks-concurrent.json`, the README, the site). Some got worse and say so: an
  invoice from a cold start is now about 200× faster than JasperReports, not 250× (JasperReports
  starts faster on this machine than in the 0.12 runs), against a warm JVM 3 to 5× faster, not
  3 to 6×, and a million rows take 16 MB of memory, not 13 MB (the engine itself is larger), in 12 s
  instead of 14 s.
- `tools/bench_all.py` also measures a right-to-left report (`52-arabic`), the 20,000-row report
  to each format (PDF, XLSX, DOCX, ODS, ODT, CSV) for both engines, and 100,000 rows to XLSX, ODS
  and ODT, so the cost of each output is published: ODS and ODT keep the document in memory and
  take the most (about 250 MiB for 100,000 rows).
- `--wait-idle` in `tools/bench_all.py` and `tools/bench_concurrent.py`, for a machine that other
  work shares: each run starts on an idle machine, and a run that other processes took CPU time
  from is done again (`tools/idle.py`). The results record the CPU, the memory, the kernel, the
  JVM flags from the environment and how busy the machine was.
- `tools/http_check.py` again (`docs/http-check.json`): a million rows over HTTP now peak at
  47 to 49 MiB in the server, so the docs say about 50 MB instead of 45 MB.
- The race on the website, its GIF and the promo video use the new invoice timings (2.63 s
  against 11 ms: 234 invoices in the time JasperReports takes for one, not 301), and the
  playground's JasperReports times are measured again.

### Text shaping with harfrust

- Right-to-left and complex text (Arabic, Hebrew, Devanagari, Thai...) is shaped with harfrust
  0.16, the HarfBuzz project's maintained Rust port, instead of rustybuzz, which is no longer
  maintained (RUSTSEC-2026-0206). Output is unchanged: every fixture renders to the same bytes
  in every format, and the text `pdftotext` extracts is the same. Each font's shaping tables
  are still read once and kept with the font, and shape plans are still made once per
  direction and script.
- Speed: shaping takes fewer instructions (2,000 rows of the Arabic fixture: 11% fewer, of the
  Devanagari one: 18% fewer, of the Thai one: 2% more). harfrust reads a font's old-style
  `kern` table the first time it shapes with it, even with kerning off: for FreeSerif (the
  Thai and Devanagari fixtures) that is about 10 million instructions, a few milliseconds,
  once per process. Latin text is not shaped and takes the same time as before (09-large).
- Dependencies: harfrust (MIT), with read-fonts and font-types (MIT or Apache-2.0), replaces
  rustybuzz and self_cell. resvg still uses rustybuzz to lay out text in SVG images, so it stays
  in the build; `deny.toml` no longer needs to ignore its advisory.

### Font fallback and hyphenation

- Hyphenation of long words at line ends, when the template asks for it with ferroreport's own
  report property `ferroreport.text.hyphenate` (JasperReports does not hyphenate, and ignores
  it): `true` hyphenates every plain text of the report in the language of the render's locale,
  `es`, `en_US`... in that language, `false` not at all. Tables, lists and subreports without a
  setting of their own follow the report they are in. The property is also a parameter of the
  same name, so a render can set it (`-p params.json`, the packages, the HTTP API) without
  touching the template. On an element it is not read (a warning says so). A word that does not
  fit is cut after the last syllable that fits with a hyphen, which is drawn. Plain text only;
  the height of stretching texts and page splits follow the hyphenated lines. Patterns for
  English, Spanish, French, Italian, Portuguese and Catalan are built in (hypher, MIT or
  Apache-2.0, no dependencies; the command line grows by about 60 KB); the Cargo feature
  `hyphenation-all` builds in all 48 languages of hypher, and `default-features = false`
  leaves hyphenation out. Templates without the property render as before.
- PDF: a character the text's font does not have is left out, as JasperReports leaves it out,
  instead of being drawn as the font's missing-glyph box. It takes no width when the lines are
  drawn and aligned, and still counts as wide as that box for the text's height and where it is
  cut, as in JasperReports (checked against 6.20.6, identical pages). Text shaped with
  harfrust (right to left, complex scripts) still shows the box.
- Font sets, as the `<fontSet>` of a JasperReports font extension defines them:
  `--font-set "Serif Fallback=DejaVu Serif,DejaVu Sans"` (or `Renderer::register_font_set`)
  makes a text whose font name is `Serif Fallback` draw the characters DejaVu Serif lacks in
  DejaVu Sans. The text is cut into runs as JasperReports cuts it: the longest run one family
  has, in the first family that has all of it; a character no family has stays in the first
  family. Measured, broken into lines and embedded in the PDF with the font of each run;
  checked against JasperReports 6.20.6 with a font extension that defines the same sets
  (fixture 56-font-fallback, identical PDF, XLSX and CSV).
- `--font-fallback` (`Renderer::font_fallback`) does the same for every font, without a font
  set: characters a text's font lacks are drawn in the first of the `--font` families (in the
  order given), DejaVu Sans, DejaVu Serif and DejaVu Sans Mono that has them. Off by default,
  as JasperReports does not do it; texts whose characters are all in their font are drawn as
  before either way.

### Designer

- The designer is easier for people who do not program: text and fields are written as
  `Invoice {number} · {customer}` (type `{` to choose a field) and fields show on the page as
  pills, formulas stay under Advanced untouched; the bands have plain names ("Each row of your
  data") and empty ones are slim strips to drop on; the page fills the width, with the PDF in a
  Preview tab and Download PDF (or Excel, Word, PowerPoint) as the main button; the properties are in
  sections; problems are said in plain words, with the engine's message under "Technical
  details"; a three-step guide on the first visit; English and Spanish. Its browser tests run
  in CI.

### Charts as JasperReports 7 draws them

Charts follow JFreeChart 1.5's layout rules as JasperReports 7.1 uses them, measured against
the charts of JasperReports' own samples, instead of an approximation of its look.
Of JasperReports 7.1.1's 24 chart samples, 20 now come out pixel for pixel the same (none did
before), which takes the 7.1.1 samples that match from 47 to 67 of 101. Still different: the
bar chart sample with a chart customizer, meter and thermometer charts, and one page of the
time period bar chart. JasperReports 6.20 draws charts with JFreeChart 1.0.19 (framed legend,
3D charts in 3D), so its chart samples still differ.

- Bar, stacked bar, line, area and stacked area charts (and their 3D variants, which
  JasperReports 7 draws flat) come out the same as JasperReports draws them: plot insets, the
  room the axes take, tick units and tick labels, category labels shortened with `...` the way
  JFreeChart shortens them, legend items and spacing (no frame around the legend), series
  colors, shapes and grid lines.
- Pie charts come out the same as JasperReports draws them: the pie's size and place, its
  shadow, section outlines, the labels' boxes, shadows and link lines, and how labels are
  moved apart when they would overlap. 3D pie charts are drawn flat with their
  `foregroundAlpha`, as JasperReports 7 draws them.
- XY line, XY area, scatter, time series, XY bar, bubble, candlestick and high-low charts come
  out the same as JasperReports draws them: number and date axes (JFreeChart's date tick
  units, from milliseconds to years, and their formats), the room the range axis takes
  decided before the domain axis's, series drawn last to first, shapes, bars, bubbles sized on
  the range axis, candlestick bodies, wicks and volume bars. High-low charts draw open and
  close ticks only when `isShowOpenTicks` and `isShowCloseTicks` ask for them, as in
  JasperReports.
- Gantt charts come out the same as JasperReports draws them: tasks down the left, the date
  axis on top with JFreeChart's date ticks, bars placed as BarRenderer places them, and the
  done and not-done parts of each task or subtask as a band through its middle.
- Multiple axis charts made of bar and line charts come out the same as JasperReports draws
  them: every chart's value axis beside the plot in the order given, each in its own color and
  format, the grid lines of the first one, the last chart drawn first, and shapes handed out
  chart after chart.
- Where a chart drawn as SVG is translucent (3D pie sections with a `foregroundAlpha` below 1,
  candlestick volume bars), the PDF gets the same opaque bitmaps JasperReports writes there,
  pixel for pixel, instead of translucent vector shapes.
- Text in charts is measured as JFreeChart measures it (AWT's whole-point font metrics) and
  written as glyph outlines, as JasperReports writes it: chart text is no longer selectable
  in the PDF.
- `net.sf.jasperreports.chart.render.type=svg` (on the chart, in the report's properties or
  in a `jasperreports.properties` among the resource folders) measures text the way
  JasperReports does when it draws charts as SVG.
- What a chart draws inside its plot is clipped to the plot, as in JasperReports.
- The value axis of horizontal bar charts is above the plot, and a series color given alone
  replaces only the first default color, as in JasperReports.
- `tools/samples.py` runs JasperReports in the UTC time zone, like the rows' dates, so samples
  that read a date's month or day (the time series ones) group their rows the same on any
  machine.

## 1.1.0

### Short forms in `.ferro`

For people who do not come from JasperReports. Each one compiles to the same JRXML as its long
form, the long forms keep working, and every 1.0 template compiles as it did (checked against
the JRXML 1.0.2 made of the examples, the documentation and every fixture).

- `text` is one part for both kinds of text: `text : Overdue` is a static text, and
  `text : Invoice {number} · {customer}` is a text field that prints
  `"Invoice " + $F{number} + " · " + $F{customer}`. `{total}` alone is the value itself, so
  `pattern=` formats it, and a text that starts with `=` is an expression
  (`text : ={qty} * {price}`). `\{`, `\}`, `\$`, `\=` and `\\` print the character itself.
- `{name}` means `$F{name}` in any expression (`if : {overdue}`, `each : {lines}`,
  `variable total Double calculation=Sum : {amount}`). Strings, comments and the places where
  Java writes a brace are left alone, and so is every brace in a report with
  `language=groovy` or `javascript`.
- Fields written as `{name}` need no `field` line: they are declared where they are used (the
  report, or the `each` they are in) with class `Object`, so their values keep the type the
  data gives them. Declare a field to give it a class, such as `Date` for dates.
- Attributes with plain names, on parts and styles: `color=` (forecolor), `background=`
  (backcolor, and `mode=Opaque` so it shows), `align=left|center|right|justify` and
  `valign=top|middle|bottom` (text alignment, or image alignment on an image), and `size=`
  (font size). Colors may be named as JasperReports names them (`red`, `gray`, `orange`...)
  or written in hex. When a line also gives the JRXML name, the JRXML name is used.
- `size=` on a text or a style is now written to JRXML as `fontSize=`. ferroreport always read
  it as the font size, so nothing it renders changes; JasperReports now reads it too.
- `ferroreport convert` still writes the long forms.
- `examples/invoices-short.ferro` is `examples/invoices.ferro` in the short forms, and prints
  the same PDF.
- VS Code extension: highlighting for `text`, `{name}`, escapes, color names and alignments,
  and snippets for `text`, `text=` (an expression) and `field`; the other snippets use the
  short forms.

### OpenDocument: ODS and ODT

- New output formats: OpenDocument spreadsheet (`.ods`) and text (`.odt`), for LibreOffice and
  other OpenDocument applications. Pick them with the extension of `-o`, `--format ods|odt`,
  `"format": "ods"` in the HTTP API, `Format::Ods` / `Format::Odt` in Rust, `format="ods"` in
  Python, `{ format: "odt" }` in JavaScript and `Format.ODS` / `Format.ODT` in Java.
- ODS is laid out like the XLSX export (and JasperReports' ODS exporter): every page on one
  sheet, cells cut at element edges, merged cells, fonts, colors, borders, padding, alignment,
  rotation, column widths and row heights, images over their cells, charts and barcodes as
  pictures, anchors as named ranges. `--xlsx-numbers` (`XlsxOptions::detect_cell_type`) now
  also writes ODS numbers as numbers, with a number style made from their Java pattern.
- ODT is laid out like the DOCX export (and JasperReports' ODT exporter): a table per page,
  a page break between pages, texts with their fonts and styled runs, cell borders and
  backgrounds, pictures, links and bookmarks. Ellipses and slanted lines are drawn as
  pictures.
- Both are valid packages (`mimetype` stored first, manifest, `content.xml`, `styles.xml`) with
  the report's page size. The 50 fixtures not filled from a database open in LibreOffice
  with as many pages as their PDF.
- Not yet: `net.sf.jasperreports.export.ods.*` / `.odt.*` properties, dates as date cells,
  per-mille number styles, upside-down text in ODT. Both formats keep the document's body in
  memory until it is written.
- `tools/oracle` can also write JasperReports' own `.ods` and `.odt`, for comparison.

### A visual designer

- **A visual designer in the browser**, the first version: <https://cristianencarnacion.github.io/ferroreport/designer/>.
  Paste or upload a sample of your data (JSON) and its fields are listed with their types; drag
  them onto the report's bands, or add text, lines, rectangles, ellipses, images, the page
  number and today's date. Select, move and resize with the mouse or the keyboard, on a grid;
  change text, font, colors, alignment, borders and number or date format in a properties
  panel; undo and redo. The PDF is made again beside the page as you edit, by the WebAssembly
  engine, so nothing is uploaded. It opens `.ferro` and `.jrxml`, saves both, and downloads the
  report as PDF, Excel or Word. Parts it cannot edit yet (charts, tables, crosstabs, subreports,
  barcodes) are shown and kept as they are.
- JavaScript: `toTree(template, options?)` gives any template (JRXML 6 or 7, or `.ferro`, with
  the files it imports in `options.resources`) as a tree of its JRXML 7 form, and
  `fromTree(tree, "jrxml" | "ferro")` writes one back. Rust: `ferroreport::design::to_tree` and
  `from_tree`.
- The website is assembled by `tools/assemble-site.sh`, for the Pages workflow and to try it
  locally; `bindings/wasm/build.sh` honours `CARGO_TARGET_DIR`.

### Right-to-left and complex scripts

- Arabic and Hebrew run right to left. Lines are ordered with the Unicode Bidirectional
  Algorithm, so numbers, Latin words and parentheses inside them stay in reading order, and
  Arabic letters join. A text whose paragraph starts with a right-to-left letter runs right to
  left as a whole, as JasperReports decides it; Left, Right and Center stay where they are, and
  the last line of a justified paragraph sits on the right. The two new fixtures (52-arabic,
  53-hebrew-mixed, styled text included) match JasperReports 6.20.6 pixel for pixel, with the
  same XLSX and CSV.
- Devanagari, Thai and the other complex scripts are shaped with rustybuzz: conjuncts, reph,
  vowel signs drawn before their consonant, stacked Thai marks, and the font's own mark
  positioning. They need a font that has them (`--font`); the bundled DejaVu has Arabic and
  Hebrew but not these. JasperReports 6.20's PDF exporter does not shape them, so their
  fixtures (54-devanagari, 55-thai) are compared by page count.
- Lines break at the Unicode line-break opportunities, measured with the shaped widths. Thai,
  Lao, Khmer and Myanmar have no spaces between words and no dictionary here: lines break
  before a leading vowel, after a syllable-final sign and at spaces, and never inside a cluster.
- PDF: the glyphs used after shaping are embedded and subsetted, and copying gives the
  characters as written: each glyph maps to the characters of its cluster (a ligature to all of
  them), and left-to-right runs whose glyphs are reordered (Devanagari) carry their text as
  `/ActualText`. `pdftotext` gives back the words of all four fixtures as written; in lines
  that mix directions it orders numbers and punctuation itself, as it does for JasperReports'
  PDFs.
- HTML and the chart pictures in XLSX and DOCX draw the shaped lines; HTML marks right-to-left
  lines `direction="rtl"`. Word documents mark right-to-left paragraphs `<w:bidi/>` (with Left
  and Right kept on the same side) and give complex-script runs their bold, italic and size
  (`bCs`, `iCs`, `szCs`); this goes beyond JasperReports' DOCX exporter, which writes neither.
  XLSX and CSV hold the text as written, as before.
- Chart titles, labels and legends with right-to-left or complex text are shaped too.
- Text in Latin, Greek, Cyrillic and the other simple scripts takes the same path as before:
  every existing fixture renders to the same bytes in every format.
- Fixtures can name the fonts they need in `fonts.txt` (`Family=file.ttf[:bold]`, the syntax
  of `--font`); `cargo test`, `tools/compare.py`, `tools/compare_sheets.py`, `tools/pdfua.sh`
  and `tools/pdfa.sh` register those that are installed. The PDF/UA workflow installs
  `fonts-freefont-ttf` for them.
- New dependencies: rustybuzz, unicode-bidi, unicode-script, unicode-linebreak and self_cell
  (which keeps each font's shaping tables next to its bytes, read once). The first three were
  already in the build through resvg.
- Speed: each font's shaping tables and shape plans are read once and kept, and a one-line
  text is shaped once for filling, exporting and drawing. 20,000 rows of the Arabic fixture take about
  0.6 s, of the Devanagari one about 0.7 s. Latin text pays one check per text for characters
  that need shaping (09-large: 2% more instructions).
- API: `print::PrintLine` has a new field, `shaped` (the line's glyphs when it is shaped).
  Code that builds a `PrintLine` itself no longer compiles; code that reads one is not
  affected. This breaks the letter of the 1.x promise for a type that is only meant to be read,
  so `PrintLine` is now `#[non_exhaustive]`: fields added later will not break anyone. The new `shape` module says whether a text needs shaping (`needs_shaping`), whether
  it runs right to left (`is_rtl`), and shapes a line (`line`).
- Still to do: hyphenation, fallback to another font for characters the chosen one lacks, and
  Arabic and Hebrew vowel marks that the font moves up or sideways can come out of `pdftotext`
  apart from their letter.

## 1.0.2

- VS Code extension: an icon, a README with an animation and a screenshot, and section names
  (`title`, `detail`, `row`...) now colored by every theme.
- No change to the engine: what 1.0.1 renders, 1.0.2 renders the same.

## 1.0.1

- The VS Code extension for `.ferro` is on the Visual Studio Marketplace
  (`cristianencarnacion.ferroreport`), published from the release workflow without a token.
- npm and PyPI packages are published through trusted publishing, without tokens.
- No change to the engine: what 1.0.0 renders, 1.0.1 renders the same.

## 1.0.0

The first stable release: the APIs, the command line, the HTTP API and the `.ferro` format are
now kept compatible within 1.x. Since 0.12:

### Data from SQL databases

- `render --db <url>` and `serve --db <url>` run the template's own SQL query, as JasperReports
  runs it over JDBC: `$P{}` as a bound parameter, `$P!{}` pasted into the text, `$X{IN}`,
  `$X{NOTIN}`, `$X{EQUAL}`, `$X{BETWEEN}` and the other clause functions. PostgreSQL,
  MySQL and MariaDB, SQL Server and SQLite; the URL can be the JDBC one the project already
  has. Rows stream from the database as pages are written.
- Subreports run their own queries on `$P{REPORT_CONNECTION}`.
- TLS follows libpq's `sslmode` (`disable`, `prefer`, `require`, `verify-ca`, `verify-full`,
  `sslrootcert`).
- The server gives the connection only to the templates of `--templates`, and a request cannot
  set a parameter that is pasted into the SQL (`$P!{}`).

### Expressions

- JasperReports' function library as Jaspersoft Studio lists it: 78 functions (`IF`, `SUM`,
  `ROUND_HALF_UP`, `DATEFORMAT`, `WORKDAY`, `NETWORKDAYS`, `WEEKNUM`...), checked on 265
  cases against JasperReports.
- `DATERANGE`, with relative ranges (`"MONTH-1"`) and fixed ones, `getStart()`/`getEnd()`,
  and in `$X{}` clauses.
- Groovy: lists, maps and ranges, closures (`findAll`, `collect`, `inject`, `sort`,
  `groupBy`...), `"${...}"` strings, `?.`, `**`, `<=>`, `in`, `as`, indexing, string
  helpers; checked on 128 cases against JasperReports. BigDecimal division keeps Groovy's scale.
- An expression that does not parse is reported with its line and column in the template.

### PDF signatures

- PAdES baseline B-B signatures: `--sign-key` and `--sign-cert` (PEM: RSA or ECDSA P-256) or a
  PKCS#12 file with `FERROREPORT_SIGN_PASSWORD`; `--sign-name`, `--sign-reason`,
  `--sign-location`, `--sign-contact`. `serve` takes the same options and signs every PDF it
  returns.
- Signed PDF/A-2b passes veraPDF on every fixture, and the signatures are verified with
  poppler's `pdfsig` and with pyHanko on every push.

### `.ferro` templates

- `import "parts.ferro"` shares definitions between templates (only from the template's own
  folder).
- `if : <expression>` prints its parts only when the expression is true, and leaves no gap when
  it is not.
- `each : <expression>` repeats its parts for every item of a list in the row.
- `render --watch` (`-w`) renders again whenever the template or its data change.
- A VS Code extension for `.ferro`: highlighting, snippets, comments and brackets
  (`editors/vscode`, attached to every release as a `.vsix`).

### Barcodes and images

- Every Barbecue type but PostNet: USPS, SCC-14, GTIN, shipment number, UCC-128 with
  `applicationIdentifierExpression`, random-weight UPC-A, Bookland, Code 39 extended and
  PDF417, matched bar for bar on 115 symbols Barbecue drew. PostNet is skipped with a warning
  (Barbecue draws it as bars of one height; barcode4j's POSTNET component is drawn).
- The default height of Barbecue's Code 128 family now matches Barbecue for long texts too.
- `scaleImage="RealHeight"` and `"RealSize"`: the element takes its height (and width) from the
  picture, as in JasperReports, and moves to the next page when it does not fit.

### Fixes

- A date with no pattern prints in the locale's short date and time, as JasperReports does.
- `WEEKDAY` counts from Monday unless asked otherwise.
- Control characters take no room and draw nothing; in Excel, C1 control characters are written
  as `?`, as JasperReports writes them.
- A report without pages exports the XLSX file JasperReports writes for it.
- Fuzzing found and fixed: an overflow in decimal arithmetic, a panic in sorting with an
  inconsistent comparison, and two inputs that took too long (`BASE`, `padLeft`).
- The minimum Rust version of the library is 1.86 (the 1.80 declared before was wrong); the
  command line needs 1.89.

### For contributors

- Fuzz targets for templates, expressions, rows and SQL queries (`fuzz/`), run every week in
  CI, with the files OSS-Fuzz needs.
- Fixtures 48 to 51 (SQL, the function library, the other Barbecue types, RealHeight and
  RealSize): 48 of the 51 render pixel for pixel like JasperReports (2 charts are close, the
  SQL one is checked from the same rows as JSON), and 49 export the same Excel and CSV.

### Known differences

- Charts follow JFreeChart's look closely but are not pixel-identical.
- Text is set left to right without shaping: right-to-left and complex scripts are not right
  yet, and there is no hyphenation.
- A PNG with a resolution other than 72 dpi in a `RealSize` element is drawn a point wider
  than JasperReports 7.1.1 draws it (as JasperReports 6.20.6 draws it).
- Not yet: scriptlets and other Java code, maps, ODS/ODT/PPTX, signatures with timestamps.

## 0.12.0 and earlier

The notes of every earlier release are on
[GitHub Releases](https://github.com/cristianEncarnacion/ferroreport/releases):

- **0.12.0:** PDF/A-2 and PDF/A-3 with attachments, AES-256 encryption, links, anchors and
  bookmarks in every format, Barbecue barcodes, `.ferro` without coordinates, `ferroreport
  migrate`, a limit on the size of a row.
- **0.11.0:** the `.ferro` template format and `ferroreport convert`, Word (DOCX) output,
  PDF417, SVG images, tagged PDFs checked against PDF/UA-1, JRXML 7.
- **0.10.0:** the nine missing chart types, RTF markup, HTML lists, more barcodes, PDF form
  fields, icon labels; checked against JasperReports 7.1.1's samples too.
- **0.9.0 and before:** the engine, Excel and CSV export, every chart type, subreports, tables,
  lists, crosstabs, streaming of large reports, HTML previews, the Java, Python, JavaScript and
  HTTP interfaces.

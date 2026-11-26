//! In-memory model of a report template, with styles already resolved onto each element.
use std::sync::Arc;

use crate::expr::Expr;
use crate::value::JavaType;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Language {
    Java,
    Groovy,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhenNoData {
    NoPages,
    BlankPage,
    AllSectionsNoDetail,
    NoDataSection,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub class: JavaType,
    pub default: Option<Expr>,
}

/// A dataset's query (`<queryString>` in JRXML 6, `<query>` in 7), run on a database
/// connection when one is given (see [`crate::sql`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    /// `SQL` (or empty, which means SQL).
    pub language: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub class: JavaType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Calculation {
    Nothing,
    /// The value is only set from outside (a subreport return value or a scriptlet).
    System,
    Count,
    DistinctCount,
    Sum,
    Average,
    Lowest,
    Highest,
    First,
    StandardDeviation,
    Variance,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResetType {
    None,
    Report,
    Page,
    Column,
    Group(String),
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub class: JavaType,
    pub calculation: Calculation,
    pub reset: ResetType,
    pub increment: ResetType,
    pub expression: Option<Expr>,
    pub initial: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub expression: Option<Expr>,
    pub start_new_page: bool,
    pub start_new_column: bool,
    pub reprint_header_on_each_page: bool,
    pub min_height_to_start_new_page: f32,
    pub header: Vec<Band>,
    pub footer: Vec<Band>,
}

/// How a band may break across pages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitType {
    /// Never split within the declared height, but split when content stretches (default).
    Stretch,
    /// Move the whole band to the next page instead of splitting it.
    Prevent,
    /// Split anywhere.
    Immediate,
}

#[derive(Debug, Clone)]
pub struct Band {
    pub height: f32,
    pub split_type: SplitType,
    pub print_when: Option<Expr>,
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionType {
    Fix,
    Float,
    FixRelativeToTop,
    FixRelativeToBottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StretchType {
    NoStretch,
    RelativeToBandHeight,
    RelativeToTallestObject,
    ContainerHeight,
    ContainerBottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Opaque,
    Transparent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
    Double,
}

/// A stroke. `width == 0` means no stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pen {
    pub width: f32,
    pub color: Color,
    pub style: LineStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Padding {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

/// Borders and padding of a text element or frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxStyle {
    pub padding: Padding,
    pub top: Pen,
    pub left: Pen,
    pub bottom: Pen,
    pub right: Pen,
}

impl BoxStyle {
    /// No padding and no borders (pens of zero width in `color`).
    pub fn plain(color: Color) -> BoxStyle {
        let pen = Pen { width: 0.0, color, style: LineStyle::Solid };
        BoxStyle { padding: Padding::default(), top: pen, left: pen, bottom: pen, right: pen }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HAlign {
    Left,
    Center,
    Right,
    Justified,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpacing {
    Single,
    OneAndHalf,
    Double,
    Proportional(f32),
    AtLeast(f32),
    Fixed(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    pub name: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// The PDF font JasperReports writes this text in when the family is not one it has as a
    /// font extension (`pdfFontName`; Helvetica when not given).
    pub pdf_font: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub font: FontSpec,
    pub halign: HAlign,
    pub valign: VAlign,
    pub line_spacing: LineSpacing,
    pub rotation: Rotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    /// 90° counter-clockwise: reads bottom to top.
    Left,
    /// 90° clockwise: reads top to bottom.
    Right,
    UpsideDown,
}

/// The `evaluation_group` of a text field with `evaluationTime="Master"`, which is read as
/// [`EvaluationTime::Report`]: it is resolved when the report ends, with `MASTER_CURRENT_PAGE`
/// (the number of the page it is on) and `MASTER_TOTAL_PAGES` set.
pub const MASTER_EVALUATION: &str = "\u{0}Master";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EvaluationTime {
    Now,
    Report,
    Page,
    Column,
    Group,
    Band,
    Auto,
}

#[derive(Debug, Clone)]
pub enum TextContent {
    Static(String),
    Expression {
        expr: Option<Expr>,
        pattern: Option<String>,
        pattern_expr: Option<Expr>,
        blank_when_null: bool,
        evaluation_time: EvaluationTime,
        evaluation_group: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct TextElement {
    pub content: TextContent,
    pub style: TextStyle,
    pub boxed: BoxStyle,
    pub stretch: bool,
    pub markup: crate::rich::Markup,
    pub truncate: Truncate,
    /// Written as a PDF form field instead of text (`net.sf.jasperreports.export.pdf.field.*`).
    pub field: Option<Box<FormField>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Combo,
    List,
    Check,
    Radio,
}

/// A PDF form field (JasperReports' `net.sf.jasperreports.export.pdf.field.*` properties).
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    pub kind: FieldKind,
    pub name: Option<String>,
    /// The value given by property (otherwise the element's text is the value).
    pub value: Option<String>,
    pub choices: Vec<String>,
    pub checked: bool,
    /// The ZapfDingbats character of a check box's mark.
    pub check_char: char,
    pub read_only: bool,
    pub multiline: bool,
    pub editable: bool,
    /// The border style: S (solid), B (beveled), I (inset), U (underline), D (dashed).
    pub border: char,
}

/// How a text that does not fit its box is cut (`net.sf.jasperreports.text.truncate.*` and
/// `net.sf.jasperreports.print.keep.full.text`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Truncate {
    /// Cut at the last character that fits, not the last word.
    pub at_char: bool,
    /// Appended where the text is cut (an ellipsis, usually).
    pub suffix: Option<String>,
    /// Spreadsheet and CSV output get the whole text.
    pub keep_full: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineDirection {
    TopDown,
    BottomUp,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScaleImage {
    Clip,
    FillFrame,
    RetainShape,
}

/// The sizes an image element takes from its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealImage {
    Height,
    Size,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnErrorType {
    Error,
    Blank,
    Icon,
}

#[derive(Debug, Clone)]
pub struct ImageElement {
    pub expr: Option<Expr>,
    /// `scaleImage="RealHeight"` or `"RealSize"`: the element takes the height (and with
    /// `RealSize` the width) of the picture, which is then drawn as `RetainShape`.
    pub real: Option<RealImage>,
    /// `hyperlinkTooltipExpression`: used as the image's description in tagged PDFs.
    pub tooltip: Option<Expr>,
    pub scale: ScaleImage,
    pub halign: HAlign,
    pub valign: VAlign,
    pub boxed: BoxStyle,
    pub on_error: OnErrorType,
}

#[derive(Debug, Clone)]
pub enum ElementKind {
    Text(TextElement),
    Line {
        direction: LineDirection,
        pen: Pen,
    },
    Rectangle {
        radius: f32,
        pen: Pen,
    },
    Ellipse {
        pen: Pen,
    },
    Frame {
        children: Vec<Element>,
        boxed: BoxStyle,
    },
    Image(ImageElement),
    Subreport(Box<SubreportElement>),
    /// A table component (`jr:table`), filled like a subreport over a subdataset.
    Table(Box<TableElement>),
    /// A list component (`jr:list`), filled like a subreport over a subdataset.
    List(Box<ListElement>),
    /// A crosstab (`<crosstab>`).
    Crosstab(Box<CrosstabElement>),
    /// The rest of a crosstab continuing on a new page.
    CrosstabRest(crate::crosstab::Rest),
    Barcode(Box<BarcodeElement>),
    Chart(Box<crate::chart::ChartElement>),
    /// Already filled subreport content still to be printed (the continuation of a subreport
    /// split across pages).
    /// The rest of a subreport, table or list continuing on a new page; `framed` for tables and
    /// lists, which JasperReports prints in a frame.
    Content {
        segments: Arc<Vec<crate::print::Segment>>,
        framed: bool,
    },
    /// A page break (`<break>`).
    Break,
    /// A column break (`<break type="Column">`).
    ColumnBreak,
    /// An element this engine cannot render yet; it is skipped and reported by `doctor`.
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct Element {
    pub key: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub position_type: PositionType,
    pub stretch_type: StretchType,
    pub mode: Mode,
    pub forecolor: Color,
    pub backcolor: Color,
    pub print_when: Option<Expr>,
    pub remove_line_when_blank: bool,
    /// Printed again on the continuation of a band split across pages.
    pub print_when_detail_overflows: bool,
    /// `false`: printed only when its value differs from the last time it was printed.
    pub print_repeated_values: bool,
    /// With repeated values suppressed, print anyway in the first band of each page.
    pub print_in_first_whole_band: bool,
    /// With repeated values suppressed, print anyway when this group changes.
    pub print_when_group_changes: Option<String>,
    /// Style variants selected by conditional-style expressions.
    pub conditional: Option<Box<ConditionalStyles>>,
    /// What the element is in the structure of a tagged PDF, when it is more than a text or
    /// a picture.
    pub tag: Option<Tag>,
    /// Where the element leads when clicked, the anchor it is and its bookmark, if any.
    pub link: Option<Box<Hyperlink>>,
    pub kind: ElementKind,
}

/// What a text field, image or chart links to (`hyperlinkType`), the anchor it sets
/// (`anchorNameExpression`) and the level of its bookmark (`bookmarkLevel`).
#[derive(Debug, Clone, Default)]
pub struct Hyperlink {
    pub kind: LinkKind,
    /// `hyperlinkReferenceExpression`: a URL, or the file of a remote anchor or page.
    pub reference: Option<Expr>,
    /// `hyperlinkAnchorExpression`.
    pub anchor: Option<Expr>,
    /// `hyperlinkPageExpression`: a page number, from 1.
    pub page: Option<Expr>,
    /// `hyperlinkTooltipExpression`.
    pub tooltip: Option<Expr>,
    /// `anchorNameExpression`: the anchor the element is, for links and bookmarks.
    pub anchor_name: Option<Expr>,
    /// `bookmarkLevel` (0: no bookmark), or `bookmarkLevelExpression`.
    pub bookmark_level: u32,
    pub bookmark_level_expr: Option<Expr>,
}

/// Where a link leads (`hyperlinkType`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum LinkKind {
    #[default]
    None,
    /// A URL.
    Reference,
    /// An anchor of this document.
    LocalAnchor,
    /// A page of this document.
    LocalPage,
    /// An anchor of another document.
    RemoteAnchor,
    /// A page of another document.
    RemotePage,
}

/// What an element is in the structure of a tagged PDF: set with the property
/// `net.sf.jasperreports.export.accessibility.tag` (`h1` to `h6`, `table`, `th`, `td`), and
/// by tables and crosstabs for their own cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tag {
    /// A heading of this level (1 to 6).
    Heading(u8),
    Table,
    /// A cell that names a column.
    HeaderCell,
    /// A cell that names a row.
    RowHeaderCell,
    DataCell,
}

impl Tag {
    /// The tag a property value names.
    pub fn parse(value: &str) -> Option<Tag> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "h1" => Tag::Heading(1),
            "h2" => Tag::Heading(2),
            "h3" => Tag::Heading(3),
            "h4" => Tag::Heading(4),
            "h5" => Tag::Heading(5),
            "h6" => Tag::Heading(6),
            "table" => Tag::Table,
            "th" | "columnheader" => Tag::HeaderCell,
            "rowheader" => Tag::RowHeaderCell,
            "td" => Tag::DataCell,
            _ => return None,
        })
    }
}

/// The style-dependent parts of an element.
#[derive(Debug, Clone)]
pub struct StyledParts {
    pub mode: Mode,
    pub forecolor: Color,
    pub backcolor: Color,
    pub kind: ElementKind,
}

/// Conditional styles of an element: `variants[mask]` is the element styled with the
/// conditional styles whose bits are set in `mask` (bit i = `conditions[i]` is true).
#[derive(Debug, Clone)]
pub struct ConditionalStyles {
    pub conditions: Vec<Expr>,
    pub variants: Vec<StyledParts>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WhenResourceMissing {
    #[default]
    Null,
    Empty,
    Key,
    Error,
}

/// A parsed, style-resolved report template.
#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    /// Base name of the `.properties` files with the texts of `$R{key}` (`resourceBundle`).
    pub resource_bundle: Option<String>,
    /// What `$R{key}` gives when there is no such key (`whenResourceMissingType`).
    pub when_resource_missing: WhenResourceMissing,
    pub language: Language,
    pub page_width: f32,
    pub page_height: f32,
    /// `orientation="Landscape"` (spreadsheet page setup).
    pub landscape: bool,
    pub column_width: f32,
    pub column_count: u32,
    pub column_spacing: f32,
    pub margins: Margins,
    pub when_no_data: WhenNoData,
    pub title_new_page: bool,
    pub summary_new_page: bool,
    pub summary_with_page_header_and_footer: bool,
    pub float_column_footer: bool,
    pub parameters: Vec<Parameter>,
    pub fields: Vec<Field>,
    pub variables: Vec<Variable>,
    pub groups: Vec<Group>,
    pub background: Option<Band>,
    pub title: Option<Band>,
    pub page_header: Option<Band>,
    pub column_header: Option<Band>,
    pub detail: Vec<Band>,
    pub column_footer: Option<Band>,
    pub page_footer: Option<Band>,
    pub last_page_footer: Option<Band>,
    pub summary: Option<Band>,
    pub no_data: Option<Band>,
    /// `<subDataset>` definitions, used by tables.
    pub datasets: Vec<Dataset>,
    /// The query that gives the report its rows, when it is filled from a database.
    pub query: Option<Query>,
    /// Features found in the template that are not rendered (yet).
    /// Expressions read `$P{REPORT_DATA_SOURCE}` (records then keep their source JSON row).
    pub uses_data_source: bool,
    /// `printOrder="Horizontal"` with more than one column: records fill a row of columns
    /// left to right, then the next row.
    pub horizontal: bool,
    /// What the template asks of PDF output (its `net.sf.jasperreports.export.pdf.*` properties).
    pub pdf: PdfSettings,
    pub warnings: Vec<String>,
}

/// What a template asks of PDF output, with its `net.sf.jasperreports.export.pdf.*` properties.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfSettings {
    /// Where pages are moved to, odd pages then even ones (x right, y down, in points):
    /// `odd.page.offset.x` and its siblings, for binding.
    pub page_offsets: [[f32; 2]; 2],
    /// `tagged`: write a tagged PDF (for screen readers) even when the renderer is not asked to.
    pub tagged: bool,
    /// `tag.language`: the language of the text (`es-DO`), instead of the renderer's locale.
    pub language: Option<String>,
    /// `net.sf.jasperreports.export.pdfa.conformance` (`pdfa2b`...): write PDF/A.
    pub pdfa: Option<crate::pdf::PdfA>,
    /// `encrypted`, `user.password`, `owner.password`, `permissions.allowed` and
    /// `permissions.denied`: encrypt the PDF.
    pub encryption: Option<crate::crypt::Encryption>,
}

impl Report {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// A barcode component (`jr:QRCode`, `jr:Code128`, `jr:EAN13`, ...).
#[derive(Debug, Clone)]
pub struct BarcodeElement {
    pub symbology: Symbology,
    pub expr: Option<Expr>,
    pub boxed: BoxStyle,
    /// Font of the human-readable text (from the element's style).
    pub font_name: Option<String>,
    pub font_size: Option<f32>,
}

#[derive(Debug, Clone)]
pub enum Symbology {
    Qr {
        level: crate::barcode::qr::Ecl,
        margin: Option<u32>,
        version: Option<u8>,
    },
    Code128 {
        module_width: Option<f64>,
        text_position: crate::barcode::TextPosition,
        quiet_zone: Option<f64>,
        vertical_quiet_zone: Option<f64>,
    },
    /// Code 39, Interleaved 2 of 5, Codabar, EAN-13, EAN-8, UPC-A, UPC-E.
    Linear(crate::barcode::linear::Spec),
    /// EAN-128 (GS1-128): Code 128 with application identifiers.
    Ean128 {
        module_width: Option<f64>,
        text_position: crate::barcode::TextPosition,
        quiet_zone: Option<f64>,
        vertical_quiet_zone: Option<f64>,
        checksum: crate::barcode::linear::Checksum,
    },
    /// PDF417, with barcode4j's settings.
    Pdf417 {
        spec: crate::barcode::Pdf417Spec,
        text_position: crate::barcode::TextPosition,
    },
    DataMatrix {
        module_width: Option<f64>,
        shape: crate::barcode::datamatrix::Shape,
    },
    /// POSTNET (heights and widths in points).
    Postnet {
        module_width: Option<f64>,
        short_bar: Option<f64>,
        gap: Option<f64>,
        add_checksum: bool,
    },
    /// The `barbecue` component (the Barbecue library), drawn rotated by `rotation`.
    Barbecue {
        spec: crate::barcode::barbecue::Spec,
        /// `applicationIdentifierExpression`, for `UCC128`.
        application_identifier: Option<Expr>,
        rotation: Rotation,
    },
    /// Royal Mail 4-state (`usps: false`) or USPS Intelligent Mail (sizes in points).
    FourState {
        usps: bool,
        module_width: Option<f64>,
        ascender: Option<f64>,
        track: Option<f64>,
        gap: Option<f64>,
        checksum: crate::barcode::linear::Checksum,
    },
}

impl Symbology {
    /// The component name used in JRXML.
    pub fn component(&self) -> &'static str {
        match self {
            Symbology::Qr { .. } => "QRCode",
            Symbology::Code128 { .. } => "Code128",
            Symbology::Linear(spec) => spec.kind.component(),
            Symbology::Ean128 { .. } => "EAN128",
            Symbology::DataMatrix { .. } => "DataMatrix",
            Symbology::Pdf417 { .. } => "PDF417",
            Symbology::Postnet { .. } => "POSTNET",
            Symbology::FourState { usps: true, .. } => "USPSIntelligentMail",
            Symbology::Barbecue { .. } => "barbecue",
            Symbology::FourState { .. } => "RoyalMailCustomer",
        }
    }
}

/// A `<subreport>`: another template filled with its own data for each print of the element.
#[derive(Debug, Clone)]
pub struct SubreportElement {
    /// Evaluates to the template location (`"lines.jasper"` resolves to `lines.jrxml`).
    pub expr: Option<Expr>,
    pub parameters: Vec<(String, Expr)>,
    pub parameters_map: Option<Expr>,
    pub data_source: Option<Expr>,
    /// A `connectionExpression` is given: rows come from the subreport's query, on the
    /// renderer's connection.
    pub connection: bool,
    pub returns: Vec<ReturnValue>,
}

/// Copies a subreport variable into a variable of the containing report.
#[derive(Debug, Clone)]
pub struct ReturnValue {
    pub from: String,
    pub to: String,
    pub calculation: Calculation,
}

/// A `<subDataset>`: the parameters, fields, variables and groups of a data source other than
/// the report's own (tables iterate over one).
#[derive(Debug, Clone, Default)]
pub struct Dataset {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub fields: Vec<Field>,
    pub variables: Vec<Variable>,
    pub groups: Vec<Group>,
    pub query: Option<Query>,
}

/// A `<datasetRun>`: the subdataset a table or list iterates over and where its rows and
/// parameters come from.
#[derive(Debug, Clone)]
pub struct DatasetRun {
    /// The subdataset (`None`: a dataset with no fields).
    pub dataset: Option<Dataset>,
    pub parameters: Vec<(String, Expr)>,
    pub parameters_map: Option<Expr>,
    pub data_source: Option<Expr>,
    /// A `connectionExpression` is given: rows come from the subdataset's query.
    pub connection: bool,
    /// Page height of the containing report (the report built for the run uses it).
    pub page_height: f32,
    /// Expression language of the containing report.
    pub language: Language,
}

/// A table component. JasperReports fills a table as a subreport built from its columns
/// (`TableReport`); [`crate::table`] builds the same report.
#[derive(Debug, Clone)]
pub struct TableElement {
    pub run: DatasetRun,
    pub columns: Vec<TableColumn>,
    pub when_no_data: WhenNoData,
    pub no_data: Option<TableCell>,
}

/// A list component (`jr:list`): its contents repeated for each row of the subdataset,
/// one below the other.
#[derive(Debug, Clone)]
pub struct ListElement {
    pub run: DatasetRun,
    /// The contents, as a band as tall as `<listContents height>`.
    pub contents: Band,
    /// `printOrder="Horizontal"`: the contents side by side, as many to a row as fit, each
    /// `<listContents width>` wide.
    pub across: Option<f32>,
}

/// Where a group's total row or column goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TotalPosition {
    None,
    Start,
    End,
}

/// Where a group header sits in the cells it spans (`headerPosition`): along the group's
/// direction, at its start, middle or end, or stretched over all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HeaderPosition {
    Start,
    Middle,
    End,
    Stretch,
}

/// A row or column group of a crosstab.
#[derive(Debug, Clone)]
pub struct CrosstabGroup {
    pub name: String,
    /// Width of a row group's headers, or height of a column group's.
    pub size: f32,
    pub total_position: TotalPosition,
    pub header_position: HeaderPosition,
    pub bucket: Option<Expr>,
    pub descending: bool,
    /// Sorts the buckets by this value, computed on each bucket's totals, instead of by the
    /// bucket values.
    pub order_by: Option<Expr>,
    pub class: JavaType,
    /// Header and total header contents, as frames of their design size.
    pub header: Option<Element>,
    pub total_header: Option<Element>,
}

/// A crosstab measure: a value aggregated per cell.
#[derive(Debug, Clone)]
pub struct CrosstabMeasure {
    pub name: String,
    pub class: JavaType,
    pub calculation: Calculation,
    pub expr: Option<Expr>,
    /// `percentageOf="GrandTotal"`: the value is shown as a percentage of the grand total.
    pub percentage: bool,
}

/// The contents of one kind of crosstab cell: detail, or a total of a row and/or column group.
#[derive(Debug, Clone)]
pub struct CrosstabCell {
    pub row_total: Option<String>,
    pub column_total: Option<String>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub contents: Option<Element>,
}

/// A crosstab: values of the data aggregated by row and column groups.
#[derive(Debug, Clone)]
pub struct CrosstabElement {
    /// Where the rows come from: a subdataset run, or (`None`) the report's own rows.
    pub run: Option<DatasetRun>,
    /// `<crosstabParameter>`s, evaluated in the report.
    pub parameters: Vec<(String, Expr)>,
    pub row_groups: Vec<CrosstabGroup>,
    pub column_groups: Vec<CrosstabGroup>,
    pub measures: Vec<CrosstabMeasure>,
    pub cells: Vec<CrosstabCell>,
    pub header_cell: Option<Element>,
    pub no_data_cell: Option<Element>,
    pub column_break_offset: f32,
    pub repeat_column_headers: bool,
    pub repeat_row_headers: bool,
}

/// A table column, or a column group with `children`.
#[derive(Debug, Clone)]
pub struct TableColumn {
    pub width: f32,
    pub print_when: Option<Expr>,
    pub children: Vec<TableColumn>,
    pub is_group: bool,
    pub table_header: Option<TableCell>,
    pub table_footer: Option<TableCell>,
    pub column_header: Option<TableCell>,
    pub column_footer: Option<TableCell>,
    pub detail: Option<TableCell>,
    pub group_headers: Vec<(String, TableCell)>,
    pub group_footers: Vec<(String, TableCell)>,
}

/// A table cell: its content as one element at (0, 0), a frame carrying the cell's style and
/// borders, or the cell's only text element when Jasper uses it directly.
#[derive(Debug, Clone)]
pub struct TableCell {
    pub height: f32,
    pub element: Element,
}

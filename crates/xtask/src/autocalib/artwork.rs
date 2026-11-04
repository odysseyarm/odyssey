use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use eros::Context as _;
use serde::Deserialize;

use crate::cmd;

const BOARD_JSON: &str = "crates/odyssey-autocalib/board.json";
const STRING_WRITE: &str = "formatting into a String cannot fail";

const MM_PER_INCH: f64 = 25.4;
const PT_PER_INCH: f64 = 72.0;
/// Control-point distance that makes four cubic Beziers a close circle.
const KAPPA: f64 = 0.5522847498307936;

const BAR_THICKNESS_MM: f64 = 0.8;
const BAR_TICK_MM: f64 = 6.0;
/// Gap kept between the dots and the scale bar, mm.
const BAR_CLEARANCE_MM: f64 = 3.0;
/// Office printers cannot print to the paper's edge; most leave about 6 mm.
const PAPER_MARGIN_MM: f64 = 6.35;
/// Where the scale bar sits, above the bottom edge, on a page the size of the board.
const BOARD_BAR_FROM_BOTTOM_MM: f64 = 8.0;

/// Board geometry; every length is millimeters in the board's own plane, x to the
/// right and y down, origin at the board's top-left corner.
#[derive(Clone, Deserialize)]
struct Spec {
    board_mm: [f64; 2],
    cols: i64,
    rows: i64,
    pitch_mm: f64,
    dot_diameter_mm: f64,
    /// (col, row) of the grid dots left out.
    missing: BTreeSet<(i64, i64)>,
    scale_bar_mm: f64,
}

impl Spec {
    fn load(path: &Path) -> eros::Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let spec =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(spec)
    }

    /// The same board with every length multiplied by `factor`.
    fn scaled(&self, factor: f64) -> Self {
        Self {
            board_mm: [self.board_mm[0] * factor, self.board_mm[1] * factor],
            pitch_mm: self.pitch_mm * factor,
            dot_diameter_mm: self.dot_diameter_mm * factor,
            scale_bar_mm: self.scale_bar_mm * factor,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Background {
    /// A white background is laid down explicitly, for printing onto a board.
    White,
    /// Left to the paper.
    Paper,
}

/// The layouts `artwork` writes; each name is also the output file stem.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum LayoutName {
    /// 12 x 8.5 in, for a print shop to print straight onto a rigid board (white
    /// background laid down explicitly)
    #[value(name = "scale-board-rigid")]
    Rigid,
    /// The rigid board at twice the size, 24 x 17 in, for a camera that sees the
    /// board from farther away: the dots must be about 12 px across in the color
    /// image (see BOARD.md)
    #[value(name = "scale-board-rigid-large")]
    RigidLarge,
    /// US Letter landscape, for an office printer
    #[value(name = "scale-board-paper-letter")]
    PaperLetter,
    /// A4 landscape, for an office printer
    #[value(name = "scale-board-paper-a4")]
    PaperA4,
}

impl LayoutName {
    const ALL: [Self; 4] = [
        Self::Rigid,
        Self::RigidLarge,
        Self::PaperLetter,
        Self::PaperA4,
    ];
    /// The quick paper board, the first thing to print.
    const DEFAULT: Self = Self::PaperLetter;

    fn name(self) -> &'static str {
        match self {
            Self::Rigid => "scale-board-rigid",
            Self::RigidLarge => "scale-board-rigid-large",
            Self::PaperLetter => "scale-board-paper-letter",
            Self::PaperA4 => "scale-board-paper-a4",
        }
    }

    fn layout(self) -> Layout {
        match self {
            Self::Rigid => Layout {
                page: None,
                background: Background::White,
                margin: 2.0,
                top: None,
                bar: BarY::FromBottom,
                scale: 1.0,
            },
            Self::RigidLarge => Layout {
                page: None,
                background: Background::White,
                margin: 2.0,
                top: None,
                bar: BarY::FromBottom,
                scale: 2.0,
            },
            Self::PaperLetter => Layout {
                page: Some([11.0 * MM_PER_INCH, 8.5 * MM_PER_INCH]),
                background: Background::Paper,
                margin: PAPER_MARGIN_MM,
                top: Some(12.0),
                bar: BarY::At(203.0),
                scale: 1.0,
            },
            Self::PaperA4 => Layout {
                page: Some([297.0, 210.0]),
                background: Background::Paper,
                margin: PAPER_MARGIN_MM,
                top: Some(8.0),
                bar: BarY::At(198.0),
                scale: 1.0,
            },
        }
    }
}

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Board geometry [default: crates/odyssey-autocalib/board.json]
    #[arg(long)]
    spec: Option<PathBuf>,
    #[arg(long, default_value = ".")]
    out_dir: PathBuf,
    /// Write this layout (repeatable)
    #[arg(long = "layout", value_enum)]
    layouts: Vec<LayoutName>,
    /// Write every layout
    #[arg(long)]
    all: bool,
    /// Write only a layout of your own, named NAME and placed by the options below
    #[arg(
        long = "custom",
        value_name = "NAME",
        conflicts_with_all = ["layouts", "all"],
        requires_all = ["page_width_mm", "page_height_mm", "margin_mm"]
    )]
    custom: Option<String>,
    #[arg(long, requires = "custom")]
    page_width_mm: Option<f64>,
    #[arg(long, requires = "custom")]
    page_height_mm: Option<f64>,
    /// Background of the custom layout [default: paper]
    #[arg(long, value_enum, requires = "custom")]
    background: Option<Background>,
    /// Margin of the custom layout that must stay clear
    #[arg(long, requires = "custom")]
    margin_mm: Option<f64>,
    /// The custom layout's dots' top edge from the page's top [default: centered]
    #[arg(long, requires = "custom")]
    top_mm: Option<f64>,
    /// Put the custom layout's scale bar this far below the highest position that
    /// keeps its clearance from the dots [default: 8 mm above the page's bottom]
    #[arg(long, requires = "custom")]
    bar_slack_mm: Option<f64>,
}

/// How one file places the board's dots on its page; lengths in millimeters.
struct Layout {
    /// Page (w, h), or `None` for a page the size of the board.
    page: Option<[f64; 2]>,
    background: Background,
    /// Margin that must stay clear.
    margin: f64,
    /// The dots' top edge from the page's top, or `None` to center them vertically.
    top: Option<f64>,
    bar: BarY,
    /// Factor applied to every length of the board.
    scale: f64,
}

/// Everything the art needs for one file, in millimeters from the page's top-left.
struct Art {
    page: [f64; 2],
    /// Dot centers.
    dots: Vec<[f64; 2]>,
    bar: Bar,
    background: Background,
    margin: f64,
}

struct Bar {
    x0: f64,
    x1: f64,
    y: f64,
}

/// Where the scale bar's centerline goes.
#[derive(Clone, Copy)]
enum BarY {
    /// `BOARD_BAR_FROM_BOTTOM_MM` above the page's bottom.
    FromBottom,
    /// This far from the page's top.
    At(f64),
    /// `slack` mm below the highest position the clearance check accepts.
    BelowDots { slack: f64 },
}

fn place(spec: &Spec, layout: &Layout) -> Art {
    let pitch = spec.pitch_mm;
    let radius = spec.dot_diameter_mm / 2.0;
    let [page_w, page_h] = layout.page.unwrap_or(spec.board_mm);
    let top_edge = layout
        .top
        .unwrap_or_else(|| (page_h - ((spec.rows - 1) as f64 * pitch + 2.0 * radius)) / 2.0);
    let left = (page_w - (spec.cols - 1) as f64 * pitch) / 2.0;
    let mut dots = Vec::new();
    for r in 0..spec.rows {
        for c in 0..spec.cols {
            if !spec.missing.contains(&(c, r)) {
                dots.push([
                    left + c as f64 * pitch,
                    top_edge + radius + r as f64 * pitch,
                ]);
            }
        }
    }
    let length = spec.scale_bar_mm;
    let y = match layout.bar {
        BarY::FromBottom => page_h - BOARD_BAR_FROM_BOTTOM_MM,
        BarY::At(y) => y,
        BarY::BelowDots { slack } => {
            let dots_bottom = top_edge + (spec.rows - 1) as f64 * pitch + spec.dot_diameter_mm;
            dots_bottom + BAR_CLEARANCE_MM + BAR_TICK_MM / 2.0 + slack
        }
    };
    let bar = Bar {
        x0: (page_w - length) / 2.0,
        x1: (page_w + length) / 2.0,
        y,
    };
    Art {
        page: [page_w, page_h],
        dots,
        bar,
        background: layout.background,
        margin: layout.margin,
    }
}

/// Refuses art that would not fit, would be clipped by a printer, or is ambiguous.
fn check(spec: &Spec, name: &str, art: &Art) -> eros::Result<()> {
    let [page_w, page_h] = art.page;
    let radius = spec.dot_diameter_mm / 2.0;
    let margin = art.margin;
    let expected = spec.cols * spec.rows - spec.missing.len() as i64;
    eros::ensure!(
        art.dots.len() as i64 == expected,
        "{name}: a missing dot lies outside the grid"
    );
    eros::ensure!(
        !spec.missing.is_empty(),
        "without a missing dot the board looks the same turned 180 degrees"
    );
    eros::ensure!(spec.dot_diameter_mm < spec.pitch_mm, "dots would touch");
    for &[x, y] in &art.dots {
        eros::ensure!(
            margin <= x - radius && x + radius <= page_w - margin,
            "{name}: a dot is too close to a side"
        );
        eros::ensure!(
            margin <= y - radius && y + radius <= page_h - margin,
            "{name}: a dot is too close to the top or bottom"
        );
    }
    let Bar { x0, x1, y } = art.bar;
    let mut lowest = f64::NEG_INFINITY;
    for &[_, cy] in &art.dots {
        lowest = lowest.max(cy);
    }
    lowest += radius;
    eros::ensure!(
        x0 >= margin && x1 <= page_w - margin,
        "{name}: the scale bar is too wide for the page"
    );
    eros::ensure!(
        y - BAR_TICK_MM / 2.0 >= lowest + BAR_CLEARANCE_MM,
        "{name}: the scale bar crowds the dots"
    );
    eros::ensure!(
        y + BAR_TICK_MM / 2.0 <= page_h - margin,
        "{name}: the scale bar is too close to the bottom"
    );
    Ok(())
}

/// Python's `repr` of a float of everyday magnitude (Rust's `Debug` only switches
/// to exponents at different bounds, which millimeter lengths never reach).
fn repr(value: f64) -> String {
    format!("{value:?}")
}

fn svg(spec: &Spec, art: &Art) -> String {
    let [width, height] = art.page;
    let radius = spec.dot_diameter_mm / 2.0;
    let Bar { x0, x1, y } = art.bar;
    let t = BAR_THICKNESS_MM;
    let mut out = vec![
        r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_owned(),
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.4}mm" height="{height:.4}mm" viewBox="0 0 {width:.4} {height:.4}">"#
        ),
    ];
    if art.background == Background::White {
        out.push(format!(
            r##"<rect x="0" y="0" width="{width:.4}" height="{height:.4}" fill="#ffffff"/>"##
        ));
    }
    out.push(r##"<g fill="#000000" stroke="none">"##.to_owned());
    for &[x, cy] in &art.dots {
        out.push(format!(
            r#"<circle cx="{x:.4}" cy="{cy:.4}" r="{}"/>"#,
            repr(radius)
        ));
    }
    out.push(format!(
        r#"<rect x="{x0:.4}" y="{:.4}" width="{:.4}" height="{}"/>"#,
        y - t / 2.0,
        x1 - x0,
        repr(t)
    ));
    for x in [x0, x1] {
        out.push(format!(
            r#"<rect x="{:.4}" y="{:.4}" width="{}" height="{}"/>"#,
            x - t / 2.0,
            y - BAR_TICK_MM / 2.0,
            repr(t),
            repr(BAR_TICK_MM)
        ));
    }
    out.extend(["</g>".to_owned(), "</svg>".to_owned(), String::new()]);
    out.join("\n")
}

fn pdf(spec: &Spec, art: &Art) -> Vec<u8> {
    let [width, height] = art.page;
    let pt = PT_PER_INCH / MM_PER_INCH;
    let radius = spec.dot_diameter_mm / 2.0 * pt;
    let (page_w, page_h) = (width * pt, height * pt);
    let flip = |y_mm: f64| page_h - y_mm * pt;
    let mut ops = Vec::new();
    if art.background == Background::White {
        ops.push("1 g".to_owned());
        ops.push(format!("0 0 {page_w:.4} {page_h:.4} re f"));
    }
    // DeviceGray black.
    ops.push("0 g".to_owned());
    for &[x_mm, y_mm] in &art.dots {
        let (cx, cy, k) = (x_mm * pt, flip(y_mm), radius * KAPPA);
        let r = radius;
        ops.push(format!("{:.4} {cy:.4} m", cx + r));
        ops.push(format!(
            "{:.4} {:.4} {:.4} {:.4} {cx:.4} {:.4} c",
            cx + r,
            cy + k,
            cx + k,
            cy + r,
            cy + r
        ));
        ops.push(format!(
            "{:.4} {:.4} {:.4} {:.4} {:.4} {cy:.4} c",
            cx - k,
            cy + r,
            cx - r,
            cy + k,
            cx - r
        ));
        ops.push(format!(
            "{:.4} {:.4} {:.4} {:.4} {cx:.4} {:.4} c",
            cx - r,
            cy - k,
            cx - k,
            cy - r,
            cy - r
        ));
        ops.push(format!(
            "{:.4} {:.4} {:.4} {:.4} {:.4} {cy:.4} c",
            cx + k,
            cy - r,
            cx + r,
            cy - k,
            cx + r
        ));
        ops.push("f".to_owned());
    }
    let Bar { x0, x1, y } = art.bar;
    let t = BAR_THICKNESS_MM;
    let rect = |x: f64, y_top: f64, w: f64, h: f64| {
        format!(
            "{:.4} {:.4} {:.4} {:.4} re f",
            x * pt,
            flip(y_top + h),
            w * pt,
            h * pt
        )
    };
    ops.push(rect(x0, y - t / 2.0, x1 - x0, t));
    for x in [x0, x1] {
        ops.push(rect(x - t / 2.0, y - BAR_TICK_MM / 2.0, t, BAR_TICK_MM));
    }
    let stream = ops.join("\n");

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w:.4} {page_h:.4}] /TrimBox [0 0 {page_w:.4} {page_h:.4}] /Contents 4 0 R /Resources << >> >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ),
    ];
    let mut body = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (number, object) in (1..).zip(&objects) {
        offsets.push(body.len());
        write!(body, "{number} 0 obj\n{object}\nendobj\n").expect(STRING_WRITE);
    }
    let xref = body.len();
    let size = objects.len() + 1;
    write!(body, "xref\n0 {size}\n0000000000 65535 f \n").expect(STRING_WRITE);
    for offset in offsets {
        writeln!(body, "{offset:010} 00000 n ").expect(STRING_WRITE);
    }
    write!(
        body,
        "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    )
    .expect(STRING_WRITE);
    body.into_bytes()
}

/// Writes the SVG and PDF of `layout` into `out` as `<name>.svg`/`.pdf` and says what they are.
fn write_layout(base: &Spec, name: &str, layout: &Layout, out: &Path) -> eros::Result<()> {
    let spec = base.scaled(layout.scale);
    let art = place(&spec, layout);
    check(&spec, name, &art).with_context(|| format!("cannot make {name}"))?;
    let svg_path = out.join(format!("{name}.svg"));
    fs::write(&svg_path, svg(&spec, &art))
        .with_context(|| format!("writing {}", svg_path.display()))?;
    let pdf_path = out.join(format!("{name}.pdf"));
    fs::write(&pdf_path, pdf(&spec, &art))
        .with_context(|| format!("writing {}", pdf_path.display()))?;
    let [w, h] = art.page;
    // shortcut: Display stands in for Python's `:g`, equal for lengths of at most 6 significant digits.
    println!(
        "{name}: {} dots of {} mm at {} mm pitch, page {w:.1} x {h:.1} mm ({:.2} x {:.2} in), {} mm scale bar",
        art.dots.len(),
        spec.dot_diameter_mm,
        spec.pitch_mm,
        w / MM_PER_INCH,
        h / MM_PER_INCH,
        spec.scale_bar_mm
    );
    Ok(())
}

pub(super) fn run(args: &Args) -> eros::Result<()> {
    let spec_path = args
        .spec
        .clone()
        .unwrap_or_else(|| cmd::root().join(BOARD_JSON));
    let base = Spec::load(&spec_path)?;
    let out = &args.out_dir;
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    if let Some(name) = &args.custom {
        let (Some(width), Some(height), Some(margin)) =
            (args.page_width_mm, args.page_height_mm, args.margin_mm)
        else {
            eros::bail!("--custom needs --page-width-mm, --page-height-mm and --margin-mm");
        };
        let layout = Layout {
            page: Some([width, height]),
            background: args.background.unwrap_or(Background::Paper),
            margin,
            top: args.top_mm,
            bar: args
                .bar_slack_mm
                .map_or(BarY::FromBottom, |slack| BarY::BelowDots { slack }),
            scale: 1.0,
        };
        return write_layout(&base, name, &layout, out);
    }
    let layouts = if args.all {
        LayoutName::ALL.as_slice()
    } else if args.layouts.is_empty() {
        &[LayoutName::DEFAULT]
    } else {
        args.layouts.as_slice()
    };
    for &layout in layouts {
        write_layout(&base, layout.name(), &layout.layout(), out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Spec {
        Spec::load(&cmd::root().join(BOARD_JSON)).expect("board.json in the repository parses")
    }

    fn rendered(name: LayoutName) -> (Vec<u8>, String) {
        let spec = board().scaled(name.layout().scale);
        let art = place(&spec, &name.layout());
        check(&spec, name.name(), &art).expect("committed layouts pass their checks");
        (pdf(&spec, &art), svg(&spec, &art))
    }

    /// odyssey-desktop embeds these files, so the generator must keep reproducing them.
    #[test]
    fn reproduces_committed_artwork() {
        let dir = cmd::root().join("crates/odyssey-autocalib/tools/artwork");
        for name in LayoutName::ALL {
            let (pdf, svg) = rendered(name);
            let committed_pdf =
                fs::read(dir.join(format!("{}.pdf", name.name()))).expect("committed PDF");
            let committed_svg = fs::read_to_string(dir.join(format!("{}.svg", name.name())))
                .expect("committed SVG");
            assert!(pdf == committed_pdf, "{} PDF differs", name.name());
            assert_eq!(svg, committed_svg, "{} SVG differs", name.name());
        }
    }

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr(10.0), "10.0");
        assert_eq!(repr(0.8), "0.8");
        assert_eq!(repr(6.0), "6.0");
        assert_eq!(repr(0.1 + 0.2), "0.30000000000000004");
    }

    #[test]
    fn four_decimals_round_like_python() {
        // Exact binary ties round half to even, as Python's `:.4f` does.
        assert_eq!(format!("{:.4}", 1.03125), "1.0312");
        assert_eq!(format!("{:.4}", 1.09375), "1.0938");
        assert_eq!(format!("{:.4}", -0.00001), "-0.0000");
    }

    #[test]
    fn letter_layout_leaves_out_missing_dot() {
        let spec = board();
        let art = place(&spec, &LayoutName::PaperLetter.layout());
        assert_eq!(art.dots.len(), 34);
        let close =
            |[x, y]: [f64; 2], [ex, ey]: [f64; 2]| (x - ex).abs() < 1e-9 && (y - ey).abs() < 1e-9;
        // Letter is 279.4 mm wide, so column 0 sits at (279.4 - 6 * 40) / 2. Column 6 of
        // row 0 is missing, so index 5 ends row 0 and index 6 starts row 1.
        assert!(close(art.dots[5], [19.7 + 5.0 * 40.0, 22.0]));
        assert!(close(art.dots[6], [19.7, 62.0]));
    }

    #[test]
    fn rigid_large_doubles_every_length() {
        let spec = board().scaled(LayoutName::RigidLarge.layout().scale);
        assert_eq!(spec.board_mm, [609.6, 431.8]);
        assert_eq!(spec.pitch_mm, 80.0);
        assert_eq!(spec.dot_diameter_mm, 40.0);
        assert_eq!(spec.scale_bar_mm, 400.0);
    }

    #[test]
    fn check_refuses_bad_boards() {
        let layout = LayoutName::Rigid.layout();
        let refusal = |spec: &Spec| {
            let art = place(spec, &layout);
            check(spec, "t", &art)
                .expect_err("check refuses")
                .to_string()
        };
        let mut outside = board();
        outside.missing.insert((7, 0));
        assert!(refusal(&outside).contains("outside the grid"));
        let mut symmetric = board();
        symmetric.missing.clear();
        assert!(refusal(&symmetric).contains("180 degrees"));
        let mut touching = board();
        touching.dot_diameter_mm = touching.pitch_mm;
        assert!(refusal(&touching).contains("dots would touch"));
        let mut wide = board();
        wide.cols = 9;
        assert!(refusal(&wide).contains("too close to a side"));
        let mut long_bar = board();
        long_bar.scale_bar_mm = 301.0;
        assert!(refusal(&long_bar).contains("too wide"));
    }

    #[test]
    fn check_accepts_bar_exactly_at_clearance() {
        let spec = board();
        let mut layout = LayoutName::PaperLetter.layout();
        // Dots end at 12 + 4 * 40 + 20 = 192 mm.
        layout.bar = BarY::At(192.0 + BAR_CLEARANCE_MM + BAR_TICK_MM / 2.0);
        check(&spec, "t", &place(&spec, &layout)).expect("bar at the clearance fits");
        layout.bar = BarY::At(197.9);
        let art = place(&spec, &layout);
        assert!(
            check(&spec, "t", &art)
                .expect_err("bar crowds")
                .to_string()
                .contains("crowds")
        );
    }

    #[test]
    fn bar_slack_counts_from_tightest_bar() {
        let spec = board();
        let mut layout = LayoutName::PaperLetter.layout();
        layout.bar = BarY::BelowDots { slack: 0.0 };
        let art = place(&spec, &layout);
        assert_eq!(art.bar.y, 192.0 + BAR_CLEARANCE_MM + BAR_TICK_MM / 2.0);
        check(&spec, "t", &art).expect("the tightest bar fits");
        layout.top = Some(4.0);
        layout.bar = BarY::BelowDots { slack: 1.0 };
        // 4 + 4 * 40 + 20 + 3 + 3 + 1
        assert_eq!(place(&spec, &layout).bar.y, 191.0);
    }

    #[test]
    fn pdf_xref_points_at_objects() {
        let (pdf, _) = rendered(LayoutName::PaperA4);
        let text = String::from_utf8(pdf).expect("PDF is ASCII");
        let startxref: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|tail| tail.lines().next())
            .and_then(|line| line.parse().ok())
            .expect("startxref offset");
        assert!(text[startxref..].starts_with("xref\n0 5\n"));
        for (number, line) in (1..).zip(text[startxref..].lines().skip(3).take(4)) {
            let offset: usize = line[..10].parse().expect("xref offset");
            assert!(text[offset..].starts_with(&format!("{number} 0 obj\n")));
        }
    }
}

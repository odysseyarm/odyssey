//! The scale board's print files, carried inside the app, and a way to put one
//! in front of the user: written to a folder and opened in the system's PDF
//! viewer, where printing or saving is one click. Nothing here touches the
//! network, so it works the same in offline mode.
//!
//! The PDFs are generated from `crates/odyssey-autocalib/board.json` by
//! `tools/board_artwork.py --all`; see `tools/BOARD.md` there.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

macro_rules! artwork {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/odyssey-autocalib/tools/artwork/",
            $name
        ))
    };
}

/// One of the printable boards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Board {
    /// US Letter paper, for an office printer.
    Letter,
    /// A4 paper, for an office printer.
    A4,
    /// 12 x 8.5 in, for a print shop to print onto a rigid board.
    Rigid,
    /// 24 x 17 in, the same for a camera farther from the screen.
    RigidLarge,
}

impl Board {
    pub const ALL: [Board; 4] = [Board::Letter, Board::A4, Board::Rigid, Board::RigidLarge];

    pub fn file_name(self) -> &'static str {
        match self {
            Self::Letter => "scale-board-paper-letter.pdf",
            Self::A4 => "scale-board-paper-a4.pdf",
            Self::Rigid => "scale-board-rigid.pdf",
            Self::RigidLarge => "scale-board-rigid-large.pdf",
        }
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Letter => artwork!("scale-board-paper-letter.pdf"),
            Self::A4 => artwork!("scale-board-paper-a4.pdf"),
            Self::Rigid => artwork!("scale-board-rigid.pdf"),
            Self::RigidLarge => artwork!("scale-board-rigid-large.pdf"),
        }
    }

    /// Distance between dot centers, millimeters, as designed. A print can
    /// differ by the printer's own scaling; see the guide's scale check.
    pub fn pitch_mm(self) -> f64 {
        match self {
            Self::RigidLarge => 80.0,
            _ => 40.0,
        }
    }

    /// Length of the printed scale bar, millimeters, as designed.
    pub fn bar_mm(self) -> f64 {
        self.pitch_mm() * 5.0
    }
}

/// Where the print files are written.
pub fn print_dir() -> PathBuf {
    std::env::temp_dir().join("odyssey-scale-board")
}

/// Writes `board`'s PDF into `dir`, creating it, and returns the file's path.
pub fn save(board: Board, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(board.file_name());
    std::fs::write(&path, board.bytes())?;
    Ok(path)
}

/// Writes `board` to [`print_dir`] and opens it in the system's PDF viewer.
/// On failure to open, the error says where the file is so it can be opened by
/// hand.
pub fn save_and_open(board: Board) -> Result<PathBuf, String> {
    let path = save(board, &print_dir()).map_err(|e| format!("Couldn't save the file: {e}."))?;
    open(&path).map_err(|e| {
        format!(
            "Couldn't open a PDF viewer ({e}). The file is saved at {}.",
            path.display()
        )
    })?;
    Ok(path)
}

/// The system's own "open this file" command for `path`.
fn opener(path: &Path) -> Command {
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

fn open(path: &Path) -> io::Result<()> {
    let mut child = opener(path).spawn()?;
    // Reap it without making the caller wait on a viewer that may never exit.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page size in PDF points, read from the file's own MediaBox.
    fn media_box(pdf: &[u8]) -> [f64; 2] {
        let text = String::from_utf8_lossy(pdf);
        let start = text.find("/MediaBox [").expect("a MediaBox") + "/MediaBox [".len();
        let numbers: Vec<f64> = text[start..]
            .split(']')
            .next()
            .unwrap()
            .split_whitespace()
            .map(|n| n.parse().expect("a number"))
            .collect();
        [numbers[2] - numbers[0], numbers[3] - numbers[1]]
    }

    #[test]
    fn each_file_is_a_pdf_of_the_size_it_is_named_for() {
        // Points: 72 per inch. A4 is 297 x 210 mm.
        let expected = [
            (Board::Letter, [792.0, 612.0]),
            (Board::A4, [297.0 / 25.4 * 72.0, 210.0 / 25.4 * 72.0]),
            (Board::Rigid, [864.0, 612.0]),
            (Board::RigidLarge, [1728.0, 1224.0]),
        ];
        for (board, [w, h]) in expected {
            let pdf = board.bytes();
            assert!(pdf.starts_with(b"%PDF-"), "{board:?} is not a PDF");
            assert!(
                pdf.trim_ascii_end().ends_with(b"%%EOF"),
                "{board:?} is cut short"
            );
            let [got_w, got_h] = media_box(pdf);
            assert!(
                (got_w - w).abs() < 0.01 && (got_h - h).abs() < 0.01,
                "{board:?} is {got_w} x {got_h} pt, expected {w} x {h}"
            );
        }
    }

    #[test]
    fn the_files_are_distinct_and_the_scale_check_matches_the_large_board() {
        let names: std::collections::HashSet<_> =
            Board::ALL.iter().map(|b| b.file_name()).collect();
        assert_eq!(names.len(), Board::ALL.len());
        let bytes: std::collections::HashSet<_> = Board::ALL.iter().map(|b| b.bytes()).collect();
        assert_eq!(bytes.len(), Board::ALL.len(), "two boards share one file");
        // The bar is five pitches long, which is what the guide's check relies on.
        assert_eq!(
            (Board::Letter.pitch_mm(), Board::Letter.bar_mm()),
            (40.0, 200.0)
        );
        assert_eq!(
            (Board::RigidLarge.pitch_mm(), Board::RigidLarge.bar_mm()),
            (80.0, 400.0)
        );
    }

    #[test]
    fn saving_writes_the_exact_file_and_creates_the_folder() {
        let dir = std::env::temp_dir().join(format!("odyssey-board-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("not").join("there").join("yet");
        for board in Board::ALL {
            let path = save(board, &nested).expect("saved");
            assert_eq!(path, nested.join(board.file_name()));
            assert_eq!(std::fs::read(&path).unwrap(), board.bytes());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

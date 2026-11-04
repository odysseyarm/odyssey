//! Placing the board's dot lattice over the blobs.
//!
//! No knowledge of the print's size or of the camera enters here: a placement
//! is a homography from grid units to undistorted pixels, found by seeding
//! local frames from the blobs and growing them into hypotheses.

use std::cmp::Reverse;
use std::collections::HashSet;

use nalgebra::{Matrix2, Matrix3, Point2, Vector2};

use super::blobs::DotBlob;
use super::{BoardError, BoardSpec, MIN_DOTS, MIN_PLAUSIBLE_DOTS};
use crate::plane::fit_homography;

/// A predicted dot matches a blob within this fraction of the local pitch.
const MATCH_TOLERANCE: f64 = 0.3;
/// Nearest neighbors per seed blob that may serve as its grid axes.
const SEED_NEIGHBORS: usize = 6;
/// How many of the most grid-like blobs seed hypotheses.
const SEED_BLOBS: usize = 10;
/// How many of a seed's nearest neighbors are tried as its first grid axis.
const SEED_FIRST_AXES: usize = 2;
/// Sine of the smallest angle between a seed's two axes: the frame must not
/// be nearly degenerate.
const MIN_AXIS_SINE: f64 = 0.6;
/// A seed's two axes have lengths whose ratio falls in this range, as the
/// two grid steps of one lattice cell do at moderate view angles.
const AXIS_LENGTH_RATIO: std::ops::RangeInclusive<f64> = 0.55..=1.8;
/// A seed frame is only a local affine guess: the first round of matching
/// considers only lattice cells within this many grid steps of the seed
/// cell (with slack for rounding).
const SEED_NEIGHBORHOOD_STEPS: f64 = 2.01;
/// Fewest matches a seed's local guess must produce to be worth refitting.
const MIN_SEED_MATCHES: usize = 5;
/// Refit rounds after which a hypothesis is taken as it is even if its
/// matches still change.
const MAX_GROW_ROUNDS: usize = 6;
/// A homography maps a point to infinity when its last row evaluates under
/// this.
const MIN_HOMOGENEOUS_W: f64 = 1e-12;
/// Distance to the fourth-nearest neighbor over distance to the nearest, for
/// a blob with fewer than four neighbors: such a blob is never grid-like.
const ISOLATED_SCORE: f64 = 1e6;
/// Floor for the nearest-neighbor distance in that ratio, pixels.
const MIN_NEIGHBOR_DISTANCE_PX: f64 = 1e-9;
/// Index among a blob's sorted neighbor distances that is compared with the
/// nearest: the fourth neighbor is the farthest of the four edge neighbors of
/// an interior dot.
const SCORED_NEIGHBOR_RANK: usize = 3;

/// The four ways a camera can see the grid rotated in its plane: the lattice
/// steps the seed's first and second axes correspond to.
const LATTICE_ROTATIONS: [((i64, i64), (i64, i64)); 4] = [
    ((1, 0), (0, 1)),
    ((0, 1), (-1, 0)),
    ((-1, 0), (0, -1)),
    ((0, -1), (1, 0)),
];

/// One dot of the board.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cell {
    pub(super) col: u32,
    pub(super) row: u32,
    /// Position in grid units.
    pub(super) pos: Vector2<f64>,
}

/// The board's dots, without the missing ones.
#[derive(Debug)]
pub(super) struct Lattice {
    pub(super) cells: Vec<Cell>,
    cols: u32,
    rows: u32,
    /// Index into `cells` of every `(col, row)` position, `None` where the
    /// dot is missing.
    index: Vec<Option<usize>>,
}

impl Lattice {
    pub(super) fn new(spec: &BoardSpec) -> Self {
        let mut cells = Vec::new();
        let mut index = vec![None; (spec.cols * spec.rows) as usize];
        for row in 0..spec.rows {
            for col in 0..spec.cols {
                if spec.is_missing(col, row) {
                    continue;
                }
                index[(row * spec.cols + col) as usize] = Some(cells.len());
                cells.push(Cell {
                    col,
                    row,
                    pos: spec.dot_position(col, row),
                });
            }
        }
        Self {
            cells,
            cols: spec.cols,
            rows: spec.rows,
            index,
        }
    }

    /// Index of the dot at `(col, row)`, if the board has one there.
    fn at(&self, col: i64, row: i64) -> Option<usize> {
        if col < 0 || row < 0 || col >= i64::from(self.cols) || row >= i64::from(self.rows) {
            return None;
        }
        self.index[(row as usize) * self.cols as usize + col as usize]
    }
}

/// Row-major homography, for the matching loops: plain `f64` arithmetic keeps
/// the hundreds of thousands of predictions cheap even in unoptimized builds.
type Rows = [f64; 9];

fn rows_of(h: &Matrix3<f64>) -> Rows {
    [
        h[(0, 0)],
        h[(0, 1)],
        h[(0, 2)],
        h[(1, 0)],
        h[(1, 1)],
        h[(1, 2)],
        h[(2, 0)],
        h[(2, 1)],
        h[(2, 2)],
    ]
}

fn warp(h: &Rows, x: f64, y: f64) -> Option<[f64; 2]> {
    let w = h[6] * x + h[7] * y + h[8];
    (w.abs() > MIN_HOMOGENEOUS_W).then(|| {
        [
            (h[0] * x + h[1] * y + h[2]) / w,
            (h[3] * x + h[4] * y + h[5]) / w,
        ]
    })
}

/// Image distance of one grid step at `pos` (whose image is `at`).
fn pitch_at(h: &Rows, pos: [f64; 2], at: [f64; 2]) -> Option<f64> {
    let a = warp(h, pos[0] + 1.0, pos[1])?;
    let b = warp(h, pos[0], pos[1] + 1.0)?;
    Some(((a[0] - at[0]).hypot(a[1] - at[1]) + (b[0] - at[0]).hypot(b[1] - at[1])) / 2.0)
}

/// Image distance of one grid step at the grid center.
fn center_pitch(h: &Rows) -> Option<f64> {
    warp(h, 0.0, 0.0).and_then(|center| pitch_at(h, [0.0, 0.0], center))
}

/// Source and destination points of `(blob, cell)` pairs for a homography
/// fit: the cells' grid positions and the blobs' centers.
pub(super) fn matched_points(
    matches: &[(usize, usize)],
    blobs: &[DotBlob],
    lattice: &Lattice,
) -> (Vec<Point2<f64>>, Vec<Point2<f64>>) {
    let grid = matches
        .iter()
        .map(|&(_, c)| Point2::from(lattice.cells[c].pos))
        .collect();
    let pixels = matches.iter().map(|&(b, _)| blobs[b].center).collect();
    (grid, pixels)
}

/// A placement of the lattice over the blobs.
#[derive(Debug, Clone)]
pub(super) struct Hypothesis {
    /// Grid units to undistorted pixels.
    pub(super) h: Matrix3<f64>,
    /// `(blob, cell)` pairs.
    pub(super) matches: Vec<(usize, usize)>,
    /// Predicted dots that fall inside the image but have no blob.
    unmatched_inside: usize,
}

type Key = (usize, Reverse<usize>);

impl Hypothesis {
    /// Better when more dots match, then when fewer predicted dots are
    /// missing from the visible image.
    fn key(&self) -> Key {
        (self.matches.len(), Reverse(self.unmatched_inside))
    }

    /// Image distance of one grid step at the grid center.
    pub(super) fn center_pitch_px(&self) -> Option<f64> {
        center_pitch(&rows_of(&self.h))
    }
}

/// Matches the lattice's predicted dots to blobs under `h`, each blob going
/// to the nearest prediction within [`MATCH_TOLERANCE`] of the local pitch.
/// `around` limits the prediction to cells within [`SEED_NEIGHBORHOOD_STEPS`]
/// grid steps of a point. Returned pairs are `(blob, cell)` in blob order.
fn match_cells(
    h: &Rows,
    blobs: &[DotBlob],
    lattice: &Lattice,
    around: Option<[f64; 2]>,
) -> Vec<(usize, usize)> {
    let mut owner: Vec<Option<(f64, usize)>> = vec![None; blobs.len()];
    for (ci, cell) in lattice.cells.iter().enumerate() {
        if let Some(a) = around
            && (cell.pos.x - a[0]).abs().max((cell.pos.y - a[1]).abs()) > SEED_NEIGHBORHOOD_STEPS
        {
            continue;
        }
        let Some(px) = warp(h, cell.pos.x, cell.pos.y) else {
            continue;
        };
        let Some(pitch) = pitch_at(h, [cell.pos.x, cell.pos.y], px) else {
            continue;
        };
        let tolerance_sq = (MATCH_TOLERANCE * pitch).powi(2);
        let mut best: Option<(f64, usize)> = None;
        for (bi, blob) in blobs.iter().enumerate() {
            let d = (blob.center.x - px[0]).powi(2) + (blob.center.y - px[1]).powi(2);
            if d <= tolerance_sq && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, bi));
            }
        }
        if let Some((d, bi)) = best
            && owner[bi].is_none_or(|(od, _)| d < od)
        {
            owner[bi] = Some((d, ci));
        }
    }
    owner
        .iter()
        .enumerate()
        .filter_map(|(bi, o)| o.map(|(_, ci)| (bi, ci)))
        .collect()
}

fn fit_matches(
    matches: &[(usize, usize)],
    blobs: &[DotBlob],
    lattice: &Lattice,
) -> Option<Matrix3<f64>> {
    let (grid, pixels) = matched_points(matches, blobs, lattice);
    fit_homography(&grid, &pixels)
}

/// Number of lattice cells that `h` places inside the image (at least a
/// quarter pitch from its border) but that no match claims.
fn count_unmatched_inside(
    h: &Rows,
    matches: &[(usize, usize)],
    lattice: &Lattice,
    image: (f64, f64),
) -> usize {
    let mut matched = vec![false; lattice.cells.len()];
    for &(_, c) in matches {
        matched[c] = true;
    }
    let mut count = 0;
    for (ci, cell) in lattice.cells.iter().enumerate() {
        if matched[ci] {
            continue;
        }
        let Some(px) = warp(h, cell.pos.x, cell.pos.y) else {
            continue;
        };
        let margin = pitch_at(h, [cell.pos.x, cell.pos.y], px).map_or(0.0, |p| p / 4.0);
        if px[0] >= margin
            && px[1] >= margin
            && px[0] <= image.0 - margin
            && px[1] <= image.1 - margin
        {
            count += 1;
        }
    }
    count
}

/// Grows a hypothesis: matches dots under `h0`, refits a homography to the
/// matches, and repeats until the matches stop changing. The first round is
/// limited to the neighborhood of the seed because the seed is only a local
/// affine guess. `seen` holds the match sets already grown, so that two seeds
/// reaching the same placement cost one fit, not two.
fn grow_hypothesis(
    h0: &Rows,
    seed: [f64; 2],
    blobs: &[DotBlob],
    lattice: &Lattice,
    image: (f64, f64),
    seen: &mut HashSet<Vec<(usize, usize)>>,
) -> Option<Hypothesis> {
    let mut matches = match_cells(h0, blobs, lattice, Some(seed));
    if matches.len() < MIN_SEED_MATCHES {
        return None;
    }
    let mut h = fit_matches(&matches, blobs, lattice)?;
    for round in 0..MAX_GROW_ROUNDS {
        let next = match_cells(&rows_of(&h), blobs, lattice, None);
        if next.len() < MIN_PLAUSIBLE_DOTS {
            return None;
        }
        if round == 0 && !seen.insert(next.clone()) {
            return None;
        }
        let settled = next == matches;
        matches = next;
        if settled {
            break;
        }
        h = fit_matches(&matches, blobs, lattice)?;
    }
    let unmatched_inside = count_unmatched_inside(&rows_of(&h), &matches, lattice, image);
    Some(Hypothesis {
        h,
        matches,
        unmatched_inside,
    })
}

/// Whether two placements disagree about where the grid is, by more than a
/// third of a pitch at any corner of the grid.
fn placements_differ(a: &Rows, b: &Rows, lattice: &Lattice) -> bool {
    let half = [
        f64::from(lattice.cols - 1) / 2.0,
        f64::from(lattice.rows - 1) / 2.0,
    ];
    let Some(pitch) = center_pitch(a) else {
        return true;
    };
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            let (x, y) = (sx * half[0], sy * half[1]);
            match (warp(a, x, y), warp(b, x, y)) {
                (Some(pa), Some(pb)) if (pa[0] - pb[0]).hypot(pa[1] - pb[1]) <= pitch / 3.0 => {}
                _ => return true,
            }
        }
    }
    false
}

/// Blobs most likely to be interior dots of a regular grid come first: the
/// ratio of the fourth to the first neighbor distance is near 1 inside a
/// lattice and large for isolated marks and for edge dots.
fn seed_order(blobs: &[DotBlob]) -> Vec<usize> {
    let mut scored: Vec<(f64, usize)> = blobs
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let mut d: Vec<f64> = blobs
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, o)| (o.center - b.center).norm())
                .collect();
            d.sort_by(f64::total_cmp);
            let score = if d.len() > SCORED_NEIGHBOR_RANK {
                d[SCORED_NEIGHBOR_RANK] / d[0].max(MIN_NEIGHBOR_DISTANCE_PX)
            } else {
                ISOLATED_SCORE
            };
            (score, i)
        })
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored
        .into_iter()
        .take(SEED_BLOBS)
        .map(|(_, i)| i)
        .collect()
}

/// Local frames at blob `seed`: pairs of image vectors to two neighbors that
/// stand for the grid's two axes, ordered so that they wind the same way
/// every time (a board seen from the front is never mirrored).
fn seed_frames(blobs: &[DotBlob], seed: usize) -> Vec<Matrix2<f64>> {
    let origin = blobs[seed].center;
    let mut neighbors: Vec<(f64, usize)> = blobs
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != seed)
        .map(|(i, b)| ((b.center - origin).norm(), i))
        .collect();
    neighbors.sort_by(|a, b| a.0.total_cmp(&b.0));
    neighbors.truncate(SEED_NEIGHBORS);

    let mut frames = Vec::new();
    for &(la, a) in neighbors.iter().take(SEED_FIRST_AXES) {
        let va = blobs[a].center - origin;
        // The nearest neighbor at a clearly different direction and a
        // similar distance is the other axis.
        let other = neighbors.iter().find(|&&(lb, b)| {
            let vb = blobs[b].center - origin;
            b != a
                && (va.x * vb.y - va.y * vb.x).abs() / (la * lb) >= MIN_AXIS_SINE
                && AXIS_LENGTH_RATIO.contains(&(la / lb))
        });
        let Some(&(_, b)) = other else {
            continue;
        };
        let mut vb = blobs[b].center - origin;
        let mut va = va;
        if va.x * vb.y - va.y * vb.x < 0.0 {
            std::mem::swap(&mut va, &mut vb);
        }
        frames.push(Matrix2::from_columns(&[va, vb]));
    }
    frames
}

/// The affine map that sends `cell` to `origin` and the lattice steps
/// `step_a` and `step_b` to the image axes of `image_axes`, in the form the
/// matching loops use.
fn affine_guess(
    origin: Point2<f64>,
    image_axes: &Matrix2<f64>,
    cell: &Cell,
    (step_a, step_b): ((i64, i64), (i64, i64)),
) -> Rows {
    let grid_axes = Matrix2::new(
        step_a.0 as f64,
        step_b.0 as f64,
        step_a.1 as f64,
        step_b.1 as f64,
    );
    let affine = image_axes * grid_axes.transpose();
    let offset = origin.coords - affine * cell.pos;
    [
        affine[(0, 0)],
        affine[(0, 1)],
        offset.x,
        affine[(1, 0)],
        affine[(1, 1)],
        offset.y,
        0.0,
        0.0,
        1.0,
    ]
}

/// Every grown hypothesis and the best of them.
#[derive(Debug, Default)]
struct Search {
    found: Vec<(Rows, Key)>,
    best: Option<Hypothesis>,
}

impl Search {
    fn record(&mut self, hypothesis: Hypothesis) {
        self.found.push((rows_of(&hypothesis.h), hypothesis.key()));
        if self
            .best
            .as_ref()
            .is_none_or(|best| hypothesis.key() > best.key())
        {
            self.best = Some(hypothesis);
        }
    }

    /// The best hypothesis, if it has enough dots and no differently placed
    /// hypothesis explains the blobs equally well.
    fn into_unambiguous(
        self,
        blobs: &[DotBlob],
        lattice: &Lattice,
    ) -> Result<Hypothesis, BoardError> {
        let no_board = || BoardError::NoBoard { blobs: blobs.len() };
        let best = self.best.ok_or_else(no_board)?;
        if best.matches.len() < MIN_DOTS {
            return Err(if best.matches.len() < MIN_PLAUSIBLE_DOTS {
                no_board()
            } else {
                BoardError::TooFewDots {
                    found: best.matches.len(),
                    needed: MIN_DOTS,
                }
            });
        }
        let best_rows = rows_of(&best.h);
        let best_key = best.key();
        let tied = self
            .found
            .iter()
            .any(|(h, key)| *key == best_key && placements_differ(&best_rows, h, lattice));
        if tied {
            return Err(BoardError::AmbiguousPlacement {
                matched: best.matches.len(),
            });
        }
        Ok(best)
    }
}

/// Finds the grid's placement. Each of the most grid-like blobs seeds a local
/// frame with two of its nearest neighbors; the frame is tried at every
/// lattice cell in the four ways a camera can see the grid rotated in its
/// plane, grown into a homography, and the hypothesis that explains the most
/// blobs wins. The missing dot is what makes the winner unique; if another
/// placement ties it, the view is ambiguous.
pub(super) fn match_grid(
    blobs: &[DotBlob],
    lattice: &Lattice,
    image: (f64, f64),
) -> Result<Hypothesis, BoardError> {
    let mut seen = HashSet::new();
    let mut search = Search::default();
    for seed in seed_order(blobs) {
        let origin = blobs[seed].center;
        for image_axes in seed_frames(blobs, seed) {
            for cell in &lattice.cells {
                let (c, r) = (i64::from(cell.col), i64::from(cell.row));
                for rotation @ (step_a, step_b) in LATTICE_ROTATIONS {
                    if lattice.at(c + step_a.0, r + step_a.1).is_none()
                        || lattice.at(c + step_b.0, r + step_b.1).is_none()
                    {
                        continue;
                    }
                    let h0 = affine_guess(origin, &image_axes, cell, rotation);
                    let seed_cell = [cell.pos.x, cell.pos.y];
                    if let Some(hypothesis) =
                        grow_hypothesis(&h0, seed_cell, blobs, lattice, image, &mut seen)
                    {
                        search.record(hypothesis);
                    }
                }
            }
        }
    }
    search.into_unambiguous(blobs, lattice)
}

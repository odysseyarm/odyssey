//! Matching blobs to the pattern's grid: a seed homography from the three anchor dots, then
//! refinement against all the dots.

use std::collections::HashMap;

use nalgebra::{Matrix3, Point2, Vector2};

use super::blobs::Blob;
use super::{
    MIN_DOTS, Match, Pattern, dot_positions, inlier_threshold, robust_fit, turns_wrong_way,
};
use crate::plane::{apply_homography, fit_homography};

/// Side of a [`PointIndex`] cell, in pixels. Larger than a typical match radius, so a lookup
/// touches a handful of cells.
const INDEX_CELL_PX: f64 = 16.0;

/// How close a blob must lie to a dot predicted by a seed hypothesis, as a share of the local
/// dot spacing. Seed hypotheses are loose, so this is generous but still rejects wrong ones.
const SEED_GATE: f64 = 0.3;
/// The same for the refinement passes, which predict with a model fit on many dots.
const REFINE_GATE: f64 = 0.35;
/// Refinement stops early once a pass keeps the same number of matches.
const REFINE_PASSES: usize = 4;

/// Share of the dots a seed or refined fit must match to count as the pattern.
const MIN_MATCH_SHARE: f64 = 0.4;
/// A seed that matches this share of the dots ends the search: no other seed can be
/// meaningfully better, and searching on is most of the cost.
const GOOD_ENOUGH_SCORE_SHARE: f64 = 0.9;
/// Weight of a match's distance (in gates) in the seed score. Small, so that equal match
/// counts are decided by the tighter fit.
const SLOPPY_MATCH_PENALTY: f64 = 0.01;

/// At most this many blobs, the ones that stand out most in area, are tried as anchors.
const MAX_ANCHOR_CANDIDATES: usize = 16;
/// A blob needs at least this many other blobs to be judged against ...
const MIN_ANCHOR_NEIGHBORS: usize = 4;
/// ... and its area is compared with the median of this many of its nearest blobs.
const ANCHOR_NEIGHBORS_COMPARED: usize = 8;
/// Anchor-to-neighbor area ratio of an anchor candidate. The dots' radii give a nominal ratio
/// of about 3; perspective and thresholding blur it.
const ANCHOR_AREA_RATIO: std::ops::RangeInclusive<f64> = 1.9..=5.0;

/// A candidate corner triple must have its across-to-down length ratio within this factor of
/// the grid's. Foreshortening at steep viewing angles squeezes one side of the grid, so this
/// only rejects triples that cannot be corners.
const CORNER_RATIO_SLACK: f64 = 6.0;
/// Of the six edge directions leaving the three corners, this many must have a dot beside
/// them. One may be missing.
const MIN_CORNER_EDGES_WITH_DOT: usize = 5;

/// The nearest dot along an edge, as a share of the edge's grid spacing.
const NEAREST_ALONG: f64 = 0.25;
/// The farthest dot along an edge, as a share of the edge's grid spacing.
const FARTHEST_ALONG: f64 = 3.5;
/// The smallest cosine between the edge direction and the direction to a dot along it.
const MIN_ALONG_COS: f64 = 0.8;

/// [`Matcher::plausible`] samples every this-many-th grid row and column.
const PLAUSIBLE_STRIDE: usize = 3;
/// The share of sampled dots `(numerator, denominator)` that must land on a blob. A third is
/// enough: a partly hidden screen still has to be tried.
const PLAUSIBLE_HIT_SHARE: (usize, usize) = (3, 10);

/// Cell-bucketed lookup of blob centers.
pub(super) struct PointIndex {
    cell: f64,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl PointIndex {
    pub(super) fn new(blobs: &[Blob]) -> Self {
        let cell = INDEX_CELL_PX;
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, b) in blobs.iter().enumerate() {
            cells
                .entry(Self::key(cell, b.undistorted))
                .or_default()
                .push(i as u32);
        }
        Self { cell, cells }
    }

    fn key(cell: f64, p: Point2<f64>) -> (i32, i32) {
        ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32)
    }

    /// Blobs within `radius` of `p`, with their distances.
    fn within<'a>(
        &'a self,
        blobs: &'a [Blob],
        p: Point2<f64>,
        radius: f64,
    ) -> impl Iterator<Item = (usize, f64)> + 'a {
        let span = (radius / self.cell).ceil() as i32;
        let (kx, ky) = Self::key(self.cell, p);
        (-span..=span)
            .flat_map(move |dy| (-span..=span).map(move |dx| (kx + dx, ky + dy)))
            .filter_map(|k| self.cells.get(&k))
            .flatten()
            .filter_map(move |&i| {
                let d = (blobs[i as usize].undistorted - p).norm();
                (d <= radius).then_some((i as usize, d))
            })
    }
}

struct Evaluation {
    matches: Vec<Match>,
    /// Match count minus a small penalty for sloppy matches, so equal counts
    /// prefer the tighter fit.
    score: f64,
}

pub(super) struct Matcher<'a> {
    pub pattern: &'a Pattern,
    pub blobs: &'a [Blob],
    pub index: &'a PointIndex,
}

impl Matcher<'_> {
    /// A cheap necessary condition for [`evaluate`](Self::evaluate) to find much:
    /// a sparse sample of the dots, predicted with `h`, must mostly land on
    /// blobs. It lets the seed search throw out the many wrong hypotheses
    /// without predicting and matching all the dots for each.
    fn plausible(&self, h: &Matrix3<f64>, gate: f64) -> bool {
        let p = self.pattern;
        let at = |c: usize, r: usize| apply_homography(h, p.dot_at(c, r).uv());
        let (mut tried, mut hit) = (0usize, 0usize);
        for r in (0..p.rows).step_by(PLAUSIBLE_STRIDE) {
            for c in (0..p.cols).step_by(PLAUSIBLE_STRIDE) {
                // The last row and column have no successor; use the predecessor.
                let c_next = if c + 1 < p.cols { c + 1 } else { c - 1 };
                let r_next = if r + 1 < p.rows { r + 1 } else { r - 1 };
                let (Some(here), Some(across), Some(down)) =
                    (at(c, r), at(c_next, r), at(c, r_next))
                else {
                    return false;
                };
                let spacing = (across - here).norm().min((down - here).norm());
                tried += 1;
                if self
                    .index
                    .within(self.blobs, here, gate * spacing)
                    .next()
                    .is_some()
                {
                    hit += 1;
                }
            }
        }
        hit * PLAUSIBLE_HIT_SHARE.1 >= tried * PLAUSIBLE_HIT_SHARE.0
    }

    /// Where `h` puts every dot, or `None` if `h` is degenerate or does not map the screen to a
    /// convex quad with the winding order a projection of a screen keeps.
    fn predict_dots(&self, h: &Matrix3<f64>) -> Option<Vec<Point2<f64>>> {
        let p = self.pattern;
        let predicted: Vec<Point2<f64>> = p
            .dots
            .iter()
            .map(|d| apply_homography(h, d.uv()))
            .collect::<Option<_>>()?;

        let corner = |c, r| predicted[p.index(c, r)];
        let quad = [
            corner(0, 0),
            corner(p.cols - 1, 0),
            corner(p.cols - 1, p.rows - 1),
            corner(0, p.rows - 1),
        ];
        for i in 0..4 {
            let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
            if turns_wrong_way(a, b, c) {
                return None;
            }
        }
        Some(predicted)
    }

    /// Distance from dot `(c, r)` to its nearest grid neighbor, in predicted pixels.
    fn neighbor_spacing(&self, predicted: &[Point2<f64>], c: usize, r: usize) -> f64 {
        let p = self.pattern;
        let here = predicted[p.index(c, r)];
        let mut spacing = f64::INFINITY;
        if c > 0 {
            spacing = spacing.min((predicted[p.index(c - 1, r)] - here).norm());
        }
        if c + 1 < p.cols {
            spacing = spacing.min((predicted[p.index(c + 1, r)] - here).norm());
        }
        if r > 0 {
            spacing = spacing.min((predicted[p.index(c, r - 1)] - here).norm());
        }
        if r + 1 < p.rows {
            spacing = spacing.min((predicted[p.index(c, r + 1)] - here).norm());
        }
        spacing
    }

    /// Predicts every dot with `h`, then matches each to the nearest blob
    /// within `gate` times the local dot spacing, one blob per dot.
    fn evaluate(&self, h: &Matrix3<f64>, gate: f64) -> Option<Evaluation> {
        let p = self.pattern;
        let predicted = self.predict_dots(h)?;

        // `(distance in gates, dot, blob)` of each dot's nearest blob.
        let mut candidates = Vec::new();
        for r in 0..p.rows {
            for c in 0..p.cols {
                let i = p.index(c, r);
                let radius = gate * self.neighbor_spacing(&predicted, c, r);
                if let Some((blob, d)) = self
                    .index
                    .within(self.blobs, predicted[i], radius)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                {
                    candidates.push((d / radius, i, blob));
                }
            }
        }
        // Tightest matches first, so a contested blob goes to the dot it fits best.
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut taken = vec![false; self.blobs.len()];
        let mut matches = Vec::new();
        let mut penalty = 0.0;
        for (rel, dot, blob) in candidates {
            if !taken[blob] {
                taken[blob] = true;
                matches.push(Match { dot, blob });
                penalty += rel;
            }
        }
        let score = matches.len() as f64 - SLOPPY_MATCH_PENALTY * penalty;
        Some(Evaluation { matches, score })
    }

    /// Whether the three corner candidates `[tl, tr, bl]` have dots next to them along
    /// the grid's edges. This is the lattice test that separates a pattern's corners
    /// from a bright spot that happens to stand out: a spot has no dot in two
    /// perpendicular directions nearby. One corner direction may be missing.
    fn corner_neighbors_present(&self, [a, b, c]: [Point2<f64>; 3]) -> bool {
        let across = (b - a) / (self.pattern.cols - 1) as f64;
        let down = (c - a) / (self.pattern.rows - 1) as f64;
        // Each corner and the two edge directions that leave it into the grid.
        let edges = [
            (a, across),
            (a, down),
            (b, -across),
            (b, down),
            (c, across),
            (c, -down),
        ];
        edges
            .iter()
            .filter(|(corner, step)| self.dot_along(*corner, *step))
            .count()
            >= MIN_CORNER_EDGES_WITH_DOT
    }

    /// Whether a dot lies roughly in direction `step` from `corner`, `step` being the
    /// grid spacing there. Perspective changes the spacing along an edge by a large
    /// factor and bends directions a little, so this accepts a dot at any plausible
    /// distance within a generous cone rather than at one predicted position.
    fn dot_along(&self, corner: Point2<f64>, step: Vector2<f64>) -> bool {
        let reach = step.norm();
        self.index
            .within(self.blobs, corner, FARTHEST_ALONG * reach)
            .any(|(i, d)| {
                d >= NEAREST_ALONG * reach
                    && (self.blobs[i].undistorted - corner).dot(&step) >= MIN_ALONG_COS * d * reach
            })
    }
}

/// Blobs whose area clearly exceeds that of their nearest neighbors. Comparing
/// against neighbors instead of the global median keeps perspective, which
/// shrinks far dots, from promoting near dots to anchors.
fn anchor_candidates(blobs: &[Blob]) -> Vec<usize> {
    let mut scored = Vec::new();
    for (i, b) in blobs.iter().enumerate() {
        let mut near: Vec<(f64, f64)> = blobs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, o)| ((o.undistorted - b.undistorted).norm(), o.area))
            .collect();
        if near.len() < MIN_ANCHOR_NEIGHBORS {
            continue;
        }
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut areas: Vec<f64> = near
            .iter()
            .take(ANCHOR_NEIGHBORS_COMPARED)
            .map(|n| n.1)
            .collect();
        areas.sort_by(f64::total_cmp);
        let ratio = b.area / areas[areas.len() / 2];
        if ANCHOR_AREA_RATIO.contains(&ratio) {
            scored.push((ratio, i));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(MAX_ANCHOR_CANDIDATES)
        .map(|s| s.1)
        .collect()
}

/// Index triples `(i, j, l)` below `count`, in order of their largest member, so the
/// best-ranked anchors are exhausted first and a clean view stops early.
fn triples_by_largest_member(count: usize) -> impl Iterator<Item = (usize, usize, usize)> {
    (0..count).flat_map(|newest| {
        (0..=newest)
            .flat_map(move |i| (0..=newest).flat_map(move |j| (0..=newest).map(move |l| (i, j, l))))
            .filter(move |&(i, j, l)| i.max(j).max(l) == newest)
    })
}

/// The best hypothesis so far in the seed search.
struct SeedSearch<'m, 'a> {
    matcher: &'m Matcher<'a>,
    /// Pattern positions of the grid corners `[tl, tr, bl, br]`.
    grid_corners: [Point2<f64>; 4],
    /// Width-to-height ratio of the grid in dot spacings.
    expected_ratio: f64,
    best: Option<(f64, Matrix3<f64>)>,
}

impl SeedSearch<'_, '_> {
    /// Whether the undistorted blob centers `[a, b, c]` could be the top left, top right and
    /// bottom left corner dots: the right winding, a plausible shape, and dots beside each.
    fn is_corner_triple(&self, [a, b, c]: [Point2<f64>; 3]) -> bool {
        let (across, down) = (b - a, c - a);
        if across.perp(&down) <= 0.0 {
            return false;
        }
        let ratio = across.norm() / down.norm();
        if ratio < self.expected_ratio / CORNER_RATIO_SLACK
            || ratio > self.expected_ratio * CORNER_RATIO_SLACK
        {
            return false;
        }
        // Clutter that merely looks like a corner has no neighbors, which keeps a wide anchor
        // pool cheap.
        self.matcher.corner_neighbors_present([a, b, c])
    }

    /// Tries every blob but the three in `used` as the fourth corner, keeping the hypothesis
    /// that matches the most dots. The fourth corner is not where an affine guess puts it once
    /// perspective is strong, so any blob that keeps the quad convex is a candidate; the grid
    /// evaluation picks the real one. Returns whether a hypothesis matched nearly every dot.
    fn try_fourth_corners(&mut self, used: [usize; 3], [a, b, c]: [Point2<f64>; 3]) -> bool {
        let matcher = self.matcher;
        let pattern = matcher.pattern;
        let total = pattern.dots.len();
        for (index, blob) in matcher.blobs.iter().enumerate() {
            if used.contains(&index) {
                continue;
            }
            let d = blob.undistorted;
            if turns_wrong_way(a, b, d) || turns_wrong_way(b, d, c) || turns_wrong_way(d, c, a) {
                continue;
            }
            // The fourth corner is a corner dot as well: dots lie along both of
            // the edges that leave it, toward the other two corners.
            let to_c = (c - d) / (pattern.cols - 1) as f64;
            let to_b = (b - d) / (pattern.rows - 1) as f64;
            if !matcher.dot_along(d, to_c) || !matcher.dot_along(d, to_b) {
                continue;
            }
            let Some(h) = fit_homography(&self.grid_corners, &[a, b, c, d]) else {
                continue;
            };
            if !matcher.plausible(&h, SEED_GATE) {
                continue;
            }
            let Some(eval) = matcher.evaluate(&h, SEED_GATE) else {
                continue;
            };
            if self.best.as_ref().is_none_or(|(s, _)| eval.score > *s) {
                self.best = Some((eval.score, h));
            }
            if eval.score >= GOOD_ENOUGH_SCORE_SHARE * total as f64 {
                return true;
            }
        }
        false
    }
}

/// Finds a first homography from pattern coordinates to undistorted pixels by hypothesizing
/// the grid's four corners from anchor blobs, or `None` if no hypothesis matches enough dots.
pub(super) fn find_seed(matcher: &Matcher<'_>) -> Option<Matrix3<f64>> {
    let (pattern, blobs) = (matcher.pattern, matcher.blobs);
    let (last_col, last_row) = (pattern.cols - 1, pattern.rows - 1);
    let mut search = SeedSearch {
        matcher,
        grid_corners: [
            pattern.dot_at(0, 0).uv(),
            pattern.dot_at(last_col, 0).uv(),
            pattern.dot_at(0, last_row).uv(),
            pattern.dot_at(last_col, last_row).uv(),
        ],
        expected_ratio: last_col as f64 / last_row as f64,
        best: None,
    };

    let anchors = anchor_candidates(blobs);
    for (i, j, l) in triples_by_largest_member(anchors.len()) {
        let (top_left, top_right, bottom_left) = (anchors[i], anchors[j], anchors[l]);
        if top_left == top_right || top_left == bottom_left || top_right == bottom_left {
            continue;
        }
        let corners = [
            blobs[top_left].undistorted,
            blobs[top_right].undistorted,
            blobs[bottom_left].undistorted,
        ];
        if !search.is_corner_triple(corners) {
            continue;
        }
        if search.try_fourth_corners([top_left, top_right, bottom_left], corners) {
            break;
        }
    }

    let (score, h) = search.best?;
    let min_score = (MIN_MATCH_SHARE * pattern.dots.len() as f64).max(MIN_DOTS as f64);
    if score < min_score {
        return None;
    }
    Some(h)
}

/// Refits the homography on the matches and rematches with the improved model, which also
/// pulls in dots the four-corner seed predicted loosely. `None` if too few dots end up
/// matched.
pub(super) fn refine(
    matcher: &Matcher<'_>,
    seed: Matrix3<f64>,
) -> Option<(Matrix3<f64>, Vec<Match>)> {
    let (pattern, blobs) = (matcher.pattern, matcher.blobs);
    let mut h = seed;
    let mut matches: Vec<Match> = Vec::new();
    for _ in 0..REFINE_PASSES {
        let Some(eval) = matcher.evaluate(&h, REFINE_GATE) else {
            break;
        };
        let src = dot_positions(pattern, &eval.matches);
        let dst: Vec<Point2<f64>> = eval
            .matches
            .iter()
            .map(|m| blobs[m.blob].undistorted)
            .collect();
        let Some(fit) = robust_fit(&src, &dst, inlier_threshold(pattern, &h)) else {
            break;
        };
        let changed = fit.inliers.len() != matches.len();
        h = fit.h;
        matches = fit.inliers.iter().map(|&i| eval.matches[i]).collect();
        if !changed {
            break;
        }
    }
    let min_matches = (MIN_MATCH_SHARE * pattern.dots.len() as f64) as usize;
    if matches.len() < min_matches || matches.len() < MIN_DOTS {
        return None;
    }
    Some((h, matches))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples_are_visited_best_ranked_members_first() {
        let triples: Vec<_> = triples_by_largest_member(3).collect();
        assert_eq!(triples.len(), 27);
        assert_eq!(triples[0], (0, 0, 0));
        // Ordered by largest member; within one, by (i, j, l).
        let largest: Vec<_> = triples.iter().map(|&(i, j, l)| i.max(j).max(l)).collect();
        assert!(largest.is_sorted());
        assert_eq!(triples[1], (0, 0, 1));
        assert_eq!(triples[7], (1, 1, 1));
    }
}

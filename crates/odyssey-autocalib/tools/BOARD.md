# The scale board (optional, prototype)

**Status.** A prototype to find out whether a printed board of known size can
correct the depth camera's size error. Nothing in the calibration wizard uses it
yet. What exists is the print artwork, a detector, and a probe that reports what
the board shows on real hardware. No accuracy is claimed until the probe has been
run with a real print at real distances.

## What it is, in plain words

A sheet with 34 black dots on it. You stand it next to the screen, where the camera
can see both. The dots are exactly 40 mm apart, so the camera can work out how far
away the sheet is from how big it looks, and compare that with what its depth sensor
says. If the two differ, the depth sensor's sense of scale is off by that much, and
the screen's measured size can be corrected by the same amount. Nobody touches the
screen and nobody lines anything up by eye.

It is optional: calibration works without it.

## Pick a board (start with the Letter paper one)

The camera has to see each dot at least **12 pixels across** in its color image
(8 to 12 is marginal, under 8 is too small, and around 4 or less is not found at
all, in the detector's synthetic tests). The probe prints the dot size it saw and
says which of those it is. Use the large board only if the probe says the dots on the
standard board are too small at your distance; otherwise the standard one is fine.

| File | Size | Dots | Use |
| --- | --- | --- | --- |
| `scale-board-paper-letter.pdf` | US Letter landscape | 20 mm, 40 mm pitch | quick: any office printer |
| `scale-board-paper-a4.pdf` | A4 landscape | 20 mm, 40 mm pitch | quick: any office printer |
| `scale-board-rigid.pdf` | 12 × 8.5 in | 20 mm, 40 mm pitch | best: printed on rigid board |
| `scale-board-rigid-large.pdf` | 24 × 17 in | 40 mm, **80 mm pitch** | alternate: only if the standard dots read too small. Always pass `--pitch-mm 80` (never 40) |

All four are vector PDFs at exact size (the SVGs match). With no arguments the generator
writes only the Letter one; regenerate the whole set with
`python3 tools/board_artwork.py --all --out-dir tools/artwork`. The geometry is in
`board.json` and nothing else.

## Option A, quick: paper on foam board

1. Open the PDF in a normal PDF viewer (Adobe Reader, Preview, a browser) and print it.
   - **Scale: Actual size / 100%.** Never "Fit", "Shrink to printable area" or
     "Fit to page": that changes the size of everything.
   - Paper size: the one the file is for (Letter or A4). Orientation: landscape.
   - Black and white or grayscale is fine. Quality: best or high.
   - Plain white matte paper. Not glossy: it glares and washes the dots out.
2. Glue it flat onto a piece of 3/16 in foam board cut at least as big as the paper.
   Use spray adhesive or full-sheet double-sided tape, working from the middle out and
   rolling it smooth. Avoid wet glue, which makes paper ripple. Leave it flat under a
   weight for an hour. A wavy or curled sheet is the thing most likely to spoil it.
3. Check the scale (below).

Office printers and paper can be off in size by a fraction of a percent or more, which
is why the check matters.

## Option B, best: print on rigid board (foamcoreprint.com)

Their gator board printing is direct UV print on a matte face, in custom sizes, from
4 × 4 in up to 48 × 96 in, with an online proof. Their page lists 3/16 in and 1/2 in
thicknesses and says it resists denting and bending.

1. foamcoreprint.com, **Gator Board Printing**, **Upload Your Artwork**.
2. Upload `scale-board-rigid.pdf` (or `-large.pdf`).
3. Custom size **12 × 8.5 in** (large: **24 × 17 in**), thickness **3/16 in**, one side,
   no holes, grommets or cut-outs.
4. At the proof, check that the size reads exactly as above, the dots are black on
   white and none is cropped, and nothing was added or scaled. The file fills the board
   white on purpose, because their page lists a black panel colour.
5. When it arrives, check the scale (below).

I have not ordered one, so their dimensional accuracy is unverified. Do not use plain
foam core: it dents and bows.

## Showing it on a screen instead

A tablet or monitor can show the board instead of a print, and is rigid and flat for
free. Make a page the size of its screen, render it at the screen's exact resolution, and
show it at 1:1 with no zoom, fit or stretch. The physical size of such a page comes from
the screen's nominal diagonal, so it is only as accurate as that is: always do the scale
check below. A reflective or no-backlight screen (e-ink) needs room light on it. This is
not one of the official files; keep any particular screen's settings in a local script.

## Check the scale (do this once per print)

Measure the printed bar from the middle of its left end tick to the middle of its right
end tick with a steel rule or a tape. It should read **200 mm** (7.87 in), or **400 mm**
(15.75 in) on the large board. Write down what you measure. If it is off, do not fix the
printer. The probe's `--pitch-mm` takes the pitch between dots, not the bar length, so
convert:

    pitch_mm = 40 × measured_bar_mm ÷ 200        (standard and paper boards)
    pitch_mm = 80 × measured_bar_mm ÷ 400        (large board)

Worked example: the bar on a Letter print measures 201.4 mm, so the pitch is
`40 × 201.4 ÷ 200 = 40.28` and you run `board_probe -- --pitch-mm 40.28`. The board's
distance is proportional to the pitch, so passing 40 for that print would read the board
0.7% too far.

## Try it

With the board still and flat in view, the whole dot grid visible, and the camera
connected:

    cargo run -p odyssey-autocalib --example board_probe -- --pitch-mm 40

(`--pitch-mm 80` for the large board, or the corrected value from the scale check.) It
grabs about 30 frames and prints the dots found, their size in pixels, the board's
distance from its apparent size, the depth sensor's distance for the same board, the
ratio, and whether the board was big enough and steady enough to trust. Try several
distances and angles and keep the numbers: that is the evidence this prototype exists to
collect.

## What is not built

- Sizing the screen from the board. That is the intended use: stood in the screen's
  plane, the board gives a metric scale from the camera's own picture, independent of
  depth and the EDID. Today the probe only compares the board's distance with depth.
- A Setup step that uses the board in the wizard. The app has an explanation window
  (Setup, "Check the camera's depth scale", "About the scale board") with the four PDFs, each with "Open to print" (opens
  it in the system's PDF viewer) and "Save a copy…" (a Save As dialog). It is information
  only. The PDFs it carries are the ones in `tools/artwork`, so regenerate and rebuild
  together. The tablet PNG is not in the app: it comes only from a local script.
- A correction of the calibration from the board's result.
- A characterization of how accurate it is. So far the detector has run on one camera
  at three distances against a tablet showing the board, with one tape-measured point.

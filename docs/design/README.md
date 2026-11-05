# Odyssey Arm style guide

One look for everything with the Odyssey Arm name on it: the website, Odyssey
Desktop, the SDK docs, firmware-facing tools and the games. The values live in
[`tokens.css`](./tokens.css); this page says what they are for.

| Surface | Where the tokens come in |
|---|---|
| Odyssey Desktop | `crates/odyssey-desktop/input.css` imports `tokens.css` and maps it to Tailwind utilities and component classes |
| Website and docs ([odysseyarm.github.io](https://github.com/odysseyarm/odysseyarm.github.io)) | `src/styles/tokens.css` and `site.css` carry the same values; copy changes there |
| SDK diagrams (`docs/sdk/images/`) | The `--oa-dg-*` variables in the site's `docs.css` use the same palette |
| Unity games and demos | Copy the values into a USS variables file or a C# palette, see [Games](#games) |

Change a value in `tokens.css` first, then in the copies, in the same week.

## Character

Precise, calm, warm. Odyssey is aiming hardware, so the brand borrows from it:
a reticle in the emblem, a dark range-like backdrop, one hot orange for the
thing to look at. Screens stay quiet so the target, the camera picture or the
next action stands out.

- Warm charcoal, never blue-gray or pure black, for backgrounds.
- Orange marks the one thing to do or look at. If everything is orange, nothing is.
- Rounded and soft-edged: pill buttons, 10–14 px corners, thin borders over heavy shadows.
- Short, plain words. Name the action ("Pair dongle"), not the concept ("Pairing").

## Logo

The emblem is a reticle (ochre) around a keyhole "A" (flame). The files are in
the website repository, `src/assets/images/`:

| File | Use |
|---|---|
| `emblem.png` | Emblem alone: app icons, favicons, splash screens, small spaces |
| `logo-white.png` | Emblem and white wordmark, on charcoal or photos |
| `logo-dark.png` | Emblem and dark wordmark, on light backgrounds |

- Keep clear space of at least half the emblem's width on every side.
- Never recolour, outline, rotate, stretch or put a shadow behind it.
- Never put the emblem on orange: the ochre ring disappears.
- Below 24 px, use the emblem alone, without the wordmark.
- In running text and UI chrome, the product is "Odyssey" and the company is
  "Odyssey Arm". Never "ODYSSEY" or "OdysseyArm" (the second only in
  identifiers such as `com.odysseyarm.odyssey`).

## Colour

### Brand

| Token | Value | Use |
|---|---|---|
| `--oa-brand-orange` | `#ffa44f` | The accent. Primary buttons, current item, focus |
| `--oa-brand-charcoal` | `#272525` | Dark background |
| `--oa-brand-ochre` | `#c2852c` | Emblem ring. Illustration only, never UI state |
| `--oa-brand-flame` | `#ff8344` | Emblem "A". Illustration only, never UI state |

### Roles

Use roles, never raw values. Every role has a light and a dark value, and
`tokens.css` switches them by the system setting (or `data-theme` on `<html>`).

| Role | Light | Dark | Use |
|---|---|---|---|
| `canvas` | `#faf8f7` | `#272525` | Page background |
| `surface` | `#ffffff` | `#2f2c2c` | Cards, panels, dialogs |
| `surface-alt` | `#f1eeed` | `#1f1d1d` | Sunk areas: inputs, sidebar, table headers, code |
| `surface-hover` | `#e9e5e4` | `#3a3636` | Hovered rows and ghost buttons |
| `line` | `#e5e0df` | `#45403f` | Borders and dividers |
| `line-strong` | `#cfc9c8` | `#5f5957` | Outline-button borders |
| `fg-strong` | `#111111` | `#ffffff` | Headings, key values |
| `fg` | `#2b2727` | `#e5e0df` | Body text |
| `fg-muted` | `#5f5957` | `#b5b0af` | Secondary text, captions |
| `fg-faint` | `#857f7e` | `#857f7e` | Placeholders, disabled, input edges. Not for text that must be read |
| `accent` | `#ffa44f` | `#ffa44f` | Orange fill (always with `on-accent` text) |
| `accent-strong` | `#ffb36b` | `#ffb36b` | Hovered orange fill |
| `on-accent` | `#111111` | `#111111` | Text and icons on orange |
| `accent-ink` | `#994a00` | `#ffa44f` | Orange text, links, icons, focus outline |
| `accent-soft` | `#ffe4c9` | `#3d2510` | Tint behind the current or selected item |

Text on orange is near-black, not white: white on `#ffa44f` is 2.0:1 and fails;
`#111111` is 9.6:1.

### Status

Each status has a fill, an `-ink` for text on neutral or soft backgrounds, and
a `-soft` tint. Status colours mean something; never use them for decoration.
Only the warning and danger fills carry text (with the colour given); success
and info fills are for dots, shapes and overlays.

| Status | Fill | Ink (light / dark) | Soft (light / dark) | Means |
|---|---|---|---|---|
| `success` | `#1a7f37` / `#2ea043` | `#176f30` / `#4ac26b` | `#e3f6e8` / `#16301e` | Done, connected, good result |
| `warning` | `#fbbf24` (text `#1c1917`) | `#8a5a00` / `#fbbf24` | `#fff4d6` / `#3a2e10` | Needs attention, fair result, in progress with a catch |
| `danger` | `#c4302b` (text white) | `#b42823` / `#ff7b72` | `#fde8e7` / `#3d1a19` | Failed, destructive, poor result |
| `info` | `#0b5cad` / `#6cb6ff` | `#0b5cad` / `#6cb6ff` | `#e5f1ff` / `#15263b` | Neutral hint, explanation |

Over a camera picture or a game scene, draw status in the fill colours with a
dark outline or backdrop; the soft and ink values are for UI surfaces.

### Data

| Token | Value | Use |
|---|---|---|
| `--oa-data-x` | `#ff7b72` | X axis, first series |
| `--oa-data-y` | `#4ac26b` | Y axis, second series |
| `--oa-data-z` | `#6cb6ff` | Z axis, third series |

### Contrast

Every text role above meets WCAG AA (4.5:1) on `canvas`, `surface`,
`surface-alt` and `surface-hover` in both themes, except `fg-faint`, which is
for non-text use and placeholders. Check a new pair before shipping it.

## Type

- **Inter** (variable, weights 100–900), with the system UI font as fallback.
  Ship the font with the product; the desktop app bundles the Latin subset
  (`crates/odyssey-desktop/assets/fonts/`, SIL OFL 1.1), because the offline
  installer never fetches anything. Monospace: the platform's UI monospace.
- Headings: bold, tracking `-0.03em`, balanced wrapping, `fg-strong`.
- Body: 15–17 px, line height 1.5–1.6. Desktop UI text is 14 px (`text-sm`)
  with 12 px (`text-xs`) for captions and badges.
- Numbers that update live (battery, distances, counters) use tabular figures
  (`tabular-nums`) so they do not jitter.
- Small uppercase labels ("eyebrows") are 12 px, semibold, letter-spaced, `fg-muted`.
- Sentence case everywhere, buttons included: "Save zero", not "Save Zero".

## Shape, space and depth

| Token | Value | Use |
|---|---|---|
| `--oa-radius-sm` | 6 px | Small inner elements |
| `--oa-radius` | 10 px | Inputs, menus, small panels |
| `--oa-radius-lg` | 14 px | Cards, alerts, camera stages |
| `--oa-radius-xl` | 22 px | Hero panels, large media |
| (pill) | 999 px | Buttons, badges, status dots |

- Space in steps of 4 px; cards pad 20 px, pages 24–32 px.
- Separate with a 1 px `line` border before reaching for a shadow. Cards get at
  most a small shadow; only dialogs float.
- Dialog backdrops: black at 60 %.

## Motion

- Ease `--oa-ease-out` (`cubic-bezier(0.22, 1, 0.36, 1)`), 150 ms for colour
  and hover, 200–300 ms for movement, up to 1 s for reveal-on-scroll.
- Buttons with a fill lift by 1 px on hover.
- Honour reduced motion: no lifts, reveals or looping animations when the
  system asks for less motion. Status pulses are the exception only when they
  are the sole sign that something is live.

## Components

The desktop's component classes are the reference implementation
(`crates/odyssey-desktop/input.css`, `@layer components`). The website's
`.button` and `.button.secondary` follow the same rules.

| Component | Desktop class | Rules |
|---|---|---|
| Primary button | `btn btn-primary` | Orange pill, near-black label. One per view or row: the action the user came for |
| Secondary button | `btn btn-secondary` | Outline pill in `line-strong`. Everything else |
| Destructive button | `btn btn-danger` | Red fill, white label. Only to confirm something that cannot be undone; otherwise a secondary button with `danger-ink` text |
| Ghost button | `btn btn-ghost` | No border. Toolbars and inline actions inside content |
| Sizes | `btn-sm`, `btn-lg` | Small in dense rows, large in the calibration wizard |
| Link | `link` | `accent-ink`, underlined |
| Card | `card`, `card-title` | `surface`, `line` border, 14 px corners, 20 px padding |
| Field | `field`, `field-invalid`, `label`, `help` | `surface-alt` fill, `fg-faint` edge, orange edge when focused, danger edge when invalid |
| Badge | `badge badge-{neutral,accent,success,warning,danger,info}` | Soft pill with ink text |
| Alert | `alert alert-{info,success,warning,danger}` | Soft fill, faint border in the status colour, ink text |
| Eyebrow | `eyebrow` | Small uppercase label above a title |
| Stage | `stage` | Black, rounded area for camera pictures and captured images in either theme |
| Progress | `progress`, `progress-fill` | `surface-alt` track, orange fill, pill ends |
| Focus | (global) | 2 px `accent-ink` outline, 2 px offset, on every focusable element. Never remove it |
| Navigation | sidebar | `surface-alt`; current page in `accent-soft` with `accent-ink` text |

Tailwind's stock palette is switched off in the desktop app, so `gray-500` or
`blue-600` produce no CSS. To find stray colours:

```sh
cd crates/odyssey-desktop/src
grep -rnE '\b(dark:|(bg|text|border|ring|fill|stroke)-(gray|blue|red|amber|yellow|green|emerald|sky|purple|orange|slate)-[0-9]+)' .
```

Things that are not branded, on purpose: the calibration patterns the camera
photographs (pure black and white, exact geometry), crosshair images and the
zero target.

## Writing

- Say what happened and what to do next: "The dongle did not answer. Unplug it
  and plug it back in."
- Use the user's words: screen, dongle, device, zero. Not "endpoint", "PID",
  "transport" unless the screen is for developers.
- Units take a space, percent does not: "3.2 m", "95%".
- No exclamation marks in errors. No blame ("You entered…").

## Games

Unity projects use UI Toolkit. Put the roles in one USS file and reference the
variables from every stylesheet:

```css
:root {
    --oa-canvas: #272525;
    --oa-surface: #2f2c2c;
    --oa-line: #45403f;
    --oa-fg: #e5e0df;
    --oa-fg-strong: #ffffff;
    --oa-fg-muted: #b5b0af;
    --oa-accent: #ffa44f;
    --oa-on-accent: #111111;
    --oa-success: #2ea043;
    --oa-warning: #fbbf24;
    --oa-danger: #c4302b;
}
```

- Menus default to the dark theme: play usually happens in a dim room in
  front of a projector. A game's art direction may choose light menus, and a
  light or high-contrast option for accessibility is welcome.
- HUD text over the scene always sits on a dark backing (`canvas` at 70–80%
  opacity) or carries a dark outline, so it reads over any background.
- Orange is for the player's own reticle, score and the next action; status
  colours keep their meaning (hit: success, miss: danger).
- Menus follow the desktop components: pill buttons, one primary per screen,
  14 px card corners.
- Use Inter through a TextMesh Pro or UI Toolkit font asset; keep the OFL
  licence with it.

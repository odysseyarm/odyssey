# Documentation

| Directory | Audience | Published |
|---|---|---|
| `sdk/` | Developers building on Odyssey with the client SDK | [odysseyarm.com/developers](https://odysseyarm.com/developers/) |
| `contributing/` | People working on this repository | Here on GitHub only |
| `design/` | Anyone designing for Odyssey Arm: website, apps, games ([style guide](design/README.md), [tokens](design/tokens.css)) | Here on GitHub only |

The website ([odysseyarm/odysseyarm.github.io](https://github.com/odysseyarm/odysseyarm.github.io))
copies `sdk/` in when it builds. Docs for people who use Odyssey without
building on it are written on the website itself.

## Hosted Rust API

The website serves public `odyssey-protocol` and `odyssey-hub-client` rustdoc at
`/api/master/` and `/api/<Desktop version>/`. `/api/latest/` links to the highest
documented stable Desktop version. Master is unreleased.

`.github/workflows/rustdoc.yml` builds from the standalone FFI workspace so Cargo
does not resolve the root workspace's private tracking dependency. It documents
the two Rust crates without dependency documentation or private items. The full
fresh rustdoc output includes its shared scripts, styles and search indexes.

Master builds upload the `rustdoc-master` Actions artifact containing
`rustdoc-master.tar.gz`. A weekly build refreshes it within its 30-day retention
period. Stable Desktop releases attach `rustdoc-X.Y.Z.tar.gz` to the matching
release. The Desktop release workflow calls this workflow explicitly because a
release created with `GITHUB_TOKEN` does not trigger another release workflow.
The website imports these archives and deploys through its existing Cloudflare
workflow.

To bootstrap an existing stable release, run the Rust API docs workflow manually
with its `version` and matching `odyssey-desktop-vX.Y.Z` tag. This uploads only
documentation; it does not republish application or firmware binaries. Publish
master and the latest stable archive before merging the website importer.

For local documentation, enter the development shell and run:

```sh
cargo doc --locked --offline --manifest-path sdk/odyssey-hub-client-ffi/Cargo.toml \
  --no-deps -p odyssey-protocol -p odyssey-hub-client --open
```

## Writing SDK pages

Pages are GitHub-flavored Markdown and must read well here on GitHub. The site
turns a few conventions into richer components, and on GitHub those same
conventions fall back to plain Markdown.

| Write | GitHub shows | The site shows |
|---|---|---|
| A single `# Title` at the top | The heading | The page title |
| `<!-- order: 2 -->` above the title | Nothing | Position in the sidebar |
| `<!-- sidebar-label: Overview -->` | Nothing | Sidebar text, when it should differ from the title |
| `<!-- description: One sentence. -->` | Nothing | Search and link previews |
| `> [!NOTE]`, `> [!TIP]`, `> [!IMPORTANT]`, `> [!WARNING]`, `> [!CAUTION]` | An alert | A callout |
| `![Alt text](./images/name.svg "Caption")` on its own line | The image | The diagram inline, themed for light and dark mode, with the caption |
| `<!-- tabs: lang -->`, then `**Label**` lines each followed by content, then `<!-- /tabs -->` | Each label and its content in turn | Tabs. Pages using the same key (`lang`) remember the reader's choice |
| `<!-- cards -->`, a list of `- [Title](./page.md): description`, then `<!-- /cards -->` | The list | Link cards |
| `<!-- steps -->`, a numbered list, then `<!-- /steps -->` | The list | Numbered steps |

A few more rules.

Link to other pages with relative paths and the `.md` extension, as in
`[Zeroing](./zeroing.md)`. The site rewrites them. Anything outside `sdk/` gets
a full GitHub URL.

Keep raw HTML, curly braces and `<` out of prose, because the site compiles
pages as MDX. Put them in backticks.

API details belong in doc comments, where they are defined, which leaves these
pages to explain concepts and show how the pieces fit.

## Diagrams

The SVGs in `sdk/images/` come out of
[`sdk/images/generate.py`](sdk/images/generate.py). Edit the script and rerun
it, and leave the SVGs themselves alone. Its header says what it needs. The
colors come from CSS variables
(`--oa-dg-fg`, `--oa-dg-accent`, …) so the site can theme them, and when GitHub
shows a diagram as a plain image it falls back to light or dark colors by
`prefers-color-scheme`. New diagrams keep that pattern: no external fonts, no
`id` attributes, and one root `class="odg"`.

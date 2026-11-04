# Documentation

This directory is the source for the developer documentation published at
[odysseyarm.com/docs](https://odysseyarm.com/docs). It lives next to the code so
that a change to an API and the change to its documentation land in the same
pull request.

| Directory | Audience | Published at |
|---|---|---|
| `sdk/` | Developers integrating Odyssey through the client SDK (C, C++, C#, Python, Rust) | `/docs/sdk/` |
| `contributing/` | People working on this repository | `/docs/contributing/` |

Product guides for end users, the blog and the rest of the website live in
[odysseyarm/odysseyarm.github.io](https://github.com/odysseyarm/odysseyarm.github.io).
That site copies this directory in when it builds (`npm run sync-docs` there).

## Writing pages

- Plain GitHub-flavored Markdown. Pages must read correctly on GitHub as well as
  on the site, so do not use MDX or site-specific components.
- Start each page with a single `# Title`. The site uses it as the page title.
  Front matter is optional; add it only for site options such as
  `sidebar: { order: 2 }`.
- Link to other pages with relative paths, including the `.md` extension
  (`[Coordinate systems](./coordinate-systems.md)`). The site rewrites these to
  its own URLs.
- Link to code outside `docs/` with a full GitHub URL so the link also works on
  the website.
- An `index.md` in a directory is that section's landing page. This `README.md`
  is not published.
- API reference is generated from doc comments, not written here. Document a
  type or function where it is defined.

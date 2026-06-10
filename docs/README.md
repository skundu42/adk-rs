# adk-rs documentation site

A standalone Next.js application documenting the [`adk-rs`](https://github.com/skundu42/adk-rs) crate — every module, feature flag, example, and guide.

This folder is **independent of the Rust workspace**: it is not a workspace member, and the crate's `Cargo.toml` explicitly `exclude`s `/docs`, so nothing here is ever bundled into a `cargo publish` release.

## Develop

```sh
cd docs
npm install
npm run dev      # http://localhost:3000
```

## Build (static export)

```sh
npm run build    # emits a fully static site into docs/out/
```

The output in `out/` can be served from any static host.

## Layout

- `app/` — Next.js App Router shell (landing page, docs layout, catch-all docs route)
- `content/` — one typed TypeScript data file per documentation page
- `lib/` — page registry, block types, syntax highlighter
- `components/` — block renderer, code blocks, sidebar, table of contents

To add a page: create `content/<slug>.ts` exporting a `DocPage`, then register it in `lib/registry.ts`.

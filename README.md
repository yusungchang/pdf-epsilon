# PDF Epsilon

**Make PDFs smaller. Keep what matters.**

A desktop app for macOS, Linux and Windows that explains why a PDF is large and
shrinks it on your own computer. Nothing is uploaded anywhere. Drop a PDF on the window
to see where its bytes go, page by page, and what each preset would save. Then save a
smaller copy, named `<name>-epsilon.pdf`. The original is never changed.

## Analysis

Before anything is changed, PDF Epsilon shows:

- **Where the bytes go:** images, fonts, text and drawing, metadata, accessibility tags,
  old versions and more. Every object is counted once, so the parts add up to the file size.
- **What each preset saves,** measured by running all four on the file in memory.
- **What was found:** oversized images, images mostly outside the page, fonts embedded in
  full or more than once, duplicates, thumbnails, editor data, incremental saves, and images
  it cannot handle yet. Each finding says whether a preset fixes it, and by how much.
- **Page by page:** each page's share of the file, what it is made of, its issues, and every
  image on it with its resolution as drawn and what the chosen preset makes of it.
- **Warnings** for signed documents and PDF/A files.

See [docs/ANALYZER.md](docs/ANALYZER.md) for how it works.

Website: <https://pdfepsilon.com> (domain not yet registered).
Source: <https://github.com/yusungchang/pdf-epsilon>.

## What it does

1. **Downsamples oversized images.** Each image's resolution is measured from the
   size it is actually drawn on the page, not guessed from the page size.
2. **Re-encodes photographs as JPEG** at the chosen quality. Flat graphics such as
   screenshots, charts and transparency masks stay lossless.
3. **Merges identical streams.** A logo repeated on every page is stored once.
4. **Removes clutter:** page thumbnails, and optionally editor private data and XMP metadata.
5. **Repacks the file:** drops unused objects, deflates streams and packs objects
   into compressed object streams.

Text, fonts, vector drawing, links, bookmarks, forms, annotations and attachments are
kept. Every result is re-opened and its page count checked before it is saved. If the
result is not smaller, nothing is written.

## Presets

| Preset   | Images kept up to | JPEG quality | Also removes                 |
|----------|-------------------|--------------|------------------------------|
| Lossless | untouched         | –            | thumbnails                   |
| Print    | 300 dpi           | 85           | thumbnails                   |
| Balanced | 150 dpi           | 75           | thumbnails, editor data      |
| Small    | 96 dpi            | 60           | thumbnails, editor data, XMP |

Every setting can be changed under **Advanced**.

## Not yet supported

- **Encrypted PDFs** are refused, including ones with only an owner password.
- **Fonts are not subsetted.** Files dominated by embedded fonts, common with CJK text,
  shrink very little.
- **Some images are left as they are:** CMYK, indexed, spot colour, 1-bit and 16-bit
  images, JPEG 2000, JBIG2 and CCITT.

## Develop

Requirements are Rust (stable) and Node.js. On Linux, also install the
[Tauri system libraries](https://v2.tauri.app/start/prerequisites/).

```sh
npm install
npm run dev      # run the app
npm run build    # build installers into target.nosync/release/bundle/
cargo test -p epsilon --release
```

Try the engine on a file without the app:

```sh
cargo run -p epsilon --release --example inspect -- in.pdf --pages   # analysis, add --json for raw
cargo run -p epsilon --release --example squeeze -- in.pdf out.pdf balanced
```

## Layout

- [docs/HANDOVER.md](docs/HANDOVER.md) has the current state and next steps.
  [docs/HISTORY.md](docs/HISTORY.md) lists the versions.

- [crates/epsilon/](crates/epsilon/) is the analysis and compression engine, a plain Rust library.
- [src-tauri/](src-tauri/) is the desktop shell. Its binary is `pdf-epsilon`.
- [ui/](ui/) is the interface: plain HTML, CSS and JavaScript with no build step.
- [assets/icon.svg](assets/icon.svg) is the icon source. Regenerate the platform
  icons with `npx tauri icon assets/icon.svg -o src-tauri/icons`.

## Releases

Every push to `main` makes GitHub Actions run the tests and build installers for every
platform. They can be downloaded from the run's artifacts for 14 days. Pushing a tag such as
`v0.2.0` builds the same installers into a draft release: `.dmg` and `.app` for macOS, `.msi` and `.exe` for
Windows, and `.deb`, `.rpm` and `.AppImage` for Linux. The builds are not code-signed
yet. macOS Gatekeeper and Windows SmartScreen will warn on first launch.

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
- **Page by page:** a thumbnail of each page, its share of the file, what it is made of, its
  issues, and every image on it with its resolution as drawn and what the chosen preset makes
  of it.
- **Compare** shows a page as it is and as the current settings will save it: a divider to drag
  across, side by side, or with every visibly changed pixel marked, at Fit, 2× or 4×, in a window
  you can resize. Saving afterwards writes exactly the version compared.
- **Light, dark or system theme**, and English or Korean, from the two buttons at the top.
- **Progress** while a file is analysed. Several files can be dropped at once; × takes one off
  the list without touching the file.
- **Warnings** for signed documents and PDF/A files.

See [docs/ANALYZER.md](docs/ANALYZER.md) for how it works.

## Languages

The interface is in English and Korean. It follows the system language, and there is a
picker in the top corner. Every word shown lives in [ui/locales/](ui/locales/), one JSON
file per language. The engine writes no text of its own; it sends message keys and values,
and the interface fills them in for the chosen language.

To add a language:

1. Copy `ui/locales/en.json` to `ui/locales/<code>.json`, for example `ja.json`, and translate
   the values. Keep every `{placeholder}`. A value can be a plain string or, where it has a
   `{count}`, an object of plural forms such as `{"one": "...", "other": "..."}`, following
   the language's [plural rules](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html).
2. Add the language to `LANGUAGES` at the top of `ui/app.js`, under its own name.
3. Run `cargo test -p epsilon --release`. It checks that every language has exactly the keys
   and placeholders of English.

Website: <https://pdfepsilon.com> (domain not yet registered).
Source: <https://github.com/yusungchang/pdf-epsilon>.

## What it does

1. **Downsamples oversized images.** Each image's resolution is measured from the
   size it is actually drawn on the page, not guessed from the page size.
2. **Re-encodes photographs as JPEG** at the chosen quality. Flat graphics such as
   screenshots, charts and transparency masks stay lossless.
3. **Slims embedded fonts.** Fonts embedded in full keep only the glyphs the pages draw,
   and every font loses data no PDF viewer reads, such as screen bitmaps. Text looks the
   same and stays searchable. A 15 MB test file with a whole CJK font became 80 KB.
4. **Merges identical streams.** A logo repeated on every page is stored once.
5. **Removes clutter:** page thumbnails, and optionally editor private data and XMP metadata.
6. **Repacks the file:** drops unused objects, deflates streams and packs objects
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
- **Only some fonts are subset:** Type 0 TrueType fonts with Identity encoding, which is how
  Word, browsers and most CJK documents embed them. Fonts that form fields can type with are
  left whole. Simple TrueType, Type 1 and CFF fonts are not subset yet.
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
  [docs/SECURITY.md](docs/SECURITY.md) explains how hostile PDFs are handled.
  [reports/PDF size optimization techniques.md](reports/PDF%20size%20optimization%20techniques.md)
  ranks further optimizations by savings and feasibility.
  [docs/HISTORY.md](docs/HISTORY.md) lists the versions.

- [crates/epsilon/](crates/epsilon/) is the analysis and compression engine, a plain Rust library.
- [src-tauri/](src-tauri/) is the desktop shell. Its binary is `pdf-epsilon`.
- [ui/](ui/) is the interface: plain HTML, CSS and JavaScript with no build step.
- [assets/icon-art.png](assets/icon-art.png) is the icon artwork, a crushed aluminium can marked PDF.
  [assets/icon.svg](assets/icon.svg) clips it to the macOS rounded square. Regenerate the platform
  icons with `npx tauri icon assets/icon.svg -o src-tauri/icons`, then delete the `android` and `ios`
  folders it creates and copy `src-tauri/icons/128x128@2x.png` to `ui/icon.png`.

## Releases

Every push to `main` makes GitHub Actions run the tests and build installers for every
platform. They can be downloaded from the run's artifacts for 14 days. Pushing a tag such as
`v0.2.0` builds the same installers into a draft release: `.dmg` and `.app` for macOS, `.msi` and `.exe` for
Windows, and `.deb`, `.rpm` and `.AppImage` for Linux. The builds are not signed with a
developer certificate yet. On macOS, open the app once from System Settings, Privacy & Security,
Open Anyway; Windows SmartScreen asks for More info, Run anyway.

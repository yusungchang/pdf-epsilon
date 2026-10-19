# Handover

Where PDF Epsilon stands, what has been verified, and what to do next.
Last updated 19 October 2026, at version 0.3.0.

## The brief

These came from the owner at the start:

- **Name:** PDF Epsilon. The GitHub repository is `pdf-epsilon`, and the executable is `pdf-epsilon`.
- **Tagline:** "Make PDFs smaller. Keep what matters."
- **Website:** pdfepsilon.com, if available.
- **Platform:** a Tauri app installed locally on macOS, Linux and Windows.
- **Location:** iCloud Drive/Projects, next to msw and the owner's other projects.

## Current state

The app works end to end on macOS. It builds into `PDF Epsilon.app` and a `.dmg`, launches, and runs the engine.
Since 0.2.0 the analyzer is the front of the app: dropping a PDF shows where its bytes go, what each preset
saves (measured), what was found, and a page-by-page breakdown. Saving is a button inside that report.
See `ANALYZER.md`.
The code is public at https://github.com/yusungchang/pdf-epsilon, branch `main`.

| Part | State |
|---|---|
| Engine (`crates/epsilon`) | Done. Unit and integration tests pass. |
| Analyzer (`analyze.rs`) | Done. Checked on the eight test files; categories add up to the exact size. |
| Desktop shell (`src-tauri`) | Done. Release build verified on macOS, Apple silicon. |
| Interface (`ui`) | Done. Checked in headless Chrome with a mocked bridge serving real analyses. Not yet clicked through in the real app window. |
| Font optimizer (`fonts.rs`, `subset.rs`) | Done for TrueType CID fonts. Checked by text and pixel comparison in MuPDF and macOS. |
| Languages (`ui/locales`) | English and Korean. Every string is in the locale files; tests check they match. |
| CI and releases (`.github/workflows/build.yml`) | Builds installers for all three platforms on every push to `main` and on `v*` tags. First run started on 19 October 2026. |
| Windows and Linux | Configured, never built or run. |

## Environment on the owner's Mac

- **Toolchains:** Rust 1.99 and Node 26, both installed with Homebrew for this project. The Tauri CLI comes from npm.
- **Dependencies:** lopdf 0.45, image 0.25, and Tauri 2.12 with the dialog and opener plugins.
  Tauri 3 is in alpha. Stay on 2 until it is stable.
- **Checking tools:** `mutool` (MuPDF) is installed and useful for comparisons. Ghostscript and qpdf are not installed.
- **GitHub:** the GitHub CLI `gh` is installed with Homebrew and logged in as `yusungchang`, with the `repo` and
  `workflow` scopes. Git uses it for HTTPS. Commits use the GitHub noreply address
  `19603908+yusungchang@users.noreply.github.com`, set in this repository's git config.
- **iCloud:** build output goes to `target.nosync`, and `node_modules` is a symlink to `node_modules.nosync`.
  iCloud does not sync folders ending in `.nosync`, and the Rust build folder reaches about 1.5 GB.
- **Screenshots:** screen capture permission is not granted to the terminal, so the real app window cannot be screenshotted.
  Use `tools/ui-preview/preview.sh` to see the interface instead.

## Design decisions

- **Pure Rust engine, no bundled tools.** Ghostscript and MuPDF are AGPL, which complicates bundling, and qpdf would be an
  extra binary on every platform. lopdf is MIT licensed and builds everywhere Rust does.
- **Resolution comes from drawn size.** `placement.rs` follows each page's content stream through `q`, `Q`, `cm` and
  `Do`, including nested forms, and records the largest size at which each image is drawn. Images that are never found
  drawn, such as those in patterns, annotations or Type 3 fonts, fall back to the largest page's longer side. That
  fallback keeps more resolution than needed rather than less.
- **Photo or graphic.** Existing JPEGs, and images with more than 1024 distinct colours in a sample, are re-encoded as
  JPEG. Everything else is re-deflated, so screenshots and charts never get JPEG ringing. Soft masks are never made lossy.
- **Conservative thresholds.** An image is downsampled only when it has 1.25 times the needed resolution or more. An existing
  JPEG that is not downsampled is replaced only if re-encoding saves at least 25%, because each re-encode loses quality.
- **Anything not fully understood is left alone.** That includes CMYK, indexed and spot colour, 1-bit and 16-bit images,
  JPEG 2000, JBIG2, CCITT, `/Decode` arrays, colour-key masks and stencil masks.
- **Encrypted files are refused.** lopdf opens some encrypted files by itself and fails silently on others. Testing
  produced a 500-byte output from a 450 KB encrypted paper before a raw-byte check for `/Encrypt` was added. Saving a
  decrypted copy would also silently drop permission restrictions.
- **Every output is verified.** It is reloaded and its page count compared. If it is not smaller, nothing is written
  and the interface says "Already lean".
- **Outputs never overwrite.** Files are created with `create_new` as `<name>-epsilon.pdf`, then `-epsilon-2.pdf`, and so on.
- **Preset values live in the engine.** The interface asks for them with the `presets` command and does not repeat them.
- **No build step for the interface.** It uses plain HTML, CSS and JS with `withGlobalTauri`, like msw.
- **App identifier:** `com.pdfepsilon.desktop`. Tauri warns against identifiers ending in `.app` on macOS.

## Verified results

The engine was run on eight real PDFs from the owner's drives. Personal documents were avoided on purpose. The list of
files and where they live is kept locally in `scratch/test-files.md`, which git ignores, so it is not published:

- an illustrated poster
- a market survey report exported from PowerPoint
- a 12-page research paper (Less is More)
- an 18-page research paper (Oramas)
- an 801-page handbook
- a 22-page conference slide deck
- a 16-page big data guide
- a Korean newspaper article

| File | Lossless | Balanced | Small |
|---|---|---|---|
| poster | 0.7% | 78.2% | 90.1% |
| Oramas paper | 0.9% | 75.8% | 82.5% |
| slide deck | 1.5% | 66.6% | 84.0% |
| big data guide | 0.9% | 34.9% | 41.3% |
| Less is More paper | 13.8% | 30.9% | 37.8% |
| survey report | 8.9% | 27.2% | 39.3% |
| newspaper | 2.1% | 2.1% | 2.3% |
| handbook | 0% | 0% | 0.4% |

Each file took between 15 and 600 ms. These checks passed on every file:

- Text extracted with `mutool draw -F txt` was identical before and after.
- Link counts matched, for example 139 links in the Less is More paper.
- All 636 bookmarks in the handbook survived.
- No rendering errors appeared. Pages rendered with `mutool draw -r 110` look the same at page scale.
- At 200% zoom, Small visibly softens anti-aliased line art. That is expected at 96 dpi and quality 60.

The newspaper and the handbook barely shrink. The newspaper is mostly embedded Korean fonts. The handbook's few images
are CMYK or spot-colour JPEGs, which the engine leaves alone.

The error paths were also checked. An owner-password file and a user-password file are both refused. A non-PDF and a
truncated PDF both give a clear "not a readable PDF" message.

## Known issues and gaps

- **Only Type 0 TrueType fonts with Identity encoding are subset.** Simple TrueType fonts (cmap lookup rules),
  Type 1 and CFF are not yet.
- **CMYK and spot-colour images are untouched.** CMYK JPEGs with Adobe inversion need careful handling.
- **Lossless images use no PNG predictors.** Adding them would make re-deflated graphics noticeably smaller.
- **Encrypted PDFs cannot be processed.** A password prompt could open and re-encrypt them, but would need care with permissions.
- **Progress is per file only.** There is no progress within a single large file.
- **Files are processed one at a time.**
- **No "Open With" support.** Files dropped on the Dock icon or opened from Finder are not handled yet.
- **Installers are unsigned.** Gatekeeper and SmartScreen will warn on first launch.
- **No license file.** The manifests say `UNLICENSED` until the owner chooses one.
- **The domain is unregistered.** pdfepsilon.com showed no whois match on 19 October 2026, but it has not been bought.

## Next steps

1. **Test on other platforms.** The first push to `main` builds every installer. Download them from the run's
   artifacts and try the Windows and Linux builds. The workflow has never run, so expect to fix something.
   Push a tag such as `v0.2.0` for a draft release.
2. **Owner decisions.** Choose a license, register the domain, and decide on code signing. Signing needs an Apple Developer
   account for macOS and a certificate for Windows.
3. **Engine improvements,** in the order recommended by `reports/PDF size optimization techniques.md` (19 October 2026):
   CCITT G4 then lossless JBIG2 for bilevel images; lossless pixel-format reduction with PNG predictors; lossless JPEG
   Huffman optimisation (also a safe path for CMYK JPEGs); `libdeflater` plus structural tidying; simple-font and CFF
   subsetting; an opt-in to remove accessibility tags. Image cropping comes after these, once real files need it.
4. **Analyzer improvements:** account for clipping paths in the visible share, rendered before-and-after comparison,
   and per-page thumbnails in the report.
5. **App improvements:** "Open With" and Dock drops, a before-and-after page preview, and processing several files in parallel.

## How to check a change

```sh
cargo test -p epsilon --release
cargo run -p epsilon --release --example inspect -- in.pdf --pages               # the analysis as text
cargo run -p epsilon --release --example squeeze -- in.pdf out.pdf balanced
mutool draw -q -F txt in.pdf | md5; mutool draw -q -F txt out.pdf | md5   # must match
mutool draw -q -r 110 -o page.png out.pdf 1                                 # then look at it
PREVIEW_LANG=ko tools/ui-preview/preview.sh /tmp/ui.png dark 2600 a.pdf b.pdf  # interface with real analyses
npm run build                                                               # must finish
```

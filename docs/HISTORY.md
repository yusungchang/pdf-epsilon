# History

## Versions

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-10-19 | First version: engine, Tauri shell, interface, icon, CI workflow, docs. |
| 0.2.0 | 2026-10-19 | Analyzer: byte breakdown, measured preset savings, findings and page-by-page report. |

## 0.2.0, 19 October 2026

The owner asked for the analyzer as the first product, from a research brief on PDF size
reduction plus independent research.

1. **Research.** Compared the brief with Acrobat's Audit Space Usage, pdfsizeopt, and known causes of
   bloat: Illustrator `/PieceInfo`, incremental saves, artwork placed outside the page. See `ANALYZER.md`.
2. **Engine.** Added `analyze.rs`. Object sizes are measured from cross-reference offsets, every object
   is put in one category, and all four presets run in memory in parallel to measure savings.
   `placement.rs` now reports images, forms, fonts and inline images per page, and `images.rs`
   says why it skips an image.
3. **Bugs found on real files.** Repeated fonts were counted per font dictionary, which showed 233
   copies in an iText book; they are now counted per embedded program. Shared font objects inflated
   page sums to 145%; they are now split between pages.
4. **Interface.** The window now analyses first: byte breakdown, preset cards with measured sizes,
   findings, and an expandable page list. Saving a signed document asks first.
5. **Checks.** Categories add up to the exact file size on all eight test files. Compression results
   and extracted text are unchanged from 0.1.0.
6. **Version rule.** The owner changed the scheme: every session raises z, a meaningful change raises y.
   This session's analyzer is meaningful, so it is 0.2.0 rather than 0.1.1.
7. **Git and CI.** First commit. The workflow now builds installers for all three platforms on every push
   to `main` as downloadable artifacts, and on a `v*` tag as a draft release.

## 0.1.0, 19 October 2026

The project was set up in a single session with Claude Code.

1. **Scaffold.** Created a Cargo workspace with the engine crate and the Tauri shell, plus a plain-JS interface.
2. **Toolchains.** No Rust or Node was installed, so both were installed with Homebrew. The latest stable crate versions were
   looked up, and Tauri 3 was skipped because it is still in alpha.
3. **Engine.** Wrote image placement, image recompression, stream deduplication and cleanup. Fixed small API differences
   in lopdf 0.45.
4. **Real files.** Ran the engine on eight real PDFs. Text, links and bookmarks were preserved. See `HANDOVER.md`.
5. **Encryption bug.** Encrypted test files produced near-empty output. Added a raw-byte `/Encrypt` check and a
   zero-page guard. Both files are now refused.
6. **Integration test.** Added a test that builds a synthetic PDF and checks that a photo drawn at 2 inches becomes
   exactly 300 px at 150 dpi, while a flat graphic stays lossless.
7. **Interface and icon.** Built the drop-zone window and generated the epsilon icon from `assets/icon.svg`.
8. **App build.** Built `PDF Epsilon.app` and the `.dmg`, then moved build output to `.nosync` folders so iCloud skips it.
9. **Interface check.** Screen capture was blocked, so the interface was checked in headless Chrome with a mocked
   Tauri bridge. Fixed results rows that overflowed with long file names. The harness is saved as `tools/ui-preview`.
10. **Docs.** Wrote the README, CLAUDE.md, this file and HANDOVER.md, plus the GitHub Actions workflow for
    macOS, Linux and Windows builds.

# Before 1.0.0

Review of 24 October 2026 (at 0.6.1): interface, desktop shell, engine, and what is not supported.
Four reviews ran in parallel; each finding was traced in the code, and most were reproduced. The two
engine blockers were reproduced twice. Test files and harnesses were kept outside the repository.

Severity: **blocker** loses content or breaks the app; **should** fix before 1.0; **later** after 1.0.

**Where it stands (25 October 2026, code at 1.0.0, not yet tagged).** Every blocker and every must item is
done except three that wait on the owner: signed installers (`SIGNING.md`), registering pdfepsilon.com, and
trying the release candidate on Windows and Linux. The tag `v1.0.0` follows those.

## Blockers

All three fixed in 0.6.2; see `HISTORY.md` for how each was tested.

1. **Fixed in 0.6.2.** **Images whose `/DecodeParms` is an array or a reference come out as noise.** lopdf reads only a direct
   dictionary, so the PNG predictor is skipped, the row bytes shift, and the scrambled pixels are re-encoded
   as JPEG. Common in files that went through Ghostscript or iText (`/DecodeParms [<< /Predictor 15 >>]`).
   `images.rs:194-198`, `raw_image` `images.rs:350-363`; the same applies to `/CIDToGIDMap` (`subset.rs:195`).
   Fix: resolve the reference and a one-element array, otherwise leave the image; require the decoded length
   to match width × height × components.
2. **Fixed in 0.6.2.** **Font subsetting drops glyphs when a font is selected inside `q … Q`.** The current font is not restored at
   `Q`, so text drawn after it is attributed to the wrong font and its glyphs are removed. Happens even with
   Lossless. `mutool draw -F txt` does not notice, because the text is still there; only the glyph is gone.
   `subset.rs:351-411`. Fix: keep a graphics-state stack for the font.
3. **Fixed in 0.6.2.** **Closing the comparison while it is compressing locks the page.** `onclose` uses `resizeWatch` before it
   is declared, throws, and the page behind stays unscrollable. ‹ and › also throw while preparing.
   `ui/app.js:794-797`, `:1017`, `:787`.

## Should fix: engine

4. **Fixed in 0.7.0.** **Forms or patterns drawn many times make analysis exponential.** A 2.2 KB file with 8 levels took 23 s to
   save and 43 s to analyse; 10 levels would take hours. `placement.rs:185-215`. Remember results per form or
   set an operation budget; add to `tests/hostile.rs`.
5. **Fixed in 0.7.0.** **Images shown larger in annotations, patterns or soft masks are downsampled to their small page size.**
   A stamp annotation turns to mush. `placement.rs:85-98`, `images.rs:104-110`.
6. **Fixed in 0.7.0.** **`/UserUnit` is ignored**, so posters and CAD sheets are downsampled up to 10× too far. `placement.rs`.
7. **Message fixed in 1.0.0; output not changed.** **PDF/A: the advice "Use Balanced or a lighter preset to keep it archival" is wrong for PDF/A-1.** Every
   preset writes object streams and PDF 1.5, compresses the XMP stream, and leaves `/CIDSet` after
   subsetting. `lib.rs:485, 617`, `en.json` `finding.pdfa.detail`. Fix the output for PDF/A input, or change
   the message. Check with veraPDF.
   1.0.0 confirmed it on a PDF/A-1b test file (even Lossless compresses the XMP and writes PDF 1.5 with
   object streams) and changed the message: every preset breaks PDF/A, so keep the original where it
   matters. Writing valid PDF/A output stays under "Later".
8. **Fixed in 0.7.0.** **Analysis memory grows with file size:** four presets run in parallel on four copies. A 293 MB file peaked
   at 2.7 GB; near the 2 GB limit it would need about 18 GB. `analyze.rs:323`. Run presets one at a time above
   about 150 MB.
9. **Fixed in 0.7.0.** **Verification is page count only.** None of the issues above were caught. Cheap additions: an exact
   decoded-length check for images; per-page counts of annotations, fields, outline items and resources before
   and after; after subsetting, re-walk the output and check every used glyph is present; optionally a
   low-resolution render comparison of pages whose fonts or images changed (the renderer already exists).
   0.7.0 added all but the render comparison, which is still open; images are instead decoded again and
   compared with the pixels meant.
10. Smaller engine points:
    - Fixed in 0.6.2: DCT images with `/ColorTransform 0` got wrong colours; JPEGs with decode parameters are now left alone.
    - Fonts used only by streams that aren't forms (JavaScript templates, appearances without `/Subtype`) can
      be subset away (`subset.rs:282-291`).
    - Barcodes and QR codes can be downsampled until they stop scanning.
    - A crash inside a preset is reported as "failed verification" (`analyze.rs:352`).
    - A u16 overflow in `fonts::write` is possible on hostile fonts (`fonts.rs:139-147`).
    - Free entries in xref-stream updates are not read by `revisions.rs`.

## Should fix: desktop shell

11. **Fixed in 0.8.0.** **The renderer's 1 GB memory cap does not cap memory.** It counts live heap, but a 5.6 MB file with large
    images reached 5.3 GB resident. `SECURITY.md` claims otherwise. `main.rs:21-85`. Watch the child's resident
    memory from the parent and kill it above the limit; on Linux, add `RLIMIT_AS`, and on Windows a Job Object
    limit.
12. **Fixed in 0.8.0.** **There is no limit on renderer processes at once, and none are cancelled.** Each compared page starts two,
    and fast scrolling started 17 thumbnail renders. `main.rs:250-275`, `app.js:568-590`. Run one or two at a
    time; drop requests no longer wanted.
13. **Fixed in 0.8.0.** **The renderer hitting its cap aborts and writes a macOS crash report** (22 found in DiagnosticReports).
    Exit quietly instead (`main.rs:43`).
14. **Fixed in 0.8.0.** **Renderer children can outlive the app** until their page finishes (`render.rs:48-93`). Add a watchdog.
15. **Fixed in 0.8.0.** **Changing the original after comparing saves the old content.** The prepared copy and the analysis are
    keyed by path and settings only. `main.rs:171-200, 281`. Add size and modified time to the key.
16. **Fixed in 0.8.0.** **The prepared cache holds up to three full copies** (up to 6 GB) (`main.rs:180`). Cap by bytes.
17. **Fixed in 0.8.0.** **No thumbnails or comparison for files over about 0.5–1 GB;** the child fails to parse them
    (`render.rs:50-65`). Say so instead of showing blanks.
18. **Fixed in 0.8.0.** **Save errors are all "Could not read or write the file"**, with an English OS message even in Korean:
    - a removed output folder (remembered, so every save then fails);
    - a read-only volume;
    - names too long (a long Korean name plus `-epsilon.pdf` on Linux).

    Add codes for not found, permission and read-only, shorten long names, and check the remembered folder
    at start.
19. **Fixed in 0.8.0.** Smaller shell points:
    - Outputs are not written atomically, so cloud sync can upload a half-written file.
    - The download-quarantine marker is not carried over.
    - The `file_size` command is unused.

## Should fix: interface

20. **Fixed in 0.9.0.** **The dpi field can't be typed into:** each keystroke is clamped and written back ("300" becomes 36, 360,
    1200). `app.js:180`, `:162`. Clamp on blur.
21. **Fixed in 0.9.0.** **Saving is global, not per file:** other files show "Saving…", a result is lost if its file is removed,
    and changing language mid-save resets the label. `app.js:478-497`.
22. **Fixed in 0.8.0.** **Failed renders are cached forever** (`app.js:765-770`, `:588`), and quickly switching files can leave
    thumbnails blank for good (one shared timer, `:566-573`).
23. **Fixed in 0.9.0.** **Custom settings show "unchanged" for every image** in the page tables (`:711-728`).
24. **Fixed in 0.9.0.** **When every preset fails, the chip says "already lean"**, Save stays enabled, and the card tooltip is English
    engine text (`:303`, `:423`).
25. **Fixed in 0.9.0.** **Compare:**
    - a landscape page after a portrait one is drawn blurry (`:881`);
    - the side-by-side hint says "drag" (`:871`);
    - the hint mentions saving, but there is no Save button in the window.
26. **Fixed in 0.9.0.** **Keyboard focus jumps to the top** after choosing a preset, file chip, page row or compare button.
27. **Fixed in 0.9.0.** **No cancel, and no progress on Save.** Removing a file mid-analysis hides it, but the queue waits for it.
28. **Fixed in 0.9.0.** **Dropping a folder or a non-PDF does nothing and says nothing** (`:203`).
29. **Fixed in 0.9.0.** **Re-adding a file doesn't re-analyse it**, so a file that failed can't be retried without removing it first.
30. **Mostly fixed in 0.9.0** (slider no longer rebuilds the pages; render cache capped; jumping to a far page still renders the rows before it). **Large documents get slow:** a 5000-page file takes about 175 ms per quality-slider tick, and Expand all then
    jump takes 2.4 s. Render caches grow without a limit.
31. **Fixed in 0.9.0,** except a fully fixed page's amber issue count and Save staying prominent on a lean file. Smaller interface points:
    - A waiting file shows "Analysing… 0%".
    - The unreadable-pages banner stops at 10 with no "and N more".
    - "Show in Finder" fails silently if the file has moved.
    - A rate-limited update check says "Couldn't reach GitHub".
    - Menus have no arrow keys or focus return.
    - Text can't be selected for copying error messages.
    - The light theme flashes before dark applies.
    - "Show 4 more of 4 remaining" reads oddly.
    - The compare difference check freezes the window on big pages.

## Not supported (decide: build, or say so)

**Must for 1.0** (common, or the product looks broken without it):
- **Done in 0.9.0: Save all.** Each file must be selected and saved one by one.
- **Done in 0.9.0: Open With and file associations.** There is no Dock drop, no Finder or Explorer "Open with", and no
  single-instance handling on Windows and Linux.
- **Done in 1.0.0: MIT licence** (`LICENSE`). Was: `UNLICENSED`, while the repo is public and the website says "Free to use".
- **The homepage points to pdfepsilon.com, which is unregistered.** Owner's decision (25 October 2026): keep
  it; the owner registers the domain before the release.
- **A click-through of the real app on all three platforms.** The owner has used the real app on macOS
  through 0.11. Owner's decision: a release candidate from `main` is tried on Windows and Linux before the tag.
- **Signing.** Owner's decision (25 October 2026): 1.0.0 waits for signed installers. What to buy and
  what then changes is in `SIGNING.md`. macOS is ad-hoc signed and not notarised; Windows has
  no certificate, so SmartScreen warns.

**Should:**
- Owner-password-only PDFs (opening needs no password; common for statements and e-books) are refused like
  fully encrypted ones.
- Done in 0.9.0: Cancel for analysis, comparison and saving.
- Scans: 1-bit images, with no CCITT G4 or JBIG2 output (the top item in the size report).
- CMYK, indexed, spot-colour and Lab images are untouched. Lossless JPEG Huffman optimisation is the safe
  route.
- PNG predictors for lossless images.
- Optional removal of accessibility tags.
- Font subsetting covers only TrueType CID fonts (Identity). Simple TrueType, Type 1 and CFF are not covered.
- An accessibility pass: VoiceOver, NVDA and keyboard.

**Later** (disclose in the README):
- JPEG 2000, JBIG2 and CCITT inputs, chained filters, stencil and colour-key masks, `/Decode` arrays, images
  over 100 MP, 16-bit and odd bit depths, inline images.
- Cropping off-page images.
- Merging repeated typefaces.
- Removing JavaScript, attachments, annotations, bookmarks or hidden layers.
- Grayscale, target size ("under 10 MB"), mozjpeg.
- Linearisation; PDF/A output.
- A command-line tool.
- Parallel analysis.
- In-app update installation.
- Languages beyond English and Korean.
- ARM64 builds for Windows and Linux.
- Homebrew, winget and Flatpak.
- Files over 2 GB or 1 million objects, and damaged files, are refused by design.

## Website and README claims to correct

- Fixed in 1.0.0 (site and README): "Only the letters in use are kept": true only for TrueType CID fonts.
- Kept: "The size you see is the size you get": the site says it of the presets ("Each preset runs on your
  file first"), which is true.
- Fixed in 1.0.0 (site and README): "Charts and screenshots stay lossless": decided by a colour-count sample, so detailed charts can become JPEG.
  CMYK photos are not touched.
- Kept in 1.0.0: since 0.7.0 every copy is checked for every glyph the pages draw, and the qpdf corpus
  and real files are compared by extracted text. Was: "Not a word changes": checked by hand on the test files. The app checks page count only, and blocker 2 removes
  glyphs without changing the text.
- Fixed in 1.0.0: PDF/A advice in the app (item 7).
- Done in 0.9.0: The README's "Not yet supported" list is incomplete: it is missing Save all, Open With, the size and object
  limits, damaged files, inline images, and that only `.pdf` names are accepted.
- Done in 0.8.0: `SECURITY.md`: the renderer memory figures (item 11).

## Packaging

- Done: version 1.0.0 in all three files.
- `LSMinimumSystemVersion 10.13` has never been tested.
- Done in 1.0.0: `src-tauri/Info.plist` declares `en` and `ko` in `CFBundleLocalizations`, so system
  dialogs follow a Korean system. Was: no `ko`, so system menus stayed in English for Korean users.
- The NSIS installer is English only.
- MSI and NSIS installers are both shipped.
- The CI toolchain and `tauri-action` are not pinned, and releases publish no checksums.
- Done: `HANDOVER.md` brought up to date for 1.0.0.
- The minor-release routine still applies: security review, hostile corpus, fuzzing, then tag `v1.0.0`.

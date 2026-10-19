# Analyzer

PDF Epsilon explains a file before it changes it: where the bytes go, what each preset saves, what
else could be reduced, and all of this page by page. The code is `crates/epsilon/src/analyze.rs`,
and the entry point is `epsilon::analyze_bytes`. It writes nothing.
Its texts are message keys with values, rendered by the interface from `ui/locales`.

## How it works

1. **Measure.** Each object's size on disk comes from the cross-reference offsets. A stream's data is
   skipped by its known length, so binary data cannot fool the search for `endobj`. Objects packed in an
   object stream share the container's size in proportion to their length. Bytes outside every current
   object count as overhead: cross-reference data, padding, and earlier revisions from incremental saves.
2. **Classify.** Each object goes in exactly one category, in three passes:
   - First, the key that points to an object, such as `/Thumb`, `/PieceInfo`, `/Metadata`, a page's
     `/Contents`, `/AP` or `/EF`.
   - Then the object's own `/Type` or `/Subtype`.
   - Last, the category of whatever refers to it.

   Anything not reachable from the trailer is unused. The categories always add up to the file size.
3. **Walk pages.** Every page's content is followed through `q`/`Q`/`cm` and into form XObjects. The walk
   records each image drawn, with its matrix, plus forms, fonts selected with `Tf`, and inline images.
   From that, it gets each image's resolution as drawn and the share of it inside the crop box.
4. **Run presets.** All four presets run in parallel on copies in memory. Each run reports which images it
   changed and which objects it merged or pruned, so savings split into images, duplicates, thumbnails,
   editor data, metadata, unused objects and repacking. Repacking is the remainder, so the parts add up
   to the measured total.
5. **Report.** Findings for the whole file, with a status:
   - **fixed:** a measured saving for each preset.
   - **not handled yet:** PDF Epsilon cannot reduce it yet.
   - **kept:** deliberately kept, because removing it would lose something.
   - **warning:** something to know before compressing.

   Each page gets its share of the file, its issues and its saving under each preset.

Shared objects are split evenly between the pages, or fonts, that use them. Page shares therefore add up
to slightly less than the file. The rest is document-wide: tags, bookmarks, structure and overhead.

## What it detects

| Finding | Status | Notes |
|---|---|---|
| Images stored at more detail than shown | fixed | Downsampled by Print or Balanced |
| More images can be made smaller | fixed | Re-encoding, or only Small's 96 ppi |
| Images left as is (CMYK, spot colour, JPEG 2000, ...) | not yet | Grouped by reason |
| Images mostly outside the page | not yet | Under 70% visible. Clipping paths are not counted yet |
| Images not drawn on any page | kept | For example in annotations or patterns |
| Fonts embedded in full | fixed / not yet | No subset tag, program of 20 KB or more. Fixed for TrueType CID fonts |
| Fonts carrying data no viewer reads | fixed | Bitmaps, layout tables; measured per preset |
| Typeface embedded more than once | not yet | Counted per distinct font program |
| Duplicate streams | fixed | |
| Thumbnails, editor data, XMP, unused objects | fixed | |
| Saved on top of itself / structure can be packed tighter | fixed | Incremental saves, no object streams, uncompressed content |
| Very heavy drawing instructions | not yet | Page content of 1 MB or more, or a fifth of the file |
| Attachments, accessibility tags | kept | |
| Signed, PDF/A, hidden layers | warning | The app asks before saving a signed file |

## Results on the test files

All eight files from `HANDOVER.md` were analysed in 25 to 710 ms. Categories add up to the exact file
size on every one. Each preset's measured size matches what `compress_bytes` writes, and a test checks this.

| File | Largest part | What the analyzer adds |
|---|---|---|
| Newspaper | Fonts, 90% | 2 typefaces embedded 8 times, 1.5 MB, explains why it barely shrinks |
| Handbook | Text and drawing, 85% | 1 CMYK and 6 spot-colour images left alone, 15 typefaces embedded more than once |
| Survey report | Images, 64% | Saved twice: 315 KB of old version. 248 KB of accessibility tags, kept |
| Big data guide | Images, 76% | 3 CMYK images, 922 KB, that would downsample once supported |
| Slide deck | Images, 94% | A 2500 px image shown at 1763 ppi. Page 2 alone holds 1.9 MB |

## Research behind it

The owner's research brief proposed Inspect → Estimate → Optimize → Verify → Report. These changes
came from comparing it with existing tools and reports:

- **Acrobat's Audit Space Usage** reports bytes and percentage per element type: fonts, images,
  bookmarks, forms, named destinations, comments. PDF Epsilon goes further, with objects classified so
  every byte is counted once, overhead and old revisions shown separately, and the same breakdown per page.
  ([Adobe help](https://helpx.adobe.com/acrobat/desktop/create-documents/optimize-pdfs/audit-space.html),
  [Optimizing PDFs](https://helpx.adobe.com/acrobat/current/optimizing-pdfs-acrobat-pro.html))
- **pdfsizeopt** combines image optimisation, Type 1 font optimisation and removal of duplicate objects.
  Font work is the gap here, so the analyzer measures it now (full and repeated fonts) and leaves the fix
  for later. ([TUGboat paper](https://ntg.nl/maps/39/15.pdf))
- **Illustrator `/PieceInfo`** can embed a full editable copy, and incremental saves keep earlier
  versions. Both are measured, and fixed by rewriting.
  ([deillustrate](https://metacpan.org/module/deillustrate.pl),
  [Enfocus forum](https://forum.enfocus.com/viewtopic.php?p=18684))
- **Artwork outside the page.** Multi-artboard exports can carry every artboard's art on every page.
  Measuring the visible share of each image is the first step toward the brief's cropping idea.
  ([Signs101 thread](https://signs101.com/threads/does-illustrator-bloat-pdfs.178019))
- **Duplicate fonts from merged files** are a known source of bloat.
  ([GdPicture](https://www.gdpicture.com/?p=4019))

## Not yet

- Clipping paths are ignored when measuring the visible share of an image, so the real share can be smaller.
- Images and content in annotation appearances, patterns and Type 3 fonts are counted, but not placed on a page.
- Fonts used only by form fields can look unused.
- No rendered comparison yet. Verification is still a reload and page-count check, plus `mutool` by hand.

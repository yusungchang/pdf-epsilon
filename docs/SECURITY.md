# Security

PDF Epsilon opens PDFs from anywhere, so every file is treated as hostile. This page says
what a hostile PDF could try, what stops it, and how that is tested. It applies the same on
macOS, Windows and Linux.

## What PDF Epsilon never does with a PDF

- **Run it.** There is no JavaScript engine, and actions, launch commands, links and form
  scripts are never followed. They are only carried over into the smaller copy.
- **Render it in the app.** Page thumbnails are drawn by a separate copy of the app started
  just for that (see below). The window itself never parses or renders the document.
- **Extract it.** Attachments and embedded files are never written to disk.
- **Fetch anything for a PDF.** Nothing in a file can make the app connect anywhere. The app's
  only network access is the update check, which runs when someone clicks it: it asks
  `api.github.com` for the newest version number, and the Content Security Policy allows no
  other host. It sends nothing about any file. The only page it can open is the download page.

## What a hostile PDF could try, and what stops it

| Attack | Defence |
|---|---|
| Memory corruption in the parser or decoders | The whole path is Rust: lopdf, `zlib-rs` for Flate in the app (`miniz_oxide` when the engine is built on its own), `zune-jpeg` for JPEG. There is no C parsing code. `zlib-rs` and `zune-jpeg` use `unsafe` for speed, so both decoders are covered by the hostile tests and the fuzz runs below. |
| Decompression bomb (a small stream that inflates to gigabytes) | Every decode is bounded (see `crates/epsilon/src/limits.rs`). Streams loaded with the document: 256 MB. Page and form content: 64 MB. Fonts: 128 MB. Metadata and glyph maps: 16 MB. Images: never more than their own pixel count. Anything over a limit is left untouched. |
| Huge predictor rows (`/Columns 2147483647`) | Implausible `/Columns`, `/Colors` and bit depths are refused, by a raw-byte check before loading and again before any decode. |
| Millions of drawing operations in a short stream | Operations are counted before decoding; over 1 million, the content is treated as unreadable. |
| Forms drawn many times inside forms drawn many times (2 KB that would take hours to walk) | Each page's walk has a budget of 2 million operations, forms included; past it, the page counts as unreadable. The walks for fonts and for the checks read each form once. |
| Huge images (a JPEG claiming 65,000 × 65,000 pixels) | Images over 100 megapixels are left alone. The JPEG decoder also caps allocations at 512 MB. |
| Deep nesting (100,000 nested arrays) | lopdf stops at 100 levels. The engine's own walks stop at 16 levels of forms. |
| Reference cycles (a page tree, form, bookmark, form field or name tree containing itself) | Every walk tracks what it has visited, and the walks added in 0.7.0 also stop at a million nodes. |
| Lying font tables (offsets past the end, absurd glyph counts) | Every read is bounds-checked; a font that does not add up is left whole. |
| Files too large to hold in memory | Files over 2 GB are refused before reading. |
| Files with millions of tiny objects | The object count is estimated from the raw bytes before loading; files estimated at over 1 million are refused. Documents with over 100,000 objects are analysed one preset at a time. |
| Slow algorithms on large documents | Pruning and renumbering are linear. lopdf's own versions are quadratic: they took 103 s on 500,000 objects, against 2 s now. |
| Panics | Each preset runs on its own thread, and the app runs work off the main thread, so a panic becomes an error message, not a crash. |
| Rendering (thumbnails and page comparison) | The renderer (hayro, pure Rust, built with its `unsafe` speed-ups; it also decodes JBIG2, CCITT, JPEG 2000 and Type 1 fonts, which the engine never does) has no limits of its own: in testing, one hostile file made it take 10.9 GB and run past 60 s. So it runs in a separate process, started from the app's own executable with `--render-thumbnails`. That process limits itself twice: a counting allocator refuses more than 1 GB of live heap, and a watchdog thread ends the process once its resident memory passes 1.5 GB (memory a renderer frees is not always returned to the system: before 0.8.0 a 22 MB file held 5.3 GB with the heap under 1 GB) or after 65 seconds, so it never outlives the app for long. Either way it exits quietly, without a crash report. The app kills it if a page takes over 12 seconds or a batch over 60, runs at most two at once, and cancels a request when a newer one of the same kind arrives. Files over 400 MB are not drawn. Compressed copies for comparison reach it through a pipe and are never written to disk. A hostile file costs missing pictures, never the app. Measured for 0.8.0: the heavy-image files end at 1.6 GB (were 2.6 and 5.3 GB), the hostile corpus at 791 MB or less, and the qpdf corpus at 35 MB or less, with output identical to 0.7.0. |
| Text from the file in the interface (font names, producer, attachment names) | Always inserted as text (`textContent`), never as HTML. The Content Security Policy allows only the app's own scripts. |
| Overwriting files | Output is created with `create_new` as `<name>-epsilon.pdf`. An existing file or symlink is never replaced. |
| Encrypted files decrypted silently | Refused, by a raw-byte check as well as lopdf's flags. |
| Content lost or brought back by a lenient reader | lopdf reads damaged files leniently, and a copy saved from what it misread would change the document without saying so. Before writing, the engine counts three cases: an object the file lists and refers to that lopdf skipped (a syntax error, or an object stream over its limit); an object a later revision deleted that lopdf kept from an older one (lopdf ignores free entries, so the engine reads the cross-reference tables itself, see `revisions.rs`), while something still refers to it; and a stream whose data lopdf could not locate. Any of them leaves the file unchanged, with the message that part of it is damaged. On the qpdf test suite this guards 19 damaged files and an incremental update that deletes a page's content. |
| Content lost by the engine's own mistake | Checks written apart from the code they check (`verify.rs`, since 0.7.0). The saved copy is read back and must have the same pages, annotations, page resources, bookmarks, form fields, attachments and named destinations, or it is refused. After font subsetting the text is read again with its own reader of the graphics state, and every glyph drawn must still have its outline, or the fonts are restored whole. Each recompressed image is decoded again and compared with what it should hold, or it is kept as it was. |

With these limits, the hostile files measured peaked at 477 MB of memory (20,000 nested
forms), whether their bombs held 512 MB or 2 GB. Before them, a 12 KB file took 7.4 GB.
A legitimate 46 MB file with 500,000 objects takes 2 GB and 9 seconds.

## Limits of this

- **Memory still scales with the document.** lopdf holds a few kilobytes per object, so a file
  near the object limit can need several gigabytes. Very large files are refused; huge but
  legitimate ones are slow rather than fatal.
- **Output is not sanitised.** A smaller copy keeps the original's JavaScript, links and
  attachments, as promised. PDF Epsilon makes files smaller; it does not make them safe.
- **Signatures.** A signed document loses its valid signature in the copy. The app warns and
  asks first, and the original is never changed.

## How it is tested

- `crates/epsilon/tests/hostile.rs` builds 25 hostile PDFs, one for each attack above, and
  runs analysis and compression on each. Each runs on a thread with a 1 MB stack, smaller
  than any platform's default. CI runs it on macOS, Windows and Linux on every push.
- Unit tests in `limits.rs` check each bound directly.
- A larger corpus of 19 files, with 2 GB bombs and a 500,000-object file, was run by hand on
  19 October 2026 and again for 0.4.0, 0.5.0 and 0.6.0, measuring time and peak memory (see `HISTORY.md`).
  For 0.7.0 it was rebuilt with the five new cases (23 files): hostile files peak at 535 MB.
- The qpdf test suite (628 PDFs, many deliberately broken) and 78 files that once crashed qpdf
  under fuzzing are run through the analysis, every preset and the renderer. Every compressed
  copy's text is compared with the original's by MuPDF. For 0.6.0: no crash or hang, 440 MB at
  most, and identical text in all 483 copies written. For 0.7.0 (708 files): no crash or hang,
  579 MB at most, and 498 of 500 copies with identical text; the other two are originals MuPDF
  cannot read, whose copies show more text.
- `examples/fuzz.rs` damages seed PDFs at random and runs the whole analysis on each, on a
  1 MB stack under a 3 GB memory cap, recording panics, hangs and crashes. It runs on stable
  Rust. For 0.6.0: 200,000 inputs from the qpdf seeds, uncompressed with `mutool clean -d`,
  with no finding. For 0.7.0: 200,000 inputs from 1,250 seeds, with no finding.
- `.github/workflows/audit.yml` checks every dependency against the RustSec advisory database
  on every push and weekly. CI also runs the hostile tests with the Flate decoder the app ships.

To report a security problem, open an issue at <https://github.com/yusungchang/pdf-epsilon/issues>
without attaching the hostile file, and say how to reach you.

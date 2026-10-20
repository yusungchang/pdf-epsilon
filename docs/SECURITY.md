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
- **Fetch anything.** Nothing goes over the network; the app has no network permission.

## What a hostile PDF could try, and what stops it

| Attack | Defence |
|---|---|
| Memory corruption in the parser or decoders | The whole path is Rust: lopdf, `miniz_oxide` for Flate, `zune-jpeg` for JPEG. There is no C parsing code. |
| Decompression bomb (a small stream that inflates to gigabytes) | Every decode is bounded (see `crates/epsilon/src/limits.rs`). Streams loaded with the document: 256 MB. Page and form content: 64 MB. Fonts: 128 MB. Metadata and glyph maps: 16 MB. Images: never more than their own pixel count. Anything over a limit is left untouched. |
| Huge predictor rows (`/Columns 2147483647`) | Implausible `/Columns`, `/Colors` and bit depths are refused, by a raw-byte check before loading and again before any decode. |
| Millions of drawing operations in a short stream | Operations are counted before decoding; over 1 million, the content is treated as unreadable. |
| Huge images (a JPEG claiming 65,000 × 65,000 pixels) | Images over 100 megapixels are left alone. The JPEG decoder also caps allocations at 512 MB. |
| Deep nesting (100,000 nested arrays) | lopdf stops at 100 levels. The engine's own walks stop at 16 levels of forms. |
| Reference cycles (a page tree or form containing itself) | Every walk tracks what it has visited. |
| Lying font tables (offsets past the end, absurd glyph counts) | Every read is bounds-checked; a font that does not add up is left whole. |
| Files too large to hold in memory | Files over 2 GB are refused before reading. |
| Files with millions of tiny objects | The object count is estimated from the raw bytes before loading; files estimated at over 1 million are refused. Documents with over 100,000 objects are analysed one preset at a time. |
| Slow algorithms on large documents | Pruning and renumbering are linear. lopdf's own versions are quadratic: they took 103 s on 500,000 objects, against 2 s now. |
| Panics | Each preset runs on its own thread, and the app runs work off the main thread, so a panic becomes an error message, not a crash. |
| Rendering (thumbnails and page comparison) | The renderer (hayro, pure Rust, no `unsafe`) has no limits of its own: in testing, one hostile file made it take 10.9 GB and run past 60 s. So it runs in a separate process, started from the app's own executable with `--render-thumbnails`. That process caps its own heap at 1 GB with a counting allocator, on every platform alike, and the app kills it if a page takes over 8 seconds or a batch over 60. Compressed copies for comparison reach it through a pipe and are never written to disk. A hostile file costs missing pictures, never the app. Measured: the worst attacks now end in under a second at 11–796 MB. |
| Text from the file in the interface (font names, producer, attachment names) | Always inserted as text (`textContent`), never as HTML. The Content Security Policy allows only the app's own scripts. |
| Overwriting files | Output is created with `create_new` as `<name>-epsilon.pdf`. An existing file or symlink is never replaced. |
| Encrypted files decrypted silently | Refused, by a raw-byte check as well as lopdf's flags. |

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

- `crates/epsilon/tests/hostile.rs` builds 20 hostile PDFs, one for each attack above, and
  runs analysis and compression on each. Each runs on a thread with a 1 MB stack, smaller
  than any platform's default. CI runs it on macOS, Windows and Linux on every push.
- Unit tests in `limits.rs` check each bound directly.
- A larger corpus of 19 files, with 2 GB bombs and a 500,000-object file, was run by hand on
  19 October 2026, measuring time and peak memory (see `HISTORY.md`).

To report a security problem, open an issue at <https://github.com/yusungchang/pdf-epsilon/issues>
without attaching the hostile file, and say how to reach you.

# History

## Versions

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-10-19 | First version: engine, Tauri shell, interface, icon, CI workflow, docs. |
| 0.2.0 | 2026-10-19 | Analyzer: byte breakdown, measured preset savings, findings and page-by-page report. Installers for all platforms. English and Korean. |
| 0.3.0 | 2026-10-19 | Font optimizer: unused font tables dropped, fully embedded TrueType CID fonts subset. Research on further optimizations. |
| 0.3.1 | 2026-10-19 | Security hardening against hostile PDFs: bounded decoding everywhere, file size cap, hostile-file tests on all platforms. |
| 0.4.0 | 2026-10-21 | Analysis progress, page thumbnails and before-and-after page comparison in an isolated renderer, removing files from the list, compact language button, theme button, icon tools for the page list. |
| 0.6.0 | 2026-10-21 | Update check; damaged files are never half-copied; fuzzing and a public hostile corpus; security and privacy on the website. |
| 0.6.1 | 2026-10-24 | Text audit of every screen in English and Korean; wording changes from the owner's review. |
| 0.6.2 | 2026-10-24 | Three blockers from the 1.0 review fixed: scrambled predicted images, glyphs lost after `Q`, comparison window locking the page. |
| 0.7.0 | 2026-10-24 | Every copy is checked (structure, letters, images); images sized where they are really drawn; `/UserUnit`; bounded form walks; less memory on big files; stale `/Prev` fixed. |
| 0.8.0 | 2026-10-24 | Desktop shell: renderer memory really capped and quiet, two at a time and cancellable; saving never writes an outdated copy; clear save errors; safe writing. |
| 0.9.0 | 2026-10-24 | Save all, Open With and Dock drops, Cancel and save progress, and the interface fixes from the 1.0 review. |
| 0.9.1 | 2026-10-24 | Comparison of cloud files fixed; preset names as tags; version shown; Korean labels; full regression against 0.6.0. |
| 0.10.0 | 2026-10-24 | Faster: images on every core, a SIMD resizer, shared work in analyses, a lookup table in the operation count; memory sized to the computer. Analysis 1.8×, saving 2.0× against 0.9.0. |
| 0.10.1 | 2026-10-24 | Comparison pictures no longer lost when they arrive after the reply (the cause of "could not be rendered", worst at 2× and 4×). "Details" starts closed; no focus ring on the comparison's close button when it opens. |
| 0.5.0 | 2026-10-21 | 0.4.1 released as a minor version at the owner's request; first-launch steps shown on every download; security review. |
| 0.4.1 | 2026-10-21 | Long file names truncate on every WebKit, a round handle on the comparison divider, glass-finish icon, a product page on GitHub Pages, findings and pages folded under Advanced. |

## 0.10.1, 24 October 2026

**Comparison said "This page could not be rendered", worst at 2× and 4×.**

- *Reported:* with 0.10.0 installed from GitHub, Fit only sometimes showed the pages, and 2× and 4×
  always failed. The renderer itself was fine: run by hand, the installed copy drew a page 4,096 pixels
  wide in 1.25 s.
- *Found:* Tauri delivers a channel message over 8 KB (`MAX_JSON_DIRECT_EXECUTE_THRESHOLD` in
  `tauri/src/ipc/channel.rs`) by telling the window to fetch it with a second request. So a rendered
  page, always far larger, can reach the window after the reply to its `render` command.
  - Since 0.8.0 the window took a reply without a picture to mean the page could not be drawn, so a
    picture that came later was lost.
  - Bigger pictures take longer to fetch, which is why 2× and 4× nearly always failed and Fit only
    sometimes.
  - Every logging build used to investigate wrote to a file between sending the picture and replying,
    which gave the picture time to arrive. So every one of them worked, and the failure was blamed on
    other causes for a while (cloud files, interference from test runs, the downloaded copy).
- *Fix (`ui/app.js`):*
  - A finished request has always sent one message per page, an empty one for a page that could not be
    drawn, so the window now waits for it. Only a cancelled request ends without a picture.
  - A picture that never comes counts as failed after 15 seconds, instead of leaving "Rendering…".
  - Thumbnails get the same grace before a page is marked as not drawn.
- *Tested:* the screenshot harness's mock now delivers pictures 300 ms after the reply, as Tauri does
  with large messages. Before the fix, the comparison failed at Fit and at 4×; after it, both pages
  show at both. Thumbnails load the same with late delivery as without (15–16 drawn, none blank). All
  102 screens ran without error.

**"Details" starts closed at every launch.** The section below the save button was closed at first but
remembered once opened, so it kept reopening. It now always starts closed, and the stored setting
(`moreOpen`) is removed (`ui/app.js`, `ui/index.html`).

**No focus ring on the close button when the comparison opens.** The close button took focus on
opening, so a half-clipped focus ring showed at once. Focus now starts on the window (Escape still
closes it, Tab reaches the buttons), and the keyboard focus ring on the comparison's buttons is drawn
inside them so the edge cannot clip it (`ui/app.js`, `ui/style.css`).

## 0.10.0, 24 October 2026

The owner asked whether the app could be made radically faster. Profiling came first: macOS `sample`
on a build with symbols, repeating an analysis or a save so the sampler could attach.

**What the profile showed:**
- An analysis of the slide deck kept only about 1.4 of 11 cores busy, because each preset worked on
  its images one at a time.
- In the busy time, resizing images was 58% of the work, JPEG decoding 17% and JPEG encoding 10%.
- On the survey, 28% of a save went to Flate compression at the highest level: font programs after
  their tables are stripped, lossless images, and lopdf's writer.
- On the 801-page handbook, 41% of a save went to a byte search inside the operation count.

**Changes:**

1. **Images on every core (`images::recompress`).** Images do not depend on each other's results, so
   they are worked on in parallel and applied in their original order: the output is byte-for-byte the
   same as one at a time. Cancelling stops the workers after their current image.
2. **Memory sized to the computer.** A budget shared by every run in the process limits the images
   worked on at once:
   - an eighth of installed memory, at most 768 MB, and at most a quarter of what is free;
   - below 96 MB it is zero, which means one image at a time;
   - each image counts at its real size (one byte per pixel for grey, three for colour, doubled for
     the resized copy and the result);
   - one image alone always runs.

   Installed and free memory come from `hw.memsize` and the host's memory statistics on macOS,
   `/proc/meminfo` on Linux, and `GlobalMemoryStatusEx` on Windows (`system.rs`). A test pretends a
   machine with 300 MB free and confirms that never more than one image is in work, and that with
   memory to spare several are.
3. **A SIMD resizer.** `fast_image_resize` (pure Rust) with the same Lanczos3 filter as before, about
   four times faster: 38 ms → 9 ms for a 2480 × 1754 photo halved. Pictures differ from 0.9.1 by at most
   0.64/255 on the worst page of the real test files, and sizes by ±0.25%.
4. **Shared work in an analysis (`memo.rs`).**
   - Each image is decoded once for the three presets that change images, and dropped as soon as the
     last has it. Fonts compressed at the highest level are remembered (64 MB at most).
   - A save keeps nothing.
   - Keys are two hashes, with random keys per process, plus the length; image keys also hold the
     dictionary entries decoding reads and the resolved colour space.
   - Outputs are byte-for-byte the same.
5. **The operation count uses a table (`limits::operations_ok`).** It runs over every content stream
   several times per save, and searched the delimiter list for every byte. Saving the handbook in a
   loop: 0.93 s → 0.57 s.

**Results** (wall-clock, median of 7 alternating runs, against 0.9.0 built from its tag):

| File | Analysis | Save (Balanced) |
|---|---|---|
| Slides | 0.50 → 0.14 s (3.7×) | 0.40 → 0.07 s (5.4×) |
| Poster | 0.23 → 0.11 s | 0.38 → 0.05 s |
| Big data migration | 0.34 → 0.11 s | 0.22 → 0.06 s |
| Music genre paper | 0.25 → 0.09 s | 0.13 → 0.06 s |
| Survey | 0.67 → 0.45 s | 0.57 → 0.29 s |
| Handbook (801 pages, text) | about the same | about the same |
| **All eight** | **2.62 → 1.47 s (1.8×)** | **2.10 → 1.04 s (2.0×)** |

- CPU time fell 7–9%.
- The 286 MB file with three 33-megapixel photos: analysis 6.2 → 3.0 s, save 2.2 → 1.3 s.
- Its peak memory rose (save 0.85 → 1.3 GB on a 36 GB Mac), because its three photos now fit the budget
  together. On a machine with little free memory, the budget makes them go one at a time.
- Text-only files gain little: their time is in reading and writing the PDF itself.

**Checks.**
- *Tests:* all engine and shell tests pass, with new ones for the memo (results worked out once,
  shared between threads, dropped when everyone has them, within the cap), the memory budget, and one
  image at a time with little free memory.
- *Real test files:* after each step except the resizer, all 32 outputs and 8 analyses are
  byte-for-byte unchanged. After the resizer, 14 outputs changed, every page was compared as a picture
  (0.64/255 worst), text is identical in all 32, and preset sizes in the analyses moved by at most
  0.25% with the same findings and byte breakdown.
- *qpdf corpus (708 files, against 0.9.1):*
  - no crash or hang;
  - 700 outputs identical, and 8 differ by under 0.3% in size, at most 0.4/255 in pictures (every page
    compared);
  - text as before;
  - saving the corpus takes 9.3 s → 4.6 s.
- *Hostile corpus:* the same outcomes, with peak memory within run-to-run variation (five runs each).
- *Fuzzing:* 50,000 inputs, no panic, hang or crash, 357 MB at most.
- `npm run build` produces the app.

**Minor release routine.**
- *Security review:* no new parsing. The new code is threads over already-bounded work, a memo keyed by
  hashes the input cannot steer, and reading memory figures from the system.
- *Dependencies:* `fast_image_resize` resizes buffers the engine has decoded itself, and uses
  `unsafe` SIMD, so it is now covered by the hostile tests and the fuzzing; `libc` is used on macOS and
  `windows-sys` on Windows for the memory figures.

## 0.9.1, 24 October 2026

1. **Comparison failed for files in cloud folders.**
   - *Reported:* the owner, comparing a PDF in a OneDrive shared library, saw "이 페이지를 렌더링하지 못했습니다"
     ("This page could not be rendered").
   - *Found:* the 0.8.0 check that drops an outdated comparison copy compared the file's modification time
     to the nanosecond. When iCloud Drive downloads an online-only file on first read, its time moves by
     nanoseconds: measured +37 ns on a file made online-only with `brctl evict`, with the size unchanged.
     The copy was then taken as outdated and dropped, and the compressed side could not be drawn. OneDrive
     files could not be read from the terminal (macOS privacy protection), so that case was reasoned from
     the iCloud one.
   - *Fix:* the time is compared to the millisecond, and taken after the file has been read (and so
     downloaded). A test sets a file's time 37 ns later (the copy is kept), then 2 s later (dropped).
   - *A second cause:* while this was being tested, the automated test runs force-quit every PDF Epsilon
     process on the Mac, the owner's open app and its renderers included. All builds share one bundle
     identifier, so `open` could also bring up another copy that was already running. Once neither
     interfered, the owner's own run rendered 14 of 14 comparisons, at Fit and zoomed in. The owner's
     failing screenshot left no log, so it cannot be told which cause it was.
2. **Preset names inside sentences are tags.** A small bordered tag, like a keyword, replaces the
   Korean quotation marks ‘균형’, in both languages. The locale text marks names `[[like this]]`, and
   the interface draws them with `rich()`: findings, warnings, the custom-settings note, pills, the page
   table header and the comparison title. Names that stand alone (preset cards, swipe labels, label
   forms such as "균형: −19 MB") stay plain. `tests/locales.rs` checks that every mark is closed and that
   each language tags as many names as English.
3. **Korean labels.** "세부 정보" became "자세한 정보"; "발견한 것" became "발견된 문제점" (the folded block's hint too).
4. **The version is shown,** small and faint beside "PDF Epsilon" in the header.
5. **Regression against 0.6.0.**
   - *How:* 0.6.0 was built from its tag, both versions the way the app ships them (with `zlib-rs`), and
     both were run on the same files.
   - *Real test files (8 files, 4 presets):* all 32 outputs are byte-for-byte identical to 0.6.0, and
     have the original's pages and text. The analyses match exactly: every preset's size, every finding,
     every category.
   - *Renderer:* thumbnails identical to 0.6.0, at the same time and memory.
   - *qpdf corpus (708 files, Balanced):*
     - no crash or hang in either version;
     - 703 files have the same outcome and output size as 0.6.0;
     - two now write a copy (the stale `/Prev` fix of 0.7.0);
     - three `png-filters` files differ by up to 4% in size, because their decode parameters are written
       as an array, which 0.6.2 now reads properly; at 300 dpi both versions match the original
       exactly.
     - Text identical in all copies except two whose originals MuPDF cannot read, as before. One more
       mismatch in the first run did not reproduce in five reruns: that file is deliberately malformed
       and MuPDF's warnings vary.
   - *Hostile corpus (23 files):* the same outcomes, and peak memory within run-to-run variation. Five
     runs each: `many-nulls.pdf` 473–585 MB on 0.6.0 and 462–588 MB now; the font bomb 626–807 MB and
     615–693 MB. The 10-level form fan-out ran past 120 s on 0.6.0 (killed) and takes 2.8 s now. The
     500,000-object file takes 25 s and 3.6 GB on both.
   - *Performance:* measured as CPU time, alternating versions, five runs each, because wall-clock times
     varied by up to four times on this Mac.
     - Analysis takes 5.1% more CPU than 0.6.0 (7.67 s → 8.07 s for the eight files).
     - Saving with Balanced takes 4.2% more (2.16 s → 2.25 s).
     - This is the cost of the checks added in 0.7.0: reading every copy back, the letter check, and
       decoding each new image again. Memory is unchanged.
   - *Also run:* all engine and shell tests, every interface screen in both languages (49, no script
     error), and `npm run build`.

## 0.9.0, 24 October 2026

The interface items of the 1.0 review (`RELEASE-1.0.md`, items 20 to 31) and two features the owner
agreed are required for 1.0: Save all and Open With. Each change was checked in the labelled screenshot
harness from the 0.6.1 text audit, in English and Korean (46 screens, 285 of 286 strings shown; the
other appears only past 120 pages).

1. **Cancel, and progress while saving.**
   - *Engine:* `epsilon::Cancelled` stops work in progress. A progress callback raises it as a panic
     payload, and the engine passes it on. When the presets run in parallel, every run is joined
     first, so a stop is never mistaken for a crash.
   - *Shell:* each analysis, comparison copy and save carries a ticket; `cancel(ticket)` makes its next
     progress call stop. A real crash in a preset is now an internal error, not "failed
     verification" (review item 10). A panic hook keeps stops out of the error output.
   - *Interface:*
     - Removing a file mid-analysis stops it, so the next file starts at once. The analysing box
       has a Cancel link.
     - Saving shows "Saving… 45%" with Cancel beside it.
     - Closing the comparison stops its copy.
   - *Tested:* an engine test stops an analysis (presets in parallel) and a save, and both come out as
     `Cancelled`. In the harness, Cancel during a save showed "Saving… 24%", then the button came back
     with nothing saved.
2. **Save all.**
   - Appears once two files are ready, and saves every analysed file with the current settings, one
     after another ("Saving 2 of 3…").
   - Saving is now per file: another file's save does not lock this one, and a file being saved
     cannot be removed.
   - A saved file's chip says "saved −71%". Signed files still ask first, naming the file.
3. **Open With.**
   - PDF Epsilon is declared a viewer of PDFs with rank Alternate, so it is offered in Finder,
     Explorer and Linux file managers but never becomes the default. The bundle's `Info.plist`
     shows `CFBundleTypeRole Viewer`, `LSHandlerRank Alternate`, `com.adobe.pdf`, and no exported
     type.
   - Files arrive four ways: on the command line at first launch, through the macOS Opened event
     (Open With and Dock drops), and on Windows and Linux through `tauri-plugin-single-instance`,
     which hands a second launch's files to the running app.
   - The shell queues them and emits `files-opened`; the interface takes them at start and on each
     event.
   - *Tested on macOS with the built app:* a file opened while the app was closed, and another while
     it was running, both reached the interface (checked with temporary logging, since removed).
     Windows and Linux are compiled in CI but not yet tried by hand.
4. **Interface fixes.**
   - *Inputs and saving:*
     - The dpi field is read when done (Enter or leaving it), not on each keystroke.
     - Moving a slider no longer rebuilds the findings and pages unless the settings become custom or
       stop being so.
   - *Wrong or misleading displays:*
     - With custom settings, the page tables say "measured when saved" under "With custom settings".
     - When every preset failed, the chip says "couldn't compress", Save is disabled, failed presets
       draw no meter, and their cards no longer carry English engine text.
     - Each compared page is drawn at its own proportions, so a landscape page after a portrait one is
       sharp.
     - Side by side has its own hint.
   - *Keyboard:*
     - Focus stays on the preset, file chip, page row or comparison button you used.
     - Menus open with focus on their first item, move with the arrow keys, and give focus back on
       Escape.
     - The current file chip is marked `aria-current`, and the comparison's toggles carry
       `aria-pressed`.
   - *Files:*
     - Dropping folders or other files says how many were skipped.
     - Adding a file again analyses it again: retrying a failed one, or picking up changes.
     - A waiting file says "Waiting to analyse…".
     - Files dropped while the comparison is open join the list without changing the report behind
       it.
   - *Messages:*
     - The unreadable-pages banner adds "and N more" after ten pages.
     - "Show in Finder" says when the copy is gone.
     - The update check says "Couldn't check right now", which is also true when GitHub limits
       requests.
     - "Show 4 more of 4 remaining" became "Show the last 4 pages".
   - *Smoothness:*
     - Messages, warnings and results can be selected and copied.
     - A small `theme-boot.js` applies a saved Light or Dark choice before the first paint, so there is
       no flash.
     - The comparison keeps only its 24 most recent page pictures.
     - The difference check yields to the window between strips.
5. **Not done:**
   - Jumping to a far page still renders all the rows before it.
   - A page whose issues are all fixed still shows its issue count in the "not handled yet" colour.
   - A lean file still shows Save as the main button.
6. **Minor release routine.**
   - *Security review:* new inputs are file paths from the system (filtered to `.pdf`, made absolute)
     and the ticket mechanism; no new parsing.
   - *Hostile corpus:* the 23 files peak at 535 MB, as before.
   - *Fuzzing:* 50,000 more inputs, with no panic, hang or crash, at most 303 MB.
   - *Not re-run:* the qpdf corpus, since the engine's reading and writing are unchanged since 0.7.0
     (only how a stop or a crash is reported).
   - *Tests:* all engine and shell tests pass, and the app builds.

## 0.8.0, 24 October 2026

The desktop-shell items of the 1.0 review (`RELEASE-1.0.md`, items 11 to 19). The engine is unchanged.

1. **The renderer's memory limit now holds** (`render.rs`, `main.rs`).
   - *Found:* the renderer counted its live heap and refused allocations past 1 GB, but memory it freed
     was not always returned to the system. The reviewer's 22 MB file with twelve 12,000-pixel images
     made it hold 5.3 GB (6 images: 2.6 GB), while the heap never passed 1 GB.
   - *Fix:* a watchdog thread in the renderer checks its peak resident memory every 20 ms (`getrusage`
     on macOS and Linux, `K32GetProcessMemoryInfo` on Windows) and ends the process past 1.5 GB.
   - *After:* both files end at 1.6 GB in under 1.5 s, with blank thumbnails instead.
2. **No more crash reports.** A refused allocation made Rust abort, and macOS kept a crash report for
   each one (22 were found on the owner's Mac). The allocator and the watchdog now end the process
   with `_exit` (`TerminateProcess` on Windows): exit code 4 or 3, no report. Before and after the
   checks above, the number of reports stayed the same.
3. **Renderers do not outlive the app.** The watchdog also ends the process 65 seconds after it
   started, past the app's own 60-second batch limit.
4. **Two renderers at a time, and old requests cancelled.**
   - At most two renderer processes run at once; the rest wait.
   - Each request names its kind: thumbnails, or one side of the comparison. A newer request of the
     same kind cancels the older one, whether it is waiting or running, and the command returns
     `false`.
   - The interface asks for one batch of thumbnails at a time per file, with its own timer per file.
     Before, a shared timer could leave a file's thumbnails blank for good.
   - Pages cancelled before they were drawn are asked for again when they come back into view.
   - A page that failed to draw is no longer remembered as failed, so it is tried again.
   - Duplicate page numbers no longer stall a batch.
5. **Saving never writes an outdated copy.** The compressed copy kept from the comparison is now
   keyed by the file's size and modification time as well as its path and settings. If the file
   changed on disk since, the copy is dropped and saving compresses the file as it is now.
6. **The kept copies are capped at 512 MB together** (the newest always stays). Before, up to three
   whole copies were kept, up to 6 GB with files near the 2 GB limit.
7. **Files over 400 MB are not drawn.** Their thumbnails stay blank, and the comparison says
   "This file is too large to show here. Saving still works." Before, files over about 1 GB showed
   blank pages without a word.
8. **Save errors say what went wrong, in the interface's language.** New error codes:
   - `folderMissing`: the chosen folder is gone. The folder choice then goes back to "same folder as the
     original", so trying again works.
   - `notWritable`: no permission, or a read-only volume.
   - `fileMissing`: the original was moved, renamed or deleted.

   These no longer append the English system message. Names that would pass 240 bytes are shortened
   at a character boundary: a long Korean name failed on Linux, where the limit is 255 bytes.
9. **Copies are written safely.**
   - Each copy is written to a hidden temporary file, then linked under its final name (a link never
     replaces a file), so cloud sync never sees a half-written copy. Where links are not supported, it
     is written in place as before.
   - The macOS quarantine mark and the Windows Mark of the Web are carried over from the original,
     since the copy keeps its scripts too.
   - The unused `file_size` command is gone.
10. **Tests.**
    - The app shell has tests for the first time. Unit tests cover:
      - shortening long names;
      - writing next to earlier copies without overwriting them or leaving temporary files;
      - each new error code (the read-only folder case on macOS and Linux only);
      - an outdated copy being dropped;
      - the 512 MB cap;
      - at most two renderers at once, with a newer request cancelling older ones (running and waiting).
    - An integration test runs the real renderer: it draws a plain page, and a page of six huge images
      ends it with code 3 or 4.
    - CI now runs these on all three platforms. That is also the first place the Windows-only code is
      compiled, since this Mac has no Windows toolchain.
11. **Checked.**
    - The renderer on all 708 qpdf files gives output byte-for-byte identical to 0.7.0, at most 0.25 s
      and 35 MB per file.
    - On the hostile corpus: 791 MB at most, and no crash reports.
    - On the eight real test files: every page drawn as before.
    - In the interface harness: the missing-folder message with the folder choice reset, the
      too-large message in the comparison, and thumbnails still loading.
    - `npm run build` produces the app.

## 0.7.0, 24 October 2026

Two of the three blockers in 0.6.2 lost content while every existing check passed: the only check on a
copy was that it reloads with the same page count. The owner agreed to make checking every copy the next
change, together with the remaining engine items that change content (`RELEASE-1.0.md`, items 4 to 9).

1. **Checks on every copy (`verify.rs`, new).** Written apart from the code they check, so a mistake there
   is not repeated.
   - *Structure.* Before anything changes, the engine counts, per page, the annotations and the resource
     entries that point at something (forms followed, each form counted once by id), and for the document
     the bookmarks, form fields, attachments and named destinations. The saved copy is read back from its
     bytes and counted the same way. Any difference refuses the copy with "failed verification", saying
     what differs ("page 3: 4 annotations instead of 5").
   - *Letters.* After font subsetting, the text is read again by a separate reader of the graphics state:
     `q`/`Q`, `Tf`, fonts set by a graphics state, fonts inherited by forms, annotation appearances. Every
     glyph drawn with a subset font must still have its outline, unless it had none before. If one is
     missing, the subsetting is undone and the fonts stay whole (`Report.fonts_restored`). Subsetting
     now keeps what it replaced for this (`subset::Undo`).
   - *Images.* Each recompressed image is decoded again as a viewer would read it. A lossless one must
     match exactly; a JPEG must average within 20 levels per channel of the pixels it was made from.
     Otherwise that image is kept as it was.
   - *Tested:*
     - With the 0.6.2 `q`/`Q` fix taken out again, the regression file's subsetting was undone and both
       fonts were kept whole (11 glyphs each), so the bug could no longer lose a letter.
     - Unit tests: a glyph removed from the font that draws it, reached through a form after `q … Q`, is
       found; counts and their comparison; images that do and do not read back.
     - The eight real test files with Lossless, Balanced and Small: all 24 copies are byte-for-byte the
       same as 0.6.2, with the same text. The letter check ran on six subset fonts in three files and
       passed. Time is unchanged within noise.
2. **Images measured where they are really drawn (`placement.rs`).**
   - An image also shown in an annotation appearance, tiling pattern, soft mask or Type 3 glyph is no
     longer downsampled below the largest page's size. The reviewer's file, a 600 × 400 photo drawn 60 pt
     wide on the page and 600 pt wide in a stamp, had been cut to 125 × 83; it now stays 600 × 400.
   - `/UserUnit` scales the drawn size, page boxes and the largest page. A poster with `/UserUnit 10` had
     its image cut from 600 × 400 to 208 × 139; it now stays 600 × 400.
   - Each page's walk has a budget of 2 million drawing operations, forms included
     (`limits::PAGE_OPERATIONS`); past it the page counts as unreadable. The reviewer's 2.2 KB file with
     forms fanned out 8 levels took 27.5 s to save; it now takes 3.4 s. A 10-level version (a new hostile
     case) ran past 5 minutes in 0.6.2 and now takes 3.3 s to analyse.
3. **Less memory on big files.** Files over 150 MB are analysed one preset at a time
   (`limits::PARALLEL_BYTES`). A 286 MB test file peaked at 2.77 GB and took 4.2 s; now 1.6 GB and 6.3 s.
4. **Copies no longer point at the original's earlier revisions.** lopdf keeps `/Prev` from the input's
   trailer when it cannot follow it, and the copy then pointed into nothing and could not be read back.
   `save` now removes `/Prev` and `/XRefStm`. Found by the qpdf corpus run below: two files were refused
   for this in 0.6.2 and 0.7.0 alike, and are now written with identical text. On every other file the
   change makes no difference to the bytes.
5. **Tests added.**
   - `tests/regressions.rs`: `/UserUnit`, an image in a stamp annotation, annotations kept, and the stale
     `/Prev` case, which uses one small file from the qpdf test suite (`tests/data`, Apache License 2.0).
   - `tests/hostile.rs` now has 25 cases: forms fanned out 10 levels; a bookmark that is its own next; a
     form field that is its own kid; a name tree containing itself; an appearance that draws itself.
6. **Minor release routine.**
   - *Security review.* New parsing paths: the structure counts and letter reader (`verify.rs`), the
     image-use walk (`placement.rs`) and the page budget. All decode through `limits.rs`, every walk tracks
     what it has visited, and the new walks also stop at a million nodes and 16 levels. Each has a hostile
     case.
   - *Hand-run hostile corpus*, rebuilt (23 files: seven 2 GB bombs, absurd predictors, an operation
     flood, cycles, deep nesting, 20,000 nested forms, a lying font, a 65,000-pixel JPEG header, the five
     new cases, and a legitimate file of 500,000 objects): no crash or hang.
     - Hostile files peak at 535 MB, the same as 0.6.2 run on the same files (the font bomb in both).
     - The fan-out file, killed at 5 minutes in 0.6.2, now takes 3.3 s.
     - The 500,000-object file (74 MB, 166,000 pages): analysis 25 s and 3.5 GB (0.6.2: 25 s and 4.4 GB).
       A Small save takes 6.7 s and 2.8 GB (0.6.2: 6.0 s and 2.2 GB); reading the copy back for the checks
       costs that extra half gigabyte.
   - *qpdf corpus.* The qpdf test suite (630 PDFs at commit 4eba958) and 78 qpdf fuzz crashers, 708 files:
     - no crash, hang or panic;
     - analysis at most 579 MB and 0.86 s; 208 cleanly refused;
     - compressed with Balanced, 500 copies written, 498 with exactly the original's text in MuPDF. The
       other two are files MuPDF cannot read at all, whose copies show more text, not less, as in 0.6.2.
     - Every file's outcome, error and output size is the same as 0.6.2. No check refused a file wrongly,
       and no subsetting was undone. Only one written copy used subsetting, so the real test files above
       exercise the letter check more.
     - The renderer: at most 47 MB and 0.25 s.
   - *Fuzzing.* 200,000 inputs from 1,250 seeds (qpdf files under 100 KB, plain and uncompressed with
     `mutool clean -d`): no panic, hang or crash; at most 409 MB.

## 0.6.2, 24 October 2026

The review before 1.0 (`docs/RELEASE-1.0.md`) found three blockers. Each was reproduced, given a test that
failed, fixed, and checked again. The test files and harnesses live outside the repository.

1. **Images whose decode parameters were an array or a reference came out as noise.**
   - *Found:* lopdf applies `/DecodeParms` only when it is a direct dictionary (`lopdf object.rs:921`). The
     form `[<< /Predictor 15 … >>]`, common after Ghostscript or iText, was ignored, so each row kept its
     predictor byte. `raw_image` only checked that there were at least width × height × 3 bytes, so the
     shifted rows were taken as pixels and re-encoded as JPEG. lopdf also hands one dictionary to every filter
     in a chain, treats an unreadable `/Filter` (a reference) as no filter, and, for page content, appends a
     stream's raw bytes when it cannot decode it. Fonts, CID maps and page content go through the same
     decoder, so they were exposed too.
   - *Tested:* a 600 × 400 RGB photo with Flate and PNG Sub prediction, in three files that differ only in
     how `/DecodeParms` is written (direct, array, reference). Before the fix, Balanced output differed from
     the original by 113.7 per channel on average (scrambled). The reviewer's files, rendered with
     `mutool draw`, showed noise where the photo was.
   - *Fix:* `limits::decode` now checks first whether lopdf would decode correctly. It unwraps
     `[<< … >>]` for a single filter and refuses a `/Filter` that is not names, parameters by reference, and
     parameters on a chain. Refused streams are left untouched, and images report "kept: unusual filter".
     `page_content` decodes each content stream itself, and a page it cannot decode counts as unreadable.
     `raw_image` now requires exactly the pixel bytes, plus at most one padding row. A JPEG with decode
     parameters (such as `/ColorTransform 0`, which gave wrong colours) is left alone.
   - *After:* direct and array forms are recompressed correctly (65.8% smaller, picture intact); the
     reference form is kept as it is.
2. **Font subsetting removed glyphs drawn after `Q`.**
   - *Found:* the walker kept one "current font" that `Tf` overwrote. In PDF the font belongs to the
     graphics state, so `Q` restores the font from before `q`. Text after `Q` was credited to the wrong
     font, and the right font lost those glyphs. This happened even with Lossless. The `mutool draw -F txt`
     check misses it, because the text is still in the file; only the glyph outlines are gone.
   - *Tested:* two embedded TrueType fonts. F1 draws glyph 3; F2 is selected and used inside `q … Q`;
     after `Q`, F1 draws glyph 5. Before the fix, F1 kept only glyph 3. The reviewer's file lost the word
     "Wizard" on the page.
   - *Fix:* `q` pushes the current font and `Q` pops it (`subset.rs`).
   - *After:* F1 keeps 3 and 5, F2 keeps 4, and "Wizard" is drawn. That output is 98 KB instead of 43 KB:
     the missing glyphs are back.
3. **Closing the comparison while it was compressing locked the page.**
   - *Found:* `onclose` called `resizeWatch.disconnect()`, but `resizeWatch` was declared only after the
     compression finished. Closing earlier threw, `modal-open` stayed on the page, and the report could not
     scroll. ‹ and › were live while compressing and threw too, and after an error the title still said
     "Compressing…".
   - *Tested:* two new scenes in the labelled screenshot harness. One opens a comparison whose
     compression never finishes, presses ‹, closes, and records errors and the lock. The other makes
     compression fail. Before the fix, both showed the page locked and a `ReferenceError`.
   - *Fix:* `resizeWatch` is declared up front and checked before use. ‹ and › are disabled until the page
     can be drawn. After an error, the title names the page. Each opening has its own number, so work
     still finishing for an earlier opening is dropped.
   - *After:* no errors and the page scrolls, in English and Korean; the normal comparison screens are
     unchanged.

**Checks.**
- `tests/regressions.rs` holds the two engine cases, and `limits.rs` has a unit test for every form of
  `/DecodeParms`. All tests pass.
- The eight real test files were run through the engine before and after the fixes, with Lossless,
  Balanced and Small (24 runs). Every output is byte-for-byte identical to before, and the text extracted
  by mutool is identical to the original in every case.

## 0.6.1, 24 October 2026

The owner audited every interface string. To give context, the real interface was run in headless Chrome
with a mocked engine and made-up files built to reach every message, and each string was labelled with its
key where it appears: 39 screens in both languages, covering 271 of the 273 keys and every English plural
form. The harness lived in the session scratchpad and is not in the repository.

Changes from the owner's review:

1. **Drop area.** "Drop PDFs here to get started" / "시작하려면 PDF를 여기에 끌어다 놓으세요".
2. **Update menu.** The privacy line under the version check is gone (`update.privacy` removed).
3. **Preset names in Korean sentences are quoted:** ‘균형’, ‘최소’ and `{preset}` / `{settings}` inside
   sentences, because Korean has no capital letters to mark a name. Names standing alone (preset cards,
   swipe tags, "균형: 34 MB 절감" style labels) stay plain.
4. **Fully embedded fonts in Korean:** "통째로 내장된" became "전체가 내장된" / "글꼴 전체에서".
5. **Font tables:** "Remove font data PDF viewers never use" / "PDF 뷰어가 쓰지 않는 글꼴 데이터 지우기", and
   the matching finding title. The owner suggested "unused font data"; "PDF viewers" was kept because
   the option next to it (keeping only the glyphs used) also removes unused data, and the two must not
   read alike.
6. **Save location:** "same folder as the original" instead of "next to the original".
7. **Heavy pages in Korean:** "그림 명령이 매우 많은 페이지".
8. **Comparison steps named for what they do.** The first step compresses the file with the current settings
   (and that result is what saving writes), so it says "Compressing with these settings…" / "이 설정으로
   압축하는 중…" instead of "Making the smaller copy"; `error.notPrepared` follows. The owner suggested
   "preview", then "rendering"; rendering is the next step, so "Rendering the page…" / "페이지를 렌더링하는
   중…" went there, with `compare.renderFailed` to match. The saved file is still "smaller copy" / "작은 사본".

9. **Two strings deleted,** found by the audit: `files.analysing` was never used (the app shows
   `files.analysingPercent`), and `pill.warning` could not appear, because warning findings are shown as
   banners and filtered out of the list where pills are drawn. Its code branch and `.pill.bad` style went too.

## 0.6.0, 21 October 2026

The owner asked for an update check, tighter security against known PDF attacks, and the website to say how
it is tested, with a warmer line about files staying on the computer. Collecting users' email addresses was
raised and deliberately left for later.

1. **Update check.** A round button beside the theme and language buttons. It checks only when clicked, so
   nothing goes over the network on its own: it asks `api.github.com` for the newest published release and
   compares version numbers. A newer version gets a button to the download page; no network gets a calm
   message and a link. Only a strictly formatted version number is trusted from the reply, and the page it
   opens is fixed in the app. The Content Security Policy now allows that one host, and the opener plugin
   may open only the website. Preview: `PREVIEW_UPDATE=0.3.0` pretends to be that version.
2. **Research.** An agent checked every parsing dependency against RustSec, GitHub advisories and OSV (no
   match at our versions), surveyed 2018–2026 CVE classes in MuPDF, Poppler, PDFium, pdf.js and qpdf
   against this design, and listed public hostile corpora. It found three wrong claims in `SECURITY.md`:
   the app's Flate decoder is `zlib-rs`, not `miniz_oxide` (hayro turns it on); hayro is built with
   `unsafe` speed-ups and decodes JBIG2, CCITT and JPEG 2000; and the update check is network access.
   All three are corrected.
3. **Content-loss guard.** The research pointed at lopdf's lenient loading, and a new test confirmed it: an
   object with a syntax error was silently dropped and the copy written without it. Running the qpdf
   corpus with a text comparison then found two more cases: a stream with a missing or garbled `/Length`
   whose text was lost, and an incremental update that deletes a page's content, which lopdf brought back
   because it ignores free cross-reference entries. The engine now counts all three before writing (see
   `damage` in `lib.rs` and the new `revisions.rs`, which reads the classic cross-reference tables along
   the `/Prev` chain and stops at hybrid sections, whose free entries are placeholders: Word writes
   thousands). Any of them leaves the file unchanged with a new message, "part of this file is damaged".
   An early version wrongly refused a real Word report (survey.pdf); stopping at hybrid sections fixed it,
   and every real test file gives the same savings as before.
4. **Corpus.** The qpdf test suite (628 PDFs) and 78 qpdf fuzz crashers: no crash, hang or panic; at most
   440 MB and 0.8 s; 180 cleanly refused (encrypted, unreadable, damaged). Compressed with Balanced, all
   483 copies have exactly the original's text in MuPDF; before the guard, 3 did not. The renderer on the
   same files: at most 44 MB and 0.8 s.
5. **Fuzzing.** `examples/fuzz.rs`, a mutation fuzzer on stable Rust (no nightly here, so no cargo-fuzz):
   flipped bytes, hostile numbers, PDF tokens, deleted, repeated and spliced ranges, each input analysed
   with every preset on a 1 MB stack under a 3 GB cap. 200,000 inputs from 954 seeds (the small qpdf
   files, plain and uncompressed): no panic, hang or crash.
6. **CI.** `audit.yml` runs `cargo audit` on every push, pull request and weekly; only vulnerabilities fail
   it (five warnings come from Tauri's Linux window stack, none of which reads PDFs). The test job also
   runs the hostile tests with `zlib-rs`, the decoder the app ships.
7. **Minor release routine.** The 19-file hostile corpus: bombs peak at 324 MB, nested forms at 478 MB, the
   500,000-object file takes 9.0 s and 2.3 GB. All engine tests pass with both Flate decoders.
8. **Website.** A section, "Tested against the worst PDFs we could find", gives the numbers above and links
   to `SECURITY.md`. "Private and careful" became "Your files stay home", and the footer says "Your PDFs
   never leave home." The app screenshots were retaken with the update button.
9. **Badges.** Six pills under the download buttons, researched first: there is no standard badge for most
   of these claims, and "Not By AI" badges mean a work was made without AI, which is not true here, so none was
   borrowed. One icon set drawn for the page on a 24 px grid with a 1.75 stroke in the same tinted circle, the
   "no" icons sharing one slash. Each pill is a short label and a light half-line, linking to its proof where
   there is one. English labels are kept to 10–13 characters: Local only, Deterministic (Does just what you
   ask), Stress-tested, CVE-checked, No phone-home, Free to use. Korean labels use official, technical words
   (로컬 처리, 결정론적 처리, 스트레스 테스트, CVE 점검, 외부 통신 없음, 무상 제공) and the half-lines are the
   marketing voice (시킨 일만, 정확하게; 공짜. 진짜 있습니다.). "No AI inside" became "Deterministic", and
   "No tracking" was dropped, at the owner's request. "Free" means free of cost: the code has no license yet.
10. **"Details".** The folded block below the save button was called "Advanced" next to "Advanced settings";
    it is now "Details" (세부 정보), which says what it holds. The website's research-paper result was corrected
    to 777 KB (remeasured), and its qpdf figure now says "many deliberately broken" rather than calling all
    706 files hostile.
    On the page, the comparison ("See it before you save it") now follows the features, and the security
    section sits after the results with a smaller heading and plainer numbers, as supporting evidence; its
    Korean heading is 가장 까다로운 PDF로 검증했습니다.

## 0.5.0, 21 October 2026

The owner asked to make the 0.4.1 work downloadable. z versions are not tagged, so it became 0.5.0, with the
minor release routine.

1. **First-launch steps.** Visitors who clicked the top download button never saw how to get past macOS
   Gatekeeper or Windows SmartScreen: the steps were folded away in each download card. They are now a short
   numbered list in every card, and any download link also opens a small window, "Your download has started",
   with the steps for that system. The download itself starts as before.
2. **Security review.** The engine's parsing code did not change since 0.4.0, so there is no new path to add
   to `tests/hostile.rs`. All engine tests pass. The 19-file corpus was rerun by hand: every hostile file
   peaks at 446 MB or less, the legitimate 500,000-object file takes 9.3 s and 2.2 GB, and the two files with
   absurd predictor parameters are refused, all as in 0.4.0. The isolated renderer on the same corpus: the
   worst attacks end at its 1 GB cap in under 1.2 s, at 796 MB at most.
3. **Release.** `v0.5.0` is the first tag to run the new release steps: fixed-name installers for the website
   and the full Mac signature check.

## 0.4.1, 21 October 2026

The owner tested v0.4.0 on macOS (Intel and Apple silicon), Windows 11 and Linux Mint (the `.deb`, in a VM):
everything ran. Visual follow-ups:

1. **File names spilling.** On the Intel Mac a long name ran under the chip's ×. Some WebKit versions do not
   let a button shrink below its text in a flex row; the name now has an explicit width and its own
   ellipsis. Checked in Chrome and in a real WKWebView.
2. **Divider handle.** The swipe divider has a round handle with arrows, kept at the vertical middle of the
   part of the page in view while scrolling, zooming and resizing.
3. **Icon.** Apple's Liquid Glass is rendered live by the system from a layered Icon Composer file, which
   Tauri cannot package yet. A baked-in glass finish was drawn in two versions, glossy and soft, and the
   owner chose soft: a light rim brightest at the top, a sheen fading out over the upper half, and a slightly
   darker lower edge, all in `assets/icon.svg` over the unchanged artwork. Every platform icon was
   regenerated from it. The flat icon is kept as `assets/icon-flat.svg` and `icon-flat.png`. The rim
   defines the icon on dark backgrounds, so the header's CSS outline for the icon was removed.
4. **Product page.** `site/` is a one-page website in English and Korean (from the browser's language,
   a toggle, or `?lang=ko`), published by `.github/workflows/pages.yml` to
   https://yusungchang.github.io/pdf-epsilon/. It picks the download for the visitor's system (a Mac gets
   Apple silicon with Intel beside it, since browsers cannot tell them apart) and shows the latest
   release's version from the GitHub API. Its screenshots come from the preview tool, which gained
   `PREVIEW_SCALE` and `PREVIEW_CLEAN` and now forces the colour scheme from the first paint (the old flag
   was ignored and caught the drop area mid-transition). The download buttons link to
   `releases/latest/download/<name>`, which needs names without a version, so on tags `build.yml` also
   attaches copies named `PDF-Epsilon-macOS-AppleSilicon.dmg`, `PDF-Epsilon-Windows-Setup.exe` and so on.
   v0.4.0 predates this and has only versioned names.
5. **Voice, author, bug reports.** The page leads with the app's own tagline, and its copy was cut to
   one short line per feature in both languages (a more emotive headline was tried and reverted). The footer credits yusungchang, linked to LinkedIn, which is also the author in `Cargo.toml`,
   `package.json` and the installers' publisher and copyright. Bug reports and ideas come in through GitHub
   issue forms in both languages (`.github/ISSUE_TEMPLATE`), which warn that issues are public and ask
   people not to attach private PDFs; the footer links to them.
6. **Mac signature check.** `build.yml` now runs `codesign --verify --deep --strict` on the app and on the
   copy inside the dmg, so a build that macOS would call damaged fails in CI instead.
7. **Shorter report.** Below the save button, "What was found" and "Page by page" now sit in one block,
   "Advanced" (고급 기능), closed by default and remembered between launches. The settings panel inside the
   presets card, also called "Advanced" in English, became "Advanced settings" to match 고급 설정. The
   preview tool gained `PREVIEW_OPEN=1` to start that block open, and turns off CSS transitions so a
   screenshot never catches a panel mid-animation. The website screenshots were retaken.

## 0.4.0, 21 October 2026

The owner asked for four features, after an estimate of what each costs.

1. **Progress.** The engine reports progress (pages inspected, then each preset's images) through a callback;
   the analysis weighs inspection at 12% and each preset at 22%. The app forwards it over a Tauri channel,
   throttled, and the file chip and status card show a bar and a percentage.
2. **Thumbnails.** PDFs rarely carry their own (`/Thumb`; none of the eight test files has one), so pages are
   rendered with hayro 0.8 (MIT/Apache, pure Rust, no `unsafe`). It drew every test page correctly in 0–50 ms.
   Its security review found that it has no limits: on the hostile corpus it reached 2–4.5 GB, and an image
   claiming 2 billion × 2 billion pixels took it past 60 s at 10.9 GB. So it never runs in the app. The app
   starts its own executable with `--render-thumbnails`, which caps its heap at 1 GB with a counting global
   allocator (the same on every platform) and is killed after 8 s per page or 60 s per batch. Under attack
   it now stops in under a second at 11–796 MB. Rows request thumbnails as they scroll into view, 30 pages
   per renderer run; the page detail shows a larger one.
3. **Comparison, first per image, then per page.** A per-image comparison was built first (each image as
   stored and as the preset stores it). The owner then proposed comparing pages instead: there is no
   per-image tuning, and there should not be, so what matters is whether the page still looks right. That
   was adopted and the per-image code removed. `prepare` makes the compressed copy once with the current
   settings, with progress, and the app keeps it (three at most), so **saving writes exactly the version
   compared**. Both versions are drawn by the isolated renderer; the copy reaches it through a pipe and
   is never written to disk. The window offers a divider, side by side, or "Show differences", which
   marks pixels that changed by 32 of 255 or more, more strongly the more they changed, and reports the
   share at that zoom. It works through the page in strips with small canvases: at 4× on a Retina screen,
   page-sized canvases ran into WebKit's limit on canvas memory and came back blank, which marked the
   whole page; a blank canvas is now reported instead; zoom is Fit,
   2× or 4×, with previous and next page. A shortcut from each image row to its page, zoomed to the image,
   was tried and dropped at the owner's request: comparison is per page only. The window is resizable
   from its corner (CSS `resize`, which every platform's webview supports). Fit shows the whole page by
   width and height, the layout follows every resize at once, and the page is drawn again, sharper,
   when the window grows by more than a quarter.
4. **Language button.** The drop-down became a compact button: a globe and a short label, uppercase codes
   for Latin-script languages (EN) and native names for CJK (한국어, since 한글 names the script, not the
   language). The menu lists full names. It stays in the window's header rather than the title bar:
   Tauri can only overlay the title bar on macOS, and on Windows and Linux that would mean drawing
   custom window controls.
5. **Page list tools.** Only pages with issues, largest first, expand all and collapse all became icon
   buttons with tooltips drawn by the app (the same on every platform, unlike system tooltips); the first
   two are toggles that show when they are on.
6. **Comparison, refined.** A clear "Compare page" button in each page's detail, and clicking any page
   thumbnail compares that page, even when nothing on it changes. The window fills the app window and
   follows it as it is resized; "Original" and the preset's name are pinned to the swipe view's top
   corners; changing the zoom centres the view and puts the divider in the middle of the screen, and
   scrolling the divider out of sight brings it back to the middle of the view. The page behind stops
   scrolling, and hides its scroll bar, while the window is open. In "Show differences" the page
   underneath turns grey, so only real differences are in colour.
7. **Theme button** beside the language button: System (the default, following the operating system),
   Light and Dark, with monitor, sun and moon icons. A choice also sets the window frame's theme through
   Tauri where the platform allows it. Colours are defined once for light and once for dark, used by the
   system preference unless Light or Dark is chosen. The preview harness now passes its light or dark
   argument as this setting, so its screenshots no longer depend on the Mac's own appearance.
   In the dark theme the app icon gets a faint light edge and the header buttons a slightly stronger
   border, so neither disappears into the background.
8. **Removing files.** Each file chip has a ×. It takes the file off the list, and a file removed while it is
   being analysed is simply forgotten when the analysis returns.
9. **Minor release routine.** Security review as above, including the renderer drawing compressed copies.
   The 19-file analysis corpus (2 GB bombs) was rerun: every hostile file peaks at 486 MB or less, and the
   legitimate 500,000-object file takes 10 s and 2.2 GB, the same as 0.3.1. The renderer was also checked
   from inside the built `.app`. Real files are unchanged: identical text, same savings. `v0.4.0` is tagged
   once committed and green.

10. **Mac installers fixed before publishing.** The first v0.4.0 build said "damaged" when downloaded on macOS.
    The app carried only the linker's ad-hoc signature on its executable, which does not seal the bundle
    ("code has no resources but signature indicates they must be present"), and Gatekeeper treats a
    downloaded app with a broken signature as damaged. `bundle.macOS.signingIdentity: "-"` now has Tauri
    ad-hoc sign the whole bundle (with the hardened runtime); `codesign --verify --deep --strict` passes,
    and the thumbnail renderer still works and is still capped from the signed bundle. The unpublished
    draft was deleted and v0.4.0 rebuilt from the fix. Without an Apple Developer ID and notarization,
    macOS still asks for confirmation the first time (System Settings, Privacy & Security, Open Anyway).

## 0.3.1, 19 October 2026

The owner asked whether reading a PDF could be an attack vector, on every platform.

1. **Review.** PDF Epsilon never runs, renders or extracts a PDF, and its whole parsing path is Rust (lopdf,
   `miniz_oxide`, `zune-jpeg`), so the risk is crashing, not code execution. The interface inserts file text
   only as text, the CSP allows only the app's scripts, and output never overwrites.
2. **Attack corpus.** 17 hostile PDFs were built, one per path: decompression bombs in content, object
   streams, images, forms, fonts, glyph maps and metadata; huge predictors; a flood of drawing operations;
   cycles; deep nesting; lying font tables; huge JPEG headers. There were no panics, hangs or stack overflows,
   but memory was unbounded: a 12 KB file took 7.4 GB, and a 788-byte file 3.6 GB.
3. **Fix (`limits.rs`).** Every decode is bounded, and documents load with a stream limit. Operations are
   counted before decoding, images are capped at 100 megapixels, absurd predictor parameters are refused
   before loading, and files over 2 GB are refused. With these, the same corpus peaks at 477 MB, whether its
   bombs hold 512 MB or 2 GB.
4. **Quadratic lopdf code.** A 46 MB file with 500,000 objects took 108 s and 4.7 GB. Profiling found lopdf's
   `prune_objects` and `renumber_objects` checking visited objects against a list. Both were replaced with
   linear versions (103 s became 2 s for one preset). Documents with over 100,000 objects are analysed one
   preset at a time, and files estimated at over 1 million objects are refused before loading. Renumbering
   now turns references to missing objects into null, as the specification says, so they cannot point at
   a renumbered object.
5. **Tests.** `tests/hostile.rs` runs 20 hostile files on a 1 MB stack, smaller than any platform's default.
   CI now runs the tests on macOS, Windows and Linux. `docs/SECURITY.md` records the threat model.

## 0.3.0, 19 October 2026

The owner asked for font work first, and for deep research on other optimizations.

1. **Finding.** The Korean newspaper test file was 90% fonts, and its fonts were already subsets. Extracting them
   showed two 4 MB fonts made almost entirely of `bdat`/`bloc` screen bitmaps, which PDF viewers never read.
   Rebuilding them with only the tables viewers use measured 2.8 MB of compressed font data dropping to 41 KB.
2. **Font tables (`fonts.rs`).** Every embedded TrueType or OpenType font keeps only an allow list of tables: the
   ones ISO 32000 requires, plus cmap, OS/2, name, post, vertical metrics and colour glyphs. Bitmap-only fonts,
   collections and anything malformed are left alone. The newspaper went from 3.4 MB to 313 KB, losslessly.
3. **Subsetting (`subset.rs`).** Type 0 fonts with Identity-H/V encoding over an embedded TrueType CIDFont are
   subset to the glyphs drawn, keeping glyph numbers, so no text, width or ToUnicode changes. Text is collected
   from page content, form XObjects, patterns, soft masks, annotation appearances and Type 3 glyphs. Fonts in the
   form's default resources or set through a graphics state stay whole, and nothing is subset if any content
   fails to decode. Subset fonts get a six-letter tag. Two test PDFs built with whole macOS fonts went from
   15.2 MB to 80 KB and from 6.1 MB to 115 KB.
4. **Checks.** On all eight test files and the two big-font files, extracted text is identical and every page
   renders pixel-identical in MuPDF. macOS's renderer was also identical, apart from 4 pixels on the newspaper
   that differ by at most 2 levels.
5. **Analyzer.** New finding for font data no viewer reads. Fonts embedded in full are now "fixed" with measured
   savings, and repeated subsets are sized after stripping, so the newspaper no longer blames 1.5 MB on them.
6. **Options.** `stripFontTables` and `subsetFonts`, on in every preset, with checkboxes under Advanced.
   Settings saved earlier turn them on.
7. **Research.** Four research passes on images, fonts, structure and existing tools; the report is
   `reports/PDF size optimization techniques.md`.

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
7. **Git and CI.** Installed the GitHub CLI and logged in as `yusungchang`, adding the `workflow` scope so the
   pipeline can be pushed. First commit. Published as the public repository `yusungchang/pdf-epsilon`.
   Before publishing, the owner's drive paths for the test files were moved out of `HANDOVER.md` into the
   git-ignored `scratch/test-files.md`, and commits were rewritten to use the GitHub noreply email.
8. **New icon.** The owner supplied artwork of a crushed can (`assets/icon-art.png`), first marked ε, then
   replaced the same session with a photoreal aluminium can marked PDF. `assets/icon.svg`
   now clips it to the macOS rounded square, all platform icons were regenerated from it, and the window
   header shows it in place of the plain ε mark.
9. **First CI run.** Tests passed, and all four builds compiled and packaged: `.dmg` for both Macs, `.exe` and
   `.msi` for Windows, `.deb`, `.rpm` and `.AppImage` for Linux. Every upload failed because a relative
   `CARGO_TARGET_DIR` made Tauri write to `src-tauri/target.nosync`. The variable was removed.
10. **Languages.** All text moved into `ui/locales/en.json`, with a Korean `ko.json`. The engine now returns
    message keys with typed values (counts, sizes, lists, terms) instead of English sentences, and error
    codes instead of error text, so the interface formats numbers, sizes, lists and plurals per language.
    The `inspect` example reads `en.json` too. A language picker sits in the header; the first choice
    follows the system language. `tests/locales.rs` checks keys and placeholders across languages. The workflow now builds installers for all three platforms on every push
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

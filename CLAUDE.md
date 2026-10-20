# PDF Epsilon

Tauri 2 desktop app that makes PDFs smaller. Read `docs/HANDOVER.md` first: it has the
current state, verified results, design decisions and next steps. Then read `README.md`.

## Layout

- `crates/epsilon` is the engine. Pipeline order is in `src/lib.rs`.
  `analyze.rs` explains a file before it is changed (see `docs/ANALYZER.md`),
  `placement.rs` walks page content for drawn images, forms and fonts,
  `subset.rs` subsets fully embedded TrueType CID fonts, `fonts.rs` drops font tables
  no viewer reads, `images.rs` recompresses,
  `dedup.rs` merges identical streams, and `clean.rs` strips optional data.
- `src-tauri` is the shell with commands `presets`, `file_size`, `analyze` (with progress),
  `prepare` (the compressed copy for comparison, kept so saving writes exactly it), `render`,
  `forget` and `compress`. `render.rs` draws pages with hayro in a separate, memory-capped copy of
  the app (`--render-thumbnails`); never render in-process.
  Output files are written with `create_new` and are never overwritten.
- `ui` is plain JS with no bundler. Tauri APIs come from `window.__TAURI__`
  (`withGlobalTauri`).
- `ui/locales/<lang>.json` holds every user-visible string. The engine returns
  `Message { key, args }` (see `message.rs`) and error codes, never prose; the UI renders
  them with `t()`. New text goes into `en.json` and every other locale in the same change.

## Rules

- **Measure, don't predict.** The analyzer's savings come from running each preset in
  memory. Anything it cannot reduce yet is reported as "not handled yet", never as a saving.
- **Never risk content.** When the engine does not fully understand an image or
  structure, it leaves it untouched. Every output is reloaded and its page count
  checked. A result that is not smaller is discarded.
- **Every file is hostile.** Decode untrusted data only through `limits.rs` (never
  `decompressed_content()` or unbounded loads), and add a case to `tests/hostile.rs` for
  any new parsing path. See `docs/SECURITY.md`.
- **Encrypted input is refused.** The check runs on the raw bytes as well as through
  lopdf's flags, because lopdf decrypts some files on load and silently fails on others.
  Saving them produced a near-empty PDF before the raw check existed.
- **Build output stays out of iCloud.** The project lives in iCloud Drive. Build
  output goes to `target.nosync` (see `.cargo/config.toml`), and `node_modules` is a
  symlink to `node_modules.nosync`.

## Version

The version is `x.y.z`, kept in `Cargo.toml` (workspace), `package.json` and
`src-tauri/tauri.conf.json`. The owner's scheme:

- Every working session raises z by one.
- A meaningful change, such as a new feature or product, raises y instead and resets z to 0.
- x stays 0 until the owner says 1.0.

Every change is recorded in `docs/HISTORY.md`: one line in the versions table, and a
section listing what changed and why. Update `docs/HANDOVER.md` at the end of a session.

## Minor release routine

After every y increase (0.3 → 0.4), and only then, before calling the version done:

1. **Security review.** Check every new or changed parsing path against `docs/SECURITY.md`:
   all decoding goes through `limits.rs`, nothing recurses or loops without a bound. Add
   cases to `tests/hostile.rs` for new paths, rerun the hostile corpus by hand (bombs of
   2 GB, a 500,000-object file), and record peak memory and time in `HISTORY.md`.
2. **Release tag.** Once CI passes on `main`, tag `vX.Y.0` on that commit and push the tag,
   which builds a draft release with every installer. z versions (0.3.1) are not tagged. Raise z in all three files with each change, and add a line
to the Versions table in `docs/HISTORY.md`. Update `docs/HANDOVER.md` when the state changes.

## Check a change

- `cargo test -p epsilon --release` builds a synthetic PDF and checks downsampling,
  lossless mode, Print, and that the analysis accounts for every byte and matches real runs.
- `tests/locales.rs` fails when a key is used but missing, or a language's keys or
  placeholders differ from English.
- `cargo run -p epsilon --release --example inspect -- in.pdf --pages` prints the analysis.
  On real files, check that the categories add up to the file size (`--json`).
- To check against real files, run the `squeeze` example on PDFs with photos, line art
  and many pages. Compare `mutool draw -F txt` output before and after, which should
  be identical. Render pages with `mutool draw -r 110` and look at them.
- `tools/ui-preview/preview.sh out.png [light|dark]` screenshots the interface
  with a mocked Tauri bridge. The real window cannot be captured on this Mac.
- `npm run build` must finish. On macOS it produces `.app` and `.dmg`.

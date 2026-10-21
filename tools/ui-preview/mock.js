// Stands in for window.__TAURI__ so the interface runs in a plain browser.
// Analyses come from analyses.js, which preview.sh writes by running the real
// analyzer on the PDFs it is given. Saving returns a canned result.
const base = { dedupe: true, stripFontTables: true, subsetFonts: true, stripThumbnails: true, stripPrivateData: false, stripMetadata: false };
const P = [
  { id: "lossless", options: { ...base, images: null } },
  { id: "print", options: { ...base, images: { maxDpi: 300, jpegQuality: 85 } } },
  { id: "balanced", options: { ...base, images: { maxDpi: 150, jpegQuality: 75 }, stripPrivateData: true } },
  { id: "small", options: { ...base, images: { maxDpi: 96, jpegQuality: 60 }, stripPrivateData: true, stripMetadata: true } },
];
const analyses = window.__ANALYSES__ || {};
// preview.sh sets the language through PREVIEW_LANG.
if (window.__LANG__) localStorage.setItem("language", JSON.stringify(window.__LANG__));
// preview.sh passes light or dark as an explicit theme, so screenshots do not depend on the system.
if (window.__THEME__) localStorage.setItem("theme", JSON.stringify(window.__THEME__));
class MockChannel { constructor() { this.onmessage = () => {}; } }
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
window.__TAURI__ = {
  core: { Channel: MockChannel, invoke: async (cmd, args) => {
    if (cmd === "presets") return P;
    if (cmd === "analyze") {
      const name = args.path.split("/").pop();
      for (let i = 1; i <= 10; i++) { await wait(window.__SLOW__ ? 1500 : 20); args.progress.onmessage(i / 10); }
      if (!analyses[name]) throw { code: "encrypted", detail: "" };
      return analyses[name];
    }
    if (cmd === "render") {
      // Thumbnails for small widths; precomputed page renders (original or Balanced copy) otherwise.
      const name = args.path.split("/").pop();
      const r = window.__RENDERS__ || {};
      const big = args.width > 1400;
      const set = args.width <= 300 ? (window.__THUMBS__ || {})[name] || {}
        : r.name !== name ? {} : args.options ? (big ? r.zoomedCompressed : r.compressed) : (big ? r.zoomed : r.original);
      for (const page of args.pages) { await wait(5); args.each.onmessage({ page, png: (set || {})[page] || null }); }
      return null;
    }
    if (cmd === "prepare") {
      const a = analyses[args.path.split("/").pop()];
      for (let i = 1; i <= 5; i++) { await wait(20); args.progress.onmessage(i / 5); }
      const p = a.presets.find((x) => x.preset === "balanced");
      return { inputBytes: a.file.bytes, outputBytes: p.outputBytes, keptOriginal: p.keptOriginal };
    }
    if (cmd === "forget") return null;
    if (cmd === "compress") {
      const a = analyses[args.job.input.split("/").pop()];
      const p = a.presets.find((x) => x.preset === "balanced");
      return { output: args.job.input.replace(/\.pdf$/, "-epsilon.pdf"),
               report: { inputBytes: a.file.bytes, outputBytes: p.outputBytes, keptOriginal: false } };
    }
  } },
  dialog: { open: async () => null, ask: async () => true },
  opener: { revealItemInDir: async () => {} },
  webview: { getCurrentWebview: () => ({ onDragDropEvent: async () => {} }) },
};
setTimeout(async () => {
  const names = Object.keys(analyses);
  // A fake encrypted file shows the error state, except in clean screenshots (PREVIEW_CLEAN=1).
  addFiles([...names.map((n) => `/x/${n}`), ...(window.__CLEAN__ ? [] : ["/x/locked-report.pdf"])]);
  await new Promise((r) => setTimeout(r, 600));
  select(0);
  if (window.__SAVE__) await saveCopy();
  if (window.__COMPARE__) {
    // Page 2, or with differences shown for PREVIEW_COMPARE=diff.
    const f = state.files[0];
    const page = f.analysis.pages[1] || f.analysis.pages[0];
    await openCompare(f, page.number);
    if (window.__COMPARE__ === "diff") { await wait(300); document.querySelector(".cmp-check input").click(); }
    if (window.__COMPARE__ === "diff4") {
      await wait(300);
      [...document.querySelectorAll(".cmp-tools .segmented button")].find((b) => b.textContent === "4×").click();
      await wait(400);
      document.querySelector(".cmp-check input").click();
    }
    return;
  }
  // Expand the heaviest page so its detail shows in the screenshot.
  const a = state.files[0].analysis;
  if (!a) return;
  const top = [...a.pages].sort((x, y) => y.bytes - x.bytes)[0];
  if (top) { state.open.add(top.number); renderPages(a); }
}, 200);

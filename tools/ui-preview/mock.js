// Stands in for window.__TAURI__ so the interface runs in a plain browser.
// Analyses come from analyses.js, which preview.sh writes by running the real
// analyzer on the PDFs it is given. Saving returns a canned result.
const base = { dedupe: true, stripThumbnails: true, stripPrivateData: false, stripMetadata: false };
const P = [
  { id: "lossless", options: { ...base, images: null } },
  { id: "print", options: { ...base, images: { maxDpi: 300, jpegQuality: 85 } } },
  { id: "balanced", options: { ...base, images: { maxDpi: 150, jpegQuality: 75 }, stripPrivateData: true } },
  { id: "small", options: { ...base, images: { maxDpi: 96, jpegQuality: 60 }, stripPrivateData: true, stripMetadata: true } },
];
const analyses = window.__ANALYSES__ || {};
// preview.sh sets the language through PREVIEW_LANG.
if (window.__LANG__) localStorage.setItem("language", JSON.stringify(window.__LANG__));
window.__TAURI__ = {
  core: { invoke: async (cmd, args) => {
    if (cmd === "presets") return P;
    if (cmd === "analyze") {
      const name = args.path.split("/").pop();
      await new Promise((r) => setTimeout(r, 50));
      if (!analyses[name]) throw { code: "encrypted", detail: "" };
      return analyses[name];
    }
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
  addFiles([...names.map((n) => `/x/${n}`), "/x/locked-report.pdf"]);
  await new Promise((r) => setTimeout(r, 600));
  select(0);
  if (window.__SAVE__) await saveCopy();
  // Expand the heaviest page so its detail shows in the screenshot.
  const a = state.files[0].analysis;
  const top = [...a.pages].sort((x, y) => y.bytes - x.bytes)[0];
  if (top) { state.open.add(top.number); renderPages(a); }
}, 200);

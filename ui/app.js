// PDF Epsilon interface. Plain script: Tauri's APIs come from window.__TAURI__.
const { invoke } = window.__TAURI__.core;
const { open, ask } = window.__TAURI__.dialog;
const { revealItemInDir } = window.__TAURI__.opener;
const { getCurrentWebview } = window.__TAURI__.webview;

const PRESETS = ["lossless", "print", "balanced", "small"];
const NOTES = {
  lossless: "No pixel is changed. Removes clutter and repacks the file.",
  print: "Images kept up to 300 dpi, JPEG quality 85. For printing.",
  balanced: "Images kept up to 150 dpi, JPEG quality 75. Sharp on any screen.",
  small: "Images kept up to 96 dpi, JPEG quality 60, metadata removed. For email and upload limits.",
};
const CATEGORIES = {
  images: "Images",
  fonts: "Fonts",
  pageContent: "Text and drawing",
  vectorGraphics: "Vector graphics",
  annotations: "Links, comments, forms",
  attachments: "Attachments",
  thumbnails: "Thumbnails",
  editorData: "Editor data",
  metadata: "Metadata",
  tagging: "Accessibility tags",
  navigation: "Bookmarks",
  colourProfiles: "Colour profiles",
  structure: "Structure",
  unused: "Unused objects",
  overhead: "Overhead",
};
const SAVINGS = {
  images: "Images",
  duplicates: "Duplicates",
  thumbnails: "Thumbnails",
  editorData: "Editor data",
  metadata: "Metadata",
  unused: "Unused objects",
  rewrite: "Repacking",
};
// Page composition, in the order drawn.
const PARTS = [
  ["content", "pageContent", "Text and drawing"],
  ["images", "images", "Images"],
  ["fonts", "fonts", "Fonts"],
  ["graphics", "vectorGraphics", "Vector graphics"],
  ["other", "annotations", "Other"],
];
const PAGE_BATCH = 60;

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
const state = {
  presets: {},          // id -> options, from the engine
  preset: "balanced",
  options: null,        // what will actually be used
  custom: false,
  outputDir: null,      // null: next to the original
  files: [],            // { path, status, analysis, error, saved }
  current: -1,
  queue: [],
  busy: false,
  open: new Set(),      // expanded page numbers
  shown: PAGE_BATCH,
};

function load(key, fallback) {
  try { const v = localStorage.getItem(key); return v ? JSON.parse(v) : fallback; } catch { return fallback; }
}
function save(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable */ }
}

function formatBytes(n) {
  const neg = n < 0;
  n = Math.abs(n);
  let s;
  if (n < 1024) s = `${n} B`;
  else {
    const units = ["KB", "MB", "GB"];
    let v = n / 1024, i = 0;
    while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
    s = `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
  }
  return neg ? `−${s}` : s;
}
const pct = (part, whole) => (whole ? (100 * part) / whole : 0);
const formatPct = (p) => `${p < 10 ? p.toFixed(1) : Math.round(p)}%`;
const basename = (p) => p.split(/[\\/]/).pop();
const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
const title = (id) => id[0].toUpperCase() + id.slice(1);
const presetIndex = () => PRESETS.indexOf(state.preset);

// ---- settings ----

function choosePreset(id) {
  state.preset = id;
  state.custom = false;
  state.options = structuredClone(state.presets[id]);
  renderSettings();
  renderReport();
  persist();
}

function renderSettings() {
  $("preset-note").textContent = state.custom
    ? `Custom settings, starting from ${title(state.preset)}. The saved size is measured when you save.`
    : NOTES[state.preset];

  const o = state.options;
  const img = o.images || state.presets.balanced.images;
  $("images-on").checked = !!o.images;
  $("dpi").value = Math.round(img.maxDpi);
  $("quality").value = img.jpegQuality;
  $("quality-out").textContent = img.jpegQuality;
  for (const id of ["dpi", "quality"]) $(id).closest("label").classList.toggle("disabled", !o.images);
  $("dedupe").checked = o.dedupe;
  $("thumbs").checked = o.stripThumbnails;
  $("private").checked = o.stripPrivateData;
  $("metadata").checked = o.stripMetadata;

  $("out-same").classList.toggle("on", !state.outputDir);
  $("out-pick").classList.toggle("on", !!state.outputDir);
  $("out-pick").textContent = state.outputDir ? basename(state.outputDir) : "choose a folder…";
  $("out-pick").title = state.outputDir || "";
}

function readAdvanced() {
  const dpi = Math.min(1200, Math.max(36, Number($("dpi").value) || 150));
  state.options = {
    images: $("images-on").checked ? { maxDpi: dpi, jpegQuality: Number($("quality").value) } : null,
    dedupe: $("dedupe").checked,
    stripThumbnails: $("thumbs").checked,
    stripPrivateData: $("private").checked,
    stripMetadata: $("metadata").checked,
  };
  state.custom = JSON.stringify(state.options) !== JSON.stringify(state.presets[state.preset]);
  renderSettings();
  renderPresets();
  persist();
}

function persist() {
  save("settings", { preset: state.preset, custom: state.custom, options: state.options, outputDir: state.outputDir });
}

// ---- files ----

function addFiles(paths) {
  const pdfs = paths.filter((p) => /\.pdf$/i.test(p));
  if (!pdfs.length) return;
  for (const path of pdfs) {
    let i = state.files.findIndex((f) => f.path === path);
    if (i < 0) {
      state.files.push({ path, status: "waiting", analysis: null, error: null, saved: null });
      i = state.files.length - 1;
      state.queue.push(i);
    }
    if (state.current < 0 || pdfs.length === 1) select(i);
  }
  renderFiles();
  pump();
}

async function pump() {
  if (state.busy) return;
  state.busy = true;
  while (state.queue.length) {
    const i = state.queue.shift();
    const f = state.files[i];
    f.status = "analysing";
    renderFiles();
    if (i === state.current) renderReport();
    try {
      f.analysis = await invoke("analyze", { path: f.path });
      f.status = "ready";
    } catch (e) {
      f.error = String(e);
      f.status = "error";
    }
    renderFiles();
    if (i === state.current) renderReport();
  }
  state.busy = false;
}

function select(i) {
  state.current = i;
  state.open.clear();
  state.shown = PAGE_BATCH;
  renderFiles();
  renderReport();
  window.scrollTo({ top: 0 });
}

function renderFiles() {
  const nav = $("files");
  nav.hidden = state.files.length === 0;
  $("drop").classList.toggle("compact", state.files.length > 0);
  nav.replaceChildren(...state.files.map((f, i) => {
    const b = el("button", `file${i === state.current ? " on" : ""}`);
    b.type = "button";
    b.title = f.path;
    b.append(el("span", "file-name", basename(f.path)));
    let sub = "";
    if (f.status === "waiting") sub = "waiting";
    else if (f.status === "analysing") sub = "analysing…";
    else if (f.status === "error") sub = "can't read";
    else {
      const best = Math.max(...f.analysis.presets.map((p) => f.analysis.file.bytes - p.outputBytes));
      sub = best > 0 ? `up to −${formatPct(pct(best, f.analysis.file.bytes))}` : "already lean";
    }
    b.append(el("span", `file-sub${f.status === "error" ? " error" : ""}`, sub));
    b.onclick = () => select(i);
    return b;
  }));
}

// ---- report ----

function renderReport() {
  const f = state.files[state.current];
  const status = $("status");
  if (!f || f.status !== "ready") {
    $("report").hidden = true;
    status.hidden = !f;
    if (f) {
      status.replaceChildren();
      if (f.status === "error") {
        status.append(el("strong", "", basename(f.path)), el("br"), el("span", "error", f.error));
      } else {
        status.append(el("span", "spinner"), ` Analysing ${basename(f.path)}…`);
      }
    }
    return;
  }
  status.hidden = true;
  $("report").hidden = false;
  const a = f.analysis;

  $("file-name").textContent = basename(f.path);
  $("file-name").title = f.path;
  const facts = [plural(a.file.pages, "page"), `PDF ${a.file.version}`];
  if (a.file.producer || a.file.creator) facts.push(a.file.producer || a.file.creator);
  if (a.file.revisions > 1) facts.push(`saved ${a.file.revisions} times`);
  $("file-facts").textContent = facts.join(" · ");
  $("file-bytes").textContent = formatBytes(a.file.bytes);

  renderWarnings(a);
  renderUsage(a);
  renderPresets();
  renderSaveResult(f);
}

function renderWarnings(a) {
  const box = $("warnings");
  const warnings = a.findings.filter((x) => x.status === "warning");
  box.replaceChildren(...warnings.map((w) => {
    const d = el("div", "warning");
    d.append(el("strong", "", w.title), el("span", "", ` ${w.detail}`));
    return d;
  }));
  if (a.file.unreadablePages.length) {
    const d = el("div", "warning");
    d.append(el("strong", "", "Some pages could not be fully read."),
      el("span", "", ` Figures for page${a.file.unreadablePages.length === 1 ? "" : "s"} ${a.file.unreadablePages.slice(0, 10).join(", ")} are incomplete.`));
    box.append(d);
  }
}

function renderUsage(a) {
  const total = a.file.bytes;
  // Slivers under 1% are folded into one "Other" entry so the bar stays readable.
  const main = a.categories.filter((c) => pct(c.bytes, total) >= 1);
  const rest = a.categories.filter((c) => pct(c.bytes, total) < 1);
  const rows = main.map((c) => ({ key: c.category, label: CATEGORIES[c.category] || c.category, bytes: c.bytes }));
  if (rest.length) {
    rows.push({
      key: "other",
      label: "Other",
      bytes: rest.reduce((s, c) => s + c.bytes, 0),
      detail: rest.map((c) => `${CATEGORIES[c.category] || c.category} ${formatBytes(c.bytes)}`).join(", "),
    });
  }
  const bar = $("usage-bar");
  bar.setAttribute("aria-label", rows.map((r) => `${r.label} ${formatPct(pct(r.bytes, total))}`).join(", "));
  bar.replaceChildren(...rows.map((r) => {
    const s = el("span", `seg c-${r.key}`);
    s.style.flexGrow = String(r.bytes);
    s.title = `${r.label}: ${formatBytes(r.bytes)}`;
    return s;
  }));
  $("usage").replaceChildren(...rows.map((r) => {
    const li = el("li");
    li.append(el("span", `swatch c-${r.key}`), el("span", "u-label", r.label),
      el("span", "u-bytes", formatBytes(r.bytes)), el("span", "u-pct", formatPct(pct(r.bytes, total))));
    if (r.detail) li.title = r.detail;
    return li;
  }));
}

function renderPresets() {
  const f = state.files[state.current];
  if (!f || f.status !== "ready") return;
  const a = f.analysis;
  const total = a.file.bytes;
  const box = $("presets");
  box.classList.toggle("custom", state.custom);
  box.replaceChildren(...a.presets.map((p) => {
    const saved = total - p.outputBytes;
    const b = el("button", "preset");
    b.type = "button";
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(p.preset === state.preset));
    b.append(el("span", "p-name", title(p.preset)));
    if (p.error) {
      b.append(el("span", "p-size error", "failed"));
      b.title = p.error;
    } else if (saved <= 0) {
      b.append(el("span", "p-size", formatBytes(total)), el("span", "p-save none", "no saving"));
    } else {
      b.append(el("span", "p-size", formatBytes(p.outputBytes)), el("span", "p-save", `−${formatPct(pct(saved, total))}`));
    }
    const meter = el("span", "p-meter");
    const fill = el("span");
    fill.style.width = `${Math.max(0, 100 - pct(saved, total))}%`;
    meter.append(fill);
    b.append(meter);
    b.onclick = () => choosePreset(p.preset);
    return b;
  }));

  // Findings and pages show savings for the chosen preset.
  renderFindings(a);
  renderPages(a);

  const p = a.presets[presetIndex()];
  const parts = (p.savings || []).filter((s) => s.bytes !== 0).sort((x, y) => y.bytes - x.bytes);
  const bd = $("breakdown");
  bd.replaceChildren();
  if (state.custom) return;
  if (parts.length) {
    bd.append(el("span", "bd-lead", `${title(p.preset)} saves ${formatBytes(total - p.outputBytes)}:`));
    for (const s of parts) bd.append(el("span", "bd-item", `${SAVINGS[s.kind] || s.kind} ${formatBytes(s.bytes)}`));
  } else if (!p.error) {
    bd.append(el("span", "bd-lead", "This preset can't make the file smaller."));
  }
}

function renderSaveResult(f) {
  const box = $("save-result");
  box.replaceChildren();
  box.hidden = !f.saved;
  if (!f.saved) return;
  const { output, report: r, error } = f.saved;
  if (error) {
    box.append(el("span", "error", error));
  } else if (!output) {
    box.append(el("span", "", "No smaller version was possible with these settings. Nothing was written."));
  } else {
    box.append(el("span", "saving good", `−${formatPct(pct(r.inputBytes - r.outputBytes, r.inputBytes))}`),
      el("span", "", ` ${formatBytes(r.inputBytes)} → ${formatBytes(r.outputBytes)}. Saved as `),
      el("strong", "", basename(output)));
    const btn = el("button", "reveal", navigator.platform.startsWith("Mac") ? "Show in Finder" : "Show file");
    btn.type = "button";
    btn.onclick = () => revealItemInDir(output);
    box.append(btn);
  }
}

async function saveCopy() {
  const f = state.files[state.current];
  if (!f || f.status !== "ready") return;
  const a = f.analysis;
  if (a.file.signed) {
    const go = await ask("This document is digitally signed. The smaller copy will not carry a valid signature. The original is kept as it is.",
      { title: "Save a copy without a valid signature?", kind: "warning", okLabel: "Save copy", cancelLabel: "Cancel" });
    if (!go) return;
  }
  const btn = $("save");
  btn.disabled = true;
  btn.textContent = "Saving…";
  try {
    f.saved = await invoke("compress", { job: { input: f.path, outputDir: state.outputDir, options: state.options } });
  } catch (e) {
    f.saved = { error: String(e) };
  }
  btn.disabled = false;
  btn.textContent = "Save smaller copy";
  if (state.files[state.current] === f) renderSaveResult(f);
}

function statusPill(x) {
  const i = presetIndex();
  if (x.status === "fixed") {
    const here = x.savings[i];
    if (here > 0 && !state.custom) return ["good", `−${formatBytes(here)} with ${title(state.preset)}`];
    const best = x.savings.reduce((b, s, j) => (s > x.savings[b] ? j : b), 0);
    if (x.savings[best] > 0) return ["muted", `${title(PRESETS[best])} saves ${formatBytes(x.savings[best])}`];
    return ["good", "fixed by repacking"];
  }
  if (x.status === "notYet") return ["warn", "not handled yet"];
  if (x.status === "kept") return ["muted", "kept"];
  return ["bad", "warning"];
}

function renderFindings(a) {
  const list = a.findings.filter((x) => x.status !== "warning");
  const ul = $("findings");
  if (!list.length) {
    ul.replaceChildren(el("li", "empty", "Nothing stands out. This file is already lean."));
    return;
  }
  ul.replaceChildren(...list.map((x) => {
    const li = el("li", `finding sev-${x.severity}`);
    const head = el("div", "f-head");
    const [tone, text] = statusPill(x);
    head.append(el("span", "f-title", x.title), el("span", "f-bytes", formatBytes(x.bytes)), el("span", `pill ${tone}`, text));
    li.append(head, el("p", "f-detail", x.detail));
    if (x.pages.length) {
      const pages = el("div", "f-pages");
      pages.append(el("span", "", x.pages.length === 1 ? "Page" : "Pages"));
      for (const n of x.pages.slice(0, 24)) {
        const b = el("button", "chip", String(n));
        b.type = "button";
        b.onclick = () => jumpToPage(n);
        pages.append(b);
      }
      if (x.pages.length > 24) pages.append(el("span", "", `and ${x.pages.length - 24} more`));
      li.append(pages);
    }
    return li;
  }));
}

function jumpToPage(n) {
  $("only-issues").checked = false;
  $("largest").checked = false;
  state.open.add(n);
  const f = state.files[state.current];
  state.shown = Math.max(state.shown, n + 10);
  renderPages(f.analysis);
  document.querySelector(`[data-page="${n}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" });
}

function renderPages(a) {
  const legend = $("page-legend");
  legend.replaceChildren(...PARTS.map(([, cls, label]) => {
    const s = el("span", "lg");
    s.append(el("span", `swatch c-${cls}`), label);
    return s;
  }));

  const i = presetIndex();
  let pages = a.pages;
  if ($("only-issues").checked) pages = pages.filter((p) => p.issues.length);
  if ($("largest").checked) pages = [...pages].sort((x, y) => y.bytes - x.bytes);
  const max = Math.max(1, ...a.pages.map((p) => p.bytes));
  const images = new Map(a.images.map((im) => [im.obj, im]));
  const fonts = new Map(a.fonts.map((ft) => [ft.obj, ft]));

  const box = $("pages");
  const shown = pages.slice(0, state.shown);
  box.replaceChildren(...shown.map((p) => pageRow(p, max, i, images, fonts)));
  if (!pages.length) box.append(el("p", "empty", "No page has an issue."));
  const more = $("more-pages");
  more.hidden = pages.length <= state.shown;
  more.textContent = `Show ${Math.min(PAGE_BATCH, pages.length - state.shown)} more of ${pages.length - state.shown} remaining`;
}

function pageRow(p, max, i, images, fonts) {
  const wrap = el("div", "page");
  wrap.dataset.page = p.number;
  const row = el("button", "page-row");
  row.type = "button";
  row.setAttribute("aria-expanded", String(state.open.has(p.number)));
  const bar = el("span", "p-bar");
  for (const [key, cls, label] of PARTS) {
    if (!p[key]) continue;
    const s = el("span", `seg c-${cls}`);
    s.style.width = `${(100 * p[key]) / max}%`;
    s.title = `${label}: ${formatBytes(p[key])}`;
    bar.append(s);
  }
  const saving = state.custom ? 0 : p.savings[i];
  row.append(
    el("span", "p-no", String(p.number)),
    bar,
    el("span", "p-bytes", formatBytes(p.bytes)),
    el("span", `p-saving${saving > 0 ? " good" : ""}`, saving > 0 ? `−${formatBytes(saving)}` : "–"),
    el("span", `p-issues${p.issues.some((x) => x.status === "notYet") ? " warn" : ""}`, p.issues.length ? plural(p.issues.length, "issue") : ""),
  );
  row.onclick = () => {
    if (state.open.has(p.number)) state.open.delete(p.number); else state.open.add(p.number);
    wrap.replaceWith(pageRow(p, max, i, images, fonts));
  };
  wrap.append(row);
  if (state.open.has(p.number)) wrap.append(pageDetail(p, i, images, fonts));
  return wrap;
}

function pageDetail(p, i, images, fonts) {
  const d = el("div", "page-detail");
  const size = `${Math.round(p.width / 72 * 25.4)} × ${Math.round(p.height / 72 * 25.4)} mm`;
  const parts = PARTS.filter(([k]) => p[k]).map(([k, , label]) => `${label.toLowerCase()} ${formatBytes(p[k])}`);
  d.append(el("p", "pd-facts", `${size} · ${parts.join(" · ") || "empty"}`));

  if (p.issues.length) {
    const ul = el("ul", "pd-issues");
    for (const x of p.issues) {
      const li = el("li", `st-${x.status}`);
      li.append(el("span", "pd-dot"), el("span", "", x.text));
      ul.append(li);
    }
    d.append(ul);
  }

  const imgs = p.imageObjs.map((o) => images.get(o)).filter((im) => im && !im.isMask);
  if (imgs.length) {
    const t = el("table", "pd-table");
    const head = el("tr");
    for (const h of ["Image", "Stored as", "Size", "As shown", `With ${state.custom ? "preset" : title(state.preset)}`]) head.append(el("th", "", h));
    t.append(head);
    for (const im of imgs) {
      const tr = el("tr");
      const r = im.results[i];
      let after = "unchanged";
      if (im.unsupported) after = `kept: ${im.unsupported}`;
      else if (r && !state.custom) after = `${r.downsampled ? `${r.width}×${r.height}, ` : "re-encoded, "}${formatBytes(r.bytes)}`;
      const shown = [im.ppi ? `${Math.round(im.ppi)} ppi` : "not found drawn"];
      if (im.visible !== null && im.visible < 0.995) shown.push(`${Math.round(im.visible * 100)}% visible`);
      if (im.pages.length > 1) shown.push(`on ${im.pages.length} pages`);
      tr.append(
        el("td", "", `${im.width}×${im.height} ${im.colour}${im.hasSoftMask ? " + mask" : ""}`),
        el("td", "", im.encoding.replace(/Decode/g, "")),
        el("td", "num", formatBytes(im.bytes)),
        el("td", "", shown.join(", ")),
        el("td", r && !state.custom ? "good" : "muted", after),
      );
      t.append(tr);
    }
    d.append(t);
  }

  const fs = p.fontObjs.map((o) => fonts.get(o)).filter(Boolean);
  if (fs.length) {
    const names = fs.map((ft) => `${ft.name}${ft.embedded ? (ft.subset ? "" : " (full)") : " (not embedded)"}`);
    d.append(el("p", "pd-fonts", `Fonts: ${[...new Set(names)].join(", ")}`));
  }
  return d;
}

// ---- wiring ----

async function init() {
  for (const p of await invoke("presets")) state.presets[p.id] = p.options;

  const saved = load("settings", null);
  if (saved && state.presets[saved.preset]) {
    state.preset = saved.preset;
    state.custom = !!saved.custom;
    state.options = saved.custom && saved.options ? saved.options : structuredClone(state.presets[saved.preset]);
    state.outputDir = saved.outputDir || null;
  } else {
    state.options = structuredClone(state.presets.balanced);
  }
  renderSettings();

  for (const id of ["images-on", "dpi", "quality", "dedupe", "thumbs", "private", "metadata"]) {
    $(id).addEventListener("input", readAdvanced);
  }
  $("out-same").addEventListener("click", () => { state.outputDir = null; renderSettings(); persist(); });
  $("out-pick").addEventListener("click", async () => {
    const dir = await open({ directory: true, multiple: false, title: "Save smaller PDFs to" });
    if (dir) { state.outputDir = dir; renderSettings(); persist(); }
  });
  $("save").addEventListener("click", saveCopy);
  for (const id of ["only-issues", "largest"]) {
    $(id).addEventListener("input", () => { state.shown = PAGE_BATCH; renderPages(state.files[state.current].analysis); });
  }
  $("more-pages").addEventListener("click", () => { state.shown += PAGE_BATCH; renderPages(state.files[state.current].analysis); });

  $("drop").addEventListener("click", async () => {
    const files = await open({ multiple: true, title: "Choose PDFs", filters: [{ name: "PDF", extensions: ["pdf", "PDF"] }] });
    if (files) addFiles(Array.isArray(files) ? files : [files]);
  });

  await getCurrentWebview().onDragDropEvent((event) => {
    const { type, paths } = event.payload;
    if (type === "enter" || type === "over") $("drop").classList.add("over");
    else if (type === "leave") $("drop").classList.remove("over");
    else if (type === "drop") { $("drop").classList.remove("over"); addFiles(paths || []); }
  });
}

init().catch((e) => { const s = $("status"); s.hidden = false; s.textContent = `Could not start: ${e}`; });

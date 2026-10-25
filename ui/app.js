// PDF Epsilon interface. Plain script: Tauri's APIs come from window.__TAURI__.
// Every word shown comes from ui/locales/<language>.json through t(); see "Languages" below.
const { invoke, Channel } = window.__TAURI__.core;
const { open, ask } = window.__TAURI__.dialog;
const { revealItemInDir } = window.__TAURI__.opener;
const { getCurrentWebview } = window.__TAURI__.webview;

// To add a language, add ui/locales/<code>.json with the same keys as en.json and list it here,
// under its own name for itself.
// `short` labels the compact switcher: an uppercase code for Latin-script languages, the
// native name for CJK languages, whose names are already short (日本語, 中文, 한국어).
const LANGUAGES = { en: { name: "English", short: "EN" }, ko: { name: "한국어", short: "한국어" } };

const PRESETS = ["lossless", "print", "balanced", "small"];
// Page composition, in the order drawn: page field, colour class, label key.
const PARTS = [
  ["content", "pageContent", "part.content"],
  ["images", "images", "part.images"],
  ["fonts", "fonts", "part.fonts"],
  ["graphics", "vectorGraphics", "part.graphics"],
  ["other", "annotations", "part.other"],
];
const PAGE_BATCH = 60;
// Thumbnails: pixel width rendered (sharp at 2x in the detail view), and pages per renderer run.
const THUMB_WIDTH = 240;
const THUMB_BATCH = 30;

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

// Text where names are marked [[like this]] (preset names inside sentences): each name is
// drawn as a small bordered tag, so it stands apart from the words around it in any language.
function rich(node, text) {
  const parts = String(text).split(/\[\[|\]\]/);
  node.replaceChildren(...parts.map((p, i) => (i % 2 ? el("span", "name-tag", p) : p)).filter((p) => p !== ""));
  return node;
}
const richEl = (tag, cls, text) => rich(el(tag, cls), text);
const state = {
  presets: {},          // id -> options, from the engine
  preset: "balanced",
  options: null,        // what will actually be used
  custom: false,
  outputDir: null,      // null: next to the original
  files: [],            // { path, status, progress, analysis, error, saved, thumbs, removed }
  current: -1,
  queue: [],            // files waiting for analysis
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

// ---- languages ----

const i18n = { lang: "en", strings: {}, english: {} };

async function fetchLocale(lang) {
  const r = await fetch(`locales/${lang}.json`);
  if (!r.ok) throw new Error(`locales/${lang}.json: ${r.status}`);
  return r.json();
}

function pickLanguage() {
  const saved = load("language", null);
  if (saved && LANGUAGES[saved]) return saved;
  for (const tag of navigator.languages || [navigator.language]) {
    const code = String(tag).toLowerCase().split("-")[0];
    if (LANGUAGES[code]) return code;
  }
  return "en";
}

async function setLanguage(lang) {
  if (!Object.keys(i18n.english).length) i18n.english = await fetchLocale("en");
  i18n.strings = lang === "en" ? i18n.english : await fetchLocale(lang);
  i18n.lang = lang;
  document.documentElement.lang = lang;
  translateStatic();
}

// Looks a key up in the chosen language, then English. A value can be a string or, for
// messages with a count, an object of plural forms ("one", "other", ...) chosen by Intl.PluralRules.
function t(key, args = {}) {
  let v = i18n.strings[key] ?? i18n.english[key];
  if (v === undefined) return key.split(".").pop();
  if (typeof v === "object") {
    v = v[new Intl.PluralRules(i18n.lang).select(Number(args.count ?? 0))] ?? v.other;
  }
  return v.replace(/\{(\w+)\}/g, (all, name) => (name in args ? formatArg(args[name]) : all));
}

// Message arguments from the engine: numbers, text, lists, {bytes} sizes and {term} keys.
function formatArg(a) {
  if (typeof a === "number") return formatNumber(a);
  if (Array.isArray(a)) return new Intl.ListFormat(i18n.lang, { type: "conjunction" }).format(a);
  if (a && typeof a === "object") {
    if ("bytes" in a) return formatBytes(a.bytes);
    if ("term" in a) return t(a.term);
  }
  return String(a);
}

// An engine message: { key, args }.
const say = (m) => t(m.key, m.args || {});

function translateStatic() {
  document.querySelectorAll("[data-i18n]").forEach((e) => { e.textContent = t(e.dataset.i18n); });
  // Icon buttons carry their label as an accessible name and as a tooltip.
  document.querySelectorAll("[data-i18n-label]").forEach((e) => {
    e.setAttribute("aria-label", t(e.dataset.i18nLabel));
    if (e.classList.contains("icon-btn")) e.dataset.tip = t(e.dataset.i18nLabel);
  });
}

function errorText(e) {
  if (e && typeof e === "object" && e.code) return t(`error.${e.code}`) + (e.detail ? ` (${e.detail})` : "");
  return String(e);
}

// ---- formatting ----

function formatNumber(n, digits = 0, fixed = false) {
  return new Intl.NumberFormat(i18n.lang, { maximumFractionDigits: digits, minimumFractionDigits: fixed ? digits : 0 }).format(n);
}
function formatBytes(n) {
  const sign = n < 0 ? "−" : "";
  n = Math.abs(n);
  if (n < 1024) return `${sign}${formatNumber(n)} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${sign}${formatNumber(v, v < 10 ? 1 : 0, true)} ${units[i]}`;
}
const pct = (part, whole) => (whole ? (100 * part) / whole : 0);
const formatPct = (p) => new Intl.NumberFormat(i18n.lang, { style: "percent", maximumFractionDigits: p < 10 ? 1 : 0 }).format(p / 100);
const basename = (p) => p.split(/[\\/]/).pop();
const presetName = (id) => t(`preset.${id}`);

// Work the app can stop (analysis, a comparison copy, a save) carries a ticket.
let lastTicket = 0;
const newTicket = () => ++lastTicket;
const cancelWork = (ticket) => { if (ticket) invoke("cancel", { ticket }).catch(() => {}); };

// Re-renders that replace the focused element give focus back to its replacement, found by
// its data-focus key, so keyboard users keep their place.
function keepFocus(render) {
  const key = document.activeElement?.dataset?.focus;
  render();
  if (key) document.querySelector(`[data-focus="${CSS.escape(key)}"]`)?.focus();
}

// A short message under the drop area, gone after a few seconds.
let noticeTimer = null;
function notice(text) {
  const n = $("notice");
  n.textContent = text;
  n.hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { n.hidden = true; }, 6000);
}
const presetIndex = () => PRESETS.indexOf(state.preset);

// ---- settings ----

function choosePreset(id) {
  state.preset = id;
  state.custom = false;
  state.options = structuredClone(state.presets[id]);
  keepFocus(() => {
    renderSettings();
    renderReport();
  });
  persist();
}

function renderSettings() {
  rich($("preset-note"), state.custom
    ? t("presets.custom", { preset: presetName(state.preset) })
    : t(`presetNote.${state.preset}`));

  const o = state.options;
  const img = o.images || state.presets.balanced.images;
  $("images-on").checked = !!o.images;
  $("dpi").value = Math.round(img.maxDpi);
  $("quality").value = img.jpegQuality;
  $("quality-out").textContent = img.jpegQuality;
  for (const id of ["dpi", "quality"]) $(id).closest("label").classList.toggle("disabled", !o.images);
  $("dedupe").checked = o.dedupe;
  $("font-tables").checked = o.stripFontTables !== false;
  $("subset-fonts").checked = o.subsetFonts !== false;
  $("thumbs").checked = o.stripThumbnails;
  $("private").checked = o.stripPrivateData;
  $("metadata").checked = o.stripMetadata;

  $("out-same").classList.toggle("on", !state.outputDir);
  $("out-pick").classList.toggle("on", !!state.outputDir);
  $("out-pick").textContent = state.outputDir ? basename(state.outputDir) : t("save.choose");
  $("out-pick").title = state.outputDir || "";
}

function readAdvanced() {
  const dpi = Math.min(1200, Math.max(36, Number($("dpi").value) || 150));
  state.options = {
    images: $("images-on").checked ? { maxDpi: dpi, jpegQuality: Number($("quality").value) } : null,
    dedupe: $("dedupe").checked,
    stripFontTables: $("font-tables").checked,
    subsetFonts: $("subset-fonts").checked,
    stripThumbnails: $("thumbs").checked,
    stripPrivateData: $("private").checked,
    stripMetadata: $("metadata").checked,
  };
  const wasCustom = state.custom;
  state.custom = JSON.stringify(state.options) !== JSON.stringify(state.presets[state.preset]);
  renderSettings();
  // Findings and pages depend only on whether the settings are custom, not on their values:
  // a slider dragged through 50 values must not rebuild thousands of page rows 50 times.
  if (wasCustom !== state.custom) keepFocus(renderPresets);
  else renderSaveButton();
  persist();
}

function persist() {
  save("settings", { preset: state.preset, custom: state.custom, options: state.options, outputDir: state.outputDir });
}

// ---- files ----

function addFiles(paths) {
  const pdfs = paths.filter((p) => /\.pdf$/i.test(p));
  // Folders and other files are not read; say so rather than doing nothing.
  const skipped = paths.length - pdfs.length;
  if (skipped) notice(t("drop.skipped", { count: skipped }));
  if (!pdfs.length) return;
  // Files dropped while the comparison is open join the list without changing the report behind it.
  const comparing = $("compare").open;
  for (const path of pdfs) {
    let i = state.files.findIndex((f) => f.path === path);
    if (i < 0) {
      const f = { path, status: "waiting", progress: 0, analysis: null, error: null, saved: null,
                  thumbs: new Map(), thumbsWanted: new Set(), removed: false };
      state.files.push(f);
      i = state.files.length - 1;
      state.queue.push(f);
    } else if (["ready", "error"].includes(state.files[i].status) && !state.files[i].saving) {
      // Added again: analysed again, in case it changed or could not be read before.
      const f = state.files[i];
      invoke("forget", { path: f.path }).catch(() => {});
      Object.assign(f, { status: "waiting", progress: 0, analysis: null, error: null, saved: null,
                         thumbs: new Map(), thumbsWanted: new Set(), renders: new Map(), thumbsBusy: false });
      state.queue.push(f);
    }
    if (!comparing && (state.current < 0 || pdfs.length === 1)) select(i);
  }
  renderFiles();
  renderReport();
  pump();
}

async function pump() {
  if (state.busy) return;
  state.busy = true;
  while (state.queue.length) {
    const f = state.queue.shift();
    if (f.removed) continue;
    f.status = "analysing";
    renderFiles();
    if (isCurrent(f)) renderReport();
    const progress = new Channel();
    progress.onmessage = (done) => { f.progress = done; showProgress(f); };
    f.ticket = newTicket();
    try {
      f.analysis = await invoke("analyze", { path: f.path, ticket: f.ticket, progress });
      f.status = "ready";
    } catch (e) {
      f.error = e;
      f.status = "error";
    }
    f.ticket = null;
    // A file removed while it was being analysed is simply forgotten.
    if (f.removed) continue;
    renderFiles();
    if (isCurrent(f)) renderReport();
  }
  state.busy = false;
}

const isCurrent = (f) => state.files[state.current] === f;

// Updates the progress shown for a file without rebuilding the page.
function showProgress(f) {
  const chip = document.querySelector(`.file[data-index="${state.files.indexOf(f)}"] .file-sub`);
  if (chip) chip.textContent = t("files.analysingPercent", { percent: formatPct(100 * f.progress) });
  if (isCurrent(f)) {
    const bar = document.querySelector("#status .progress span");
    if (bar) bar.style.width = `${(100 * f.progress).toFixed(1)}%`;
    const label = document.querySelector("#status .progress-label");
    if (label) label.textContent = formatPct(100 * f.progress);
  }
}

// Takes a file off the list. The file itself is not touched.
function removeFile(f) {
  const i = state.files.indexOf(f);
  if (i < 0) return;
  f.removed = true;
  // An analysis in progress stops, so the next file's begins at once.
  cancelWork(f.ticket);
  invoke("forget", { path: f.path }).catch(() => {});
  state.files.splice(i, 1);
  state.queue = state.queue.filter((q) => q !== f);
  if (state.current > i) state.current -= 1;
  else if (state.current === i) {
    state.current = Math.min(i, state.files.length - 1);
    state.open.clear();
    state.shown = PAGE_BATCH;
  }
  renderFiles();
  renderReport();
}

function select(i) {
  state.current = i;
  state.open.clear();
  state.shown = PAGE_BATCH;
  keepFocus(() => {
    renderFiles();
    renderReport();
  });
  window.scrollTo({ top: 0 });
}

function renderFiles() {
  const nav = $("files");
  nav.hidden = state.files.length === 0;
  $("drop").classList.toggle("compact", state.files.length > 0);
  nav.replaceChildren(...state.files.map((f, i) => {
    const chip = el("div", `file${i === state.current ? " on" : ""}`);
    chip.dataset.index = i;
    const b = el("button", "file-pick");
    b.type = "button";
    b.title = f.path;
    b.dataset.focus = `file:${f.path}`;
    if (i === state.current) b.setAttribute("aria-current", "true");
    b.append(el("span", "file-name", basename(f.path)));
    let sub = "";
    if (f.status === "waiting") sub = t("files.waiting");
    else if (f.status === "analysing") sub = t("files.analysingPercent", { percent: formatPct(100 * f.progress) });
    else if (f.status === "error") sub = t("files.cantRead");
    else if (f.saved?.output) sub = t("files.saved", { percent: formatPct(pct(f.saved.report.inputBytes - f.saved.report.outputBytes, f.saved.report.inputBytes)) });
    else if (f.analysis.presets.every((p) => p.error)) sub = t("files.failed");
    else {
      const best = Math.max(...f.analysis.presets.filter((p) => !p.error).map((p) => f.analysis.file.bytes - p.outputBytes));
      sub = best > 0 ? t("files.upTo", { percent: formatPct(pct(best, f.analysis.file.bytes)) }) : t("files.alreadyLean");
    }
    const failed = f.status === "error" || (f.status === "ready" && !f.saved?.output && f.analysis.presets.every((p) => p.error));
    b.append(el("span", `file-sub${failed ? " error" : ""}`, sub));
    b.onclick = () => select(i);
    const x = el("button", "file-remove", "×");
    x.type = "button";
    x.title = t("files.remove", { name: basename(f.path) });
    x.setAttribute("aria-label", x.title);
    // A file being saved stays until its copy is written.
    x.disabled = !!f.saving;
    x.onclick = () => removeFile(f);
    chip.append(b, x);
    return chip;
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
        status.append(el("strong", "", basename(f.path)), el("br"), el("span", "error", errorText(f.error)));
      } else {
        const head = el("div", "status-head");
        const waiting = f.status === "waiting";
        const stop = el("button", "link", t("action.cancel"));
        stop.type = "button";
        stop.onclick = () => removeFile(f);
        head.append(el("span", "spinner"), el("span", "", t(waiting ? "status.waiting" : "status.analysing", { name: basename(f.path) })),
          el("span", "progress-label", waiting ? "" : formatPct(100 * f.progress)), stop);
        const bar = el("div", "progress");
        const fill = el("span");
        fill.style.width = `${(100 * f.progress).toFixed(1)}%`;
        bar.append(fill);
        status.append(head, bar);
      }
    }
    return;
  }
  status.hidden = true;
  $("report").hidden = false;
  const a = f.analysis;

  $("file-name").textContent = basename(f.path);
  $("file-name").title = f.path;
  const facts = [t("summary.pages", { count: a.file.pages }), t("summary.version", { version: a.file.version })];
  if (a.file.producer || a.file.creator) facts.push(a.file.producer || a.file.creator);
  if (a.file.revisions > 1) facts.push(t("summary.revisions", { count: a.file.revisions }));
  $("file-facts").textContent = facts.join(" · ");
  $("file-bytes").textContent = formatBytes(a.file.bytes);

  renderWarnings(a);
  renderUsage(a);
  renderPresets();
  renderSaveResult(f);
  renderSaveButton();
}

function renderWarnings(a) {
  const box = $("warnings");
  const warnings = a.findings.filter((x) => x.status === "warning");
  box.replaceChildren(...warnings.map((w) => {
    const d = el("div", "warning");
    d.append(richEl("strong", "", say(w.title)), richEl("span", "", ` ${w.detail.map(say).join(" ")}`));
    return d;
  }));
  const unreadable = a.file.unreadablePages;
  if (unreadable.length) {
    const d = el("div", "warning");
    d.append(el("strong", "", t("warning.unreadable.title")),
      el("span", "", ` ${t("warning.unreadable.detail", { count: unreadable.length, pages: [
        ...unreadable.slice(0, 10).map(String),
        ...(unreadable.length > 10 ? [t("found.morePages", { count: unreadable.length - 10 })] : []),
      ] })}`));
    box.append(d);
  }
}

function renderUsage(a) {
  const total = a.file.bytes;
  // Slivers under 1% are folded into one "Other" entry so the bar stays readable.
  const main = a.categories.filter((c) => pct(c.bytes, total) >= 1);
  const rest = a.categories.filter((c) => pct(c.bytes, total) < 1);
  const rows = main.map((c) => ({ key: c.category, label: t(`category.${c.category}`), bytes: c.bytes }));
  if (rest.length) {
    rows.push({
      key: "other",
      label: t("usage.other"),
      bytes: rest.reduce((s, c) => s + c.bytes, 0),
      detail: rest.map((c) => `${t(`category.${c.category}`)} ${formatBytes(c.bytes)}`).join(", "),
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
    b.dataset.focus = `preset:${p.preset}`;
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(p.preset === state.preset));
    b.append(el("span", "p-name", presetName(p.preset)));
    if (p.error) {
      b.append(el("span", "p-size error", t("presets.failed")));
    } else if (saved <= 0) {
      b.append(el("span", "p-size", formatBytes(total)), el("span", "p-save none", t("presets.noSaving")));
    } else {
      b.append(el("span", "p-size", formatBytes(p.outputBytes)), el("span", "p-save", `−${formatPct(pct(saved, total))}`));
    }
    const meter = el("span", "p-meter");
    const fill = el("span");
    // A failed preset has no size to show.
    fill.style.width = p.error ? "0" : `${Math.max(0, 100 - pct(saved, total))}%`;
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
    bd.append(el("span", "bd-lead", t("breakdown.lead", { preset: presetName(p.preset), size: { bytes: total - p.outputBytes } })));
    for (const s of parts) bd.append(el("span", "bd-item", `${t(`saving.${s.kind}`)} ${formatBytes(s.bytes)}`));
  } else if (!p.error) {
    bd.append(el("span", "bd-lead", t("breakdown.none")));
  }
}

function renderSaveResult(f) {
  const box = $("save-result");
  box.replaceChildren();
  box.hidden = !f.saved;
  if (!f.saved) return;
  const { output, report: r, error } = f.saved;
  if (error) {
    box.append(el("span", "error", errorText(error)));
  } else if (!output) {
    box.append(el("span", "", t("save.nothing")));
  } else {
    box.append(el("span", "saving good", `−${formatPct(pct(r.inputBytes - r.outputBytes, r.inputBytes))}`),
      el("span", "", ` ${t("save.done", { before: { bytes: r.inputBytes }, after: { bytes: r.outputBytes } })} `),
      el("strong", "", basename(output)));
    const btn = el("button", "reveal", t(navigator.platform.startsWith("Mac") ? "save.revealMac" : "save.reveal"));
    btn.type = "button";
    btn.onclick = () => revealItemInDir(output).catch(() => {
      // Moved or deleted since it was saved.
      box.replaceChildren(el("span", "error", t("error.fileMissing")));
    });
    box.append(btn);
  }
}

// The save buttons for the current file: saving is per file, so another file's save does not
// lock this one. "Save all" appears once two files are ready.
function renderSaveButton() {
  const f = state.files[state.current];
  const btn = $("save");
  if (f && f.status === "ready") {
    const p = f.analysis.presets[presetIndex()];
    btn.disabled = !!f.saving || !!state.batch || (!state.custom && !!p.error);
    btn.textContent = !f.saving ? t("save.button")
      : f.saveProgress > 0 ? t("save.savingPercent", { percent: formatPct(100 * f.saveProgress) }) : t("save.saving");
    $("save-cancel").hidden = !f.saving || !!state.batch;
  }
  const ready = state.files.filter((x) => x.status === "ready");
  const all = $("save-all");
  all.hidden = ready.length < 2 && !state.batch;
  all.disabled = !!state.batch || ready.some((x) => x.saving);
  all.textContent = state.batch
    ? t("save.allProgress", { current: Math.min(state.batch.done + 1, state.batch.total), total: state.batch.total })
    : t("save.all", { count: ready.length });
}

async function saveCopy() {
  const f = state.files[state.current];
  if (f && f.status === "ready" && !f.saving) await saveFile(f);
}

// Saves one file with the current settings, showing progress; it can be stopped until writing
// begins. A signed file asks first.
async function saveFile(f) {
  if (f.analysis.file.signed) {
    const go = await ask(`${basename(f.path)}\n\n${t("save.signed.message")}`,
      { title: t("save.signed.title"), kind: "warning", okLabel: t("save.signed.ok"), cancelLabel: t("save.signed.cancel") });
    if (!go) return;
  }
  f.saving = true;
  f.saveProgress = 0;
  f.saveTicket = newTicket();
  const refresh = () => { if (isCurrent(f)) renderSaveButton(); };
  renderFiles();
  refresh();
  const progress = new Channel();
  progress.onmessage = (done) => { f.saveProgress = done; refresh(); };
  try {
    f.saved = await invoke("compress", { job: { input: f.path, outputDir: state.outputDir, options: state.options }, ticket: f.saveTicket, progress });
  } catch (e) {
    f.saved = e && e.code === "cancelled" ? null : { error: e };
    // The chosen folder is gone: go back to saving next to the original, so the next try works.
    if (e && e.code === "folderMissing" && state.outputDir) {
      state.outputDir = null;
      renderSettings();
      persist();
    }
  }
  f.saving = false;
  f.saveTicket = null;
  renderFiles();
  if (isCurrent(f)) renderSaveResult(f);
  renderSaveButton();
}

// Saves every analysed file with the current settings, one after another.
async function saveAll() {
  const files = state.files.filter((f) => f.status === "ready" && !f.saving);
  if (!files.length) return;
  state.batch = { done: 0, total: files.length };
  renderSaveButton();
  for (const f of files) {
    if (!f.removed && f.status === "ready") {
      if (state.files[state.current] !== f) select(state.files.indexOf(f));
      await saveFile(f);
    }
    state.batch.done += 1;
    renderSaveButton();
  }
  state.batch = null;
  renderSaveButton();
}

function statusPill(x) {
  const i = presetIndex();
  if (x.status === "fixed") {
    const here = x.savings[i];
    if (here > 0 && !state.custom) return ["good", t("pill.with", { size: { bytes: here }, preset: presetName(state.preset) })];
    const best = x.savings.reduce((b, s, j) => (s > x.savings[b] ? j : b), 0);
    if (x.savings[best] > 0) return ["muted", t("pill.only", { preset: presetName(PRESETS[best]), size: { bytes: x.savings[best] } })];
    return ["good", t("pill.repack")];
  }
  if (x.status === "notYet") return ["warn", t("pill.notYet")];
  // "kept". Warnings never reach here: they are shown as banners above the report.
  return ["muted", t("pill.kept")];
}

function renderFindings(a) {
  const list = a.findings.filter((x) => x.status !== "warning");
  const ul = $("findings");
  if (!list.length) {
    ul.replaceChildren(el("li", "empty", t("found.empty")));
    return;
  }
  ul.replaceChildren(...list.map((x) => {
    const li = el("li", `finding sev-${x.severity}`);
    const head = el("div", "f-head");
    const [tone, text] = statusPill(x);
    head.append(richEl("span", "f-title", say(x.title)), el("span", "f-bytes", formatBytes(x.bytes)), richEl("span", `pill ${tone}`, text));
    li.append(head, richEl("p", "f-detail", x.detail.map(say).join(" ")));
    if (x.pages.length) {
      const pages = el("div", "f-pages");
      pages.append(el("span", "", t("found.pages", { count: x.pages.length })));
      for (const n of x.pages.slice(0, 24)) {
        const b = el("button", "chip", String(n));
        b.type = "button";
        b.onclick = () => jumpToPage(n);
        pages.append(b);
      }
      if (x.pages.length > 24) pages.append(el("span", "", t("found.morePages", { count: x.pages.length - 24 })));
      li.append(pages);
    }
    return li;
  }));
}

const pressed = (id) => $(id).getAttribute("aria-pressed") === "true";

function jumpToPage(n) {
  $("only-issues").setAttribute("aria-pressed", "false");
  $("largest").setAttribute("aria-pressed", "false");
  state.open.add(n);
  const f = state.files[state.current];
  state.shown = Math.max(state.shown, n + 10);
  renderPages(f.analysis);
  document.querySelector(`[data-page="${n}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" });
}

// "Details" below the save button starts closed at every launch. It used to be remembered, so a
// stored "open" from an earlier version is cleared.
try { localStorage.removeItem("moreOpen"); } catch { /* storage unavailable */ }

// Thumbnails are rendered on demand, for rows that scroll into view, in a separate process.
const thumbObserver = new IntersectionObserver((entries) => {
  const f = state.files[state.current];
  if (!f) return;
  for (const e of entries) {
    if (e.isIntersecting) wantThumb(f, Number(e.target.dataset.thumb));
  }
}, { rootMargin: "400px" });

// Each file asks for one batch at a time; pages that scroll into view meanwhile wait for the
// next. Thumbnails of another file cancel the batch in progress (the app keeps only the newest
// request of each kind), and those pages are asked for again when they come back into view.
function wantThumb(f, page) {
  if (f.thumbs.has(page) || f.thumbsWanted.has(page)) return;
  f.thumbsWanted.add(page);
  clearTimeout(f.thumbTimer);
  f.thumbTimer = setTimeout(() => fetchThumbs(f), 120);
}

async function fetchThumbs(f) {
  if (f.thumbsBusy) return;
  const pages = [...f.thumbsWanted].filter((p) => !f.thumbs.has(p)).slice(0, THUMB_BATCH);
  if (!pages.length) return;
  f.thumbsBusy = true;
  for (const p of pages) f.thumbs.set(p, undefined); // requested
  const each = new Channel();
  each.onmessage = ({ page, png }) => {
    f.thumbs.set(page, png ? `data:image/png;base64,${png}` : null);
    f.thumbsWanted.delete(page);
    if (isCurrent(f)) document.querySelectorAll(`[data-thumb="${page}"]`).forEach((slot) => fillThumb(slot, f, page));
  };
  let finished = true;
  try {
    finished = (await invoke("render", { path: f.path, options: null, pages, width: THUMB_WIDTH, kind: "thumbnails", each })) !== false;
  } catch { /* thumbnails are optional */ }
  // Pictures can arrive after the reply (see renderPage): give the rest time before marking any
  // page as one that could not be drawn.
  if (finished) {
    const until = Date.now() + LATE_PICTURE_MS;
    while (pages.some((p) => f.thumbs.get(p) === undefined) && Date.now() < until) await new Promise((r) => setTimeout(r, 100));
  }
  f.thumbsBusy = false;
  for (const p of pages) {
    if (f.thumbs.get(p) !== undefined) continue;
    if (finished) {
      f.thumbs.set(p, null);
      f.thumbsWanted.delete(p);
    } else {
      // Cancelled: not drawn, not failed. Asked for again when the page is next in view.
      f.thumbs.delete(p);
      f.thumbsWanted.delete(p);
    }
  }
  if (!finished && isCurrent(f)) renderPages(f.analysis);
  if (f.thumbsWanted.size) fetchThumbs(f);
}

function fillThumb(slot, f, page) {
  const src = f.thumbs.get(page);
  slot.classList.toggle("none", src === null);
  if (src && !slot.querySelector("img")) {
    const img = el("img");
    img.src = src;
    img.alt = "";
    slot.append(img);
  }
}

function thumbSlot(f, page, cls) {
  // Clicking a thumbnail compares that page, whether or not anything on it changes.
  const slot = el("button", cls);
  slot.type = "button";
  slot.title = t("compare.pageButton");
  slot.setAttribute("aria-label", t("compare.pageButton"));
  slot.onclick = (e) => { e.stopPropagation(); if (f.status === "ready") openCompare(f, page); };
  slot.dataset.thumb = page;
  fillThumb(slot, f, page);
  if (!f.thumbs.has(page)) thumbObserver.observe(slot);
  return slot;
}

function renderPages(a) {
  const legend = $("page-legend");
  legend.replaceChildren(...PARTS.map(([, cls, key]) => {
    const s = el("span", "lg");
    s.append(el("span", `swatch c-${cls}`), t(key));
    return s;
  }));

  const i = presetIndex();
  let pages = a.pages;
  if (pressed("only-issues")) pages = pages.filter((p) => p.issues.length);
  if (pressed("largest")) pages = [...pages].sort((x, y) => y.bytes - x.bytes);
  const max = Math.max(1, ...a.pages.map((p) => p.bytes));
  const images = new Map(a.images.map((im) => [im.obj, im]));
  const fonts = new Map(a.fonts.map((ft) => [ft.obj, ft]));

  const box = $("pages");
  thumbObserver.disconnect();
  const shown = pages.slice(0, state.shown);
  box.replaceChildren(...shown.map((p) => pageRow(p, max, i, images, fonts)));
  if (!pages.length) box.append(el("p", "empty", t("pages.empty")));
  const more = $("more-pages");
  const remaining = pages.length - state.shown;
  more.hidden = remaining <= 0;
  more.textContent = remaining <= PAGE_BATCH ? t("pages.rest", { count: remaining }) : t("pages.more", { count: PAGE_BATCH, remaining });
}

function pageRow(p, max, i, images, fonts) {
  const wrap = el("div", "page");
  wrap.dataset.page = p.number;
  // A div acting as a button, since it holds the thumbnail's own button.
  const row = el("div", "page-row");
  row.setAttribute("role", "button");
  row.tabIndex = 0;
  row.dataset.focus = `page:${p.number}`;
  row.setAttribute("aria-expanded", String(state.open.has(p.number)));
  row.onkeydown = (e) => { if ((e.key === "Enter" || e.key === " ") && e.target === row) { e.preventDefault(); row.onclick(); } };
  const bar = el("span", "p-bar");
  for (const [key, cls, label] of PARTS) {
    if (!p[key]) continue;
    const s = el("span", `seg c-${cls}`);
    s.style.width = `${(100 * p[key]) / max}%`;
    s.title = `${t(label)}: ${formatBytes(p[key])}`;
    bar.append(s);
  }
  const saving = state.custom ? 0 : p.savings[i];
  const f = state.files[state.current];
  row.append(
    el("span", "p-no", String(p.number)),
    thumbSlot(f, p.number, "p-thumb"),
    bar,
    el("span", "p-bytes", formatBytes(p.bytes)),
    el("span", `p-saving${saving > 0 ? " good" : ""}`, saving > 0 ? formatBytes(-saving) : "–"),
    el("span", `p-issues${p.issues.some((x) => x.status === "notYet") ? " warn" : ""}`,
      p.issues.length ? t("pages.issues", { count: p.issues.length }) : ""),
  );
  row.onclick = () => {
    if (state.open.has(p.number)) state.open.delete(p.number); else state.open.add(p.number);
    keepFocus(() => wrap.replaceWith(pageRow(p, max, i, images, fonts)));
  };
  wrap.append(row);
  if (state.open.has(p.number)) wrap.append(pageDetail(p, i, images, fonts));
  return wrap;
}

function pageDetail(p, i, images, fonts) {
  const d = el("div", "page-detail");
  const f = state.files[state.current];
  // The page's thumbnail beside its facts and issues; images and fonts below.
  const top = el("div", "pd-top");
  const info = el("div", "pd-info");
  top.append(thumbSlot(f, p.number, "pd-thumb"), info);
  d.append(top);
  const size = t("page.size", { width: Math.round(p.width / 72 * 25.4), height: Math.round(p.height / 72 * 25.4) });
  const parts = PARTS.filter(([k]) => p[k]).map(([k, , label]) => t("page.part", { part: t(label), size: { bytes: p[k] } }));
  const facts = el("p", "pd-facts", `${size} · ${parts.join(" · ") || t("page.empty")}`);
  // One "Compare pages" button sits above the list (the comparison moves between pages itself),
  // and the thumbnail beside these facts opens this page.
  info.append(facts);

  if (p.issues.length) {
    const ul = el("ul", "pd-issues");
    for (const x of p.issues) {
      const li = el("li", `st-${x.status}`);
      li.append(el("span", "pd-dot"), el("span", "", say(x.message)));
      ul.append(li);
    }
    info.append(ul);
  }

  const imgs = p.imageObjs.map((o) => images.get(o)).filter((im) => im && !im.isMask);
  if (imgs.length) {
    const tbl = el("table", "pd-table");
    const head = el("tr");
    const withPreset = state.custom ? t("table.withCustom") : t("table.with", { preset: presetName(state.preset) });
    for (const h of [t("table.image"), t("table.storedAs"), t("table.size"), t("table.shown"), withPreset]) head.append(richEl("th", "", h));
    tbl.append(head);
    for (const im of imgs) {
      const tr = el("tr");
      const r = im.results[i];
      let after = t("image.unchanged");
      if (im.unsupported) after = t("image.kept", { reason: { term: `imageReason.${im.unsupported}` } });
      // Custom settings are measured only when saving.
      else if (state.custom) after = t("image.custom");
      else if (r) {
        after = r.downsampled
          ? t("image.downsampled", { width: r.width, height: r.height, size: { bytes: r.bytes } })
          : t("image.reencoded", { size: { bytes: r.bytes } });
      }
      const shown = [im.ppi ? t("image.ppi", { ppi: Math.round(im.ppi) }) : t("image.notDrawn")];
      if (im.visible !== null && im.visible < 0.995) shown.push(t("image.visible", { percent: Math.round(im.visible * 100) }));
      if (im.pages.length > 1) shown.push(t("image.onPages", { count: im.pages.length }));
      const label = { width: im.width, height: im.height, colour: { term: `colour.${im.colour}` } };
      const afterCell = el("td", r && !state.custom ? "good" : "muted", after);
      tr.append(
        el("td", "", t(im.hasSoftMask ? "image.labelMask" : "image.label", label)),
        el("td", "", im.encoding.replace(/Decode/g, "")),
        el("td", "num", formatBytes(im.bytes)),
        el("td", "", shown.join(", ")),
        afterCell,
      );
      tbl.append(tr);
    }
    d.append(tbl);
  }

  const fs = p.fontObjs.map((o) => fonts.get(o)).filter(Boolean);
  if (fs.length) {
    const names = fs.map((ft) => (ft.embedded ? (ft.subset ? ft.name : t("font.full", { name: ft.name })) : t("font.notEmbedded", { name: ft.name })));
    d.append(el("p", "pd-fonts", t("page.fonts", { names: [...new Set(names)] })));
  }
  return d;
}

// ---- compare: a page as it is, and as the current settings will save it ----
//
// The smaller copy is made once with the current settings and kept by the app, so saving
// afterwards writes exactly what was compared. Both versions are drawn by the isolated
// renderer at the size needed, and cached per file.

const ZOOMS = [1, 2, 4];

function settingsLabel() {
  return state.custom ? t("compare.custom") : presetName(state.preset);
}

// How long a picture may take to arrive after its request has finished (see renderPage).
const LATE_PICTURE_MS = 15000;

// A page as a PNG data URL, null when it could not be drawn, or "tooLarge" for a file too
// large to draw. Each side keeps only its newest request: turning pages quickly cancels the
// pages left behind. Only drawn pages are remembered, so a failure is tried again next time.
function renderPage(f, compressed, page, width) {
  const key = `${compressed ? JSON.stringify(state.options) : "original"}|${page}|${width}`;
  f.renders = f.renders || new Map();
  if (!f.renders.has(key)) {
    const drawing = new Promise((resolve) => {
      const each = new Channel();
      each.onmessage = ({ png }) => resolve(png ? `data:image/png;base64,${png}` : null);
      invoke("render", { path: f.path, options: compressed ? state.options : null, pages: [page], width, kind: compressed ? "compare-after" : "compare-before", each })
        .then(
          // A finished request has sent its page, but Tauri delivers a message over 8 KB with a
          // second request, so the picture can arrive after this reply: wait for it (a page that
          // could not be drawn arrives too, as empty). Only a cancelled request sends nothing.
          (finished) => (finished === false ? resolve(null) : setTimeout(() => resolve(null), LATE_PICTURE_MS)),
          (e) => resolve(e && e.code === "tooLargeToShow" ? "tooLarge" : null),
        );
    });
    f.renders.set(key, drawing);
    drawing.then((src) => { if (!src || src === "tooLarge") f.renders.delete(key); });
    // Only the most recent pages are kept: each is a full-size picture.
    while (f.renders.size > 24) f.renders.delete(f.renders.keys().next().value);
  }
  return f.renders.get(key);
}

async function openCompare(f, page) {
  const dlg = $("compare");
  // Each opening has its own number, so work still finishing for an earlier opening stops.
  const session = (dlg.session = (dlg.session || 0) + 1);
  const current = () => dlg.open && dlg.session === session;
  let resizeWatch = null;
  const view = { page, zoom: 1, mode: "swipe", split: 50, diff: false, drawnFor: 0, aspect: 0 };
  const head = el("div", "cmp-head");
  const title = el("h3");
  const nav = el("div", "cmp-nav");
  const prev = el("button", "cmp-step", "‹");
  const next = el("button", "cmp-step", "›");
  for (const [b, key, d] of [[prev, "compare.prev", -1], [next, "compare.next", 1]]) {
    b.type = "button";
    b.dataset.focus = `compare:${key}`;
    b.setAttribute("aria-label", t(key));
    b.title = t(key);
    // Each page is drawn at its own proportions, not those of the page before.
    b.onclick = () => step(d);
    // Enabled once the page can be drawn.
    b.disabled = true;
  }
  const close = el("button", "cmp-close", "×");
  close.type = "button";
  close.setAttribute("aria-label", t("compare.close"));
  close.onclick = () => dlg.close();
  let prepareTicket = null;
  dlg.onkeydown = null;
  dlg.onclose = () => {
    // Closing stops the smaller copy still being made.
    cancelWork(prepareTicket);
    resizeWatch?.disconnect();
    document.documentElement.classList.remove("modal-open");
  };
  nav.append(close);
  head.append(title, nav);
  const body = el("div", "cmp-body");
  dlg.replaceChildren(head, body);
  if (!dlg.open) dlg.showModal();
  // Focus starts on the window, not the close button, which would show its focus ring as soon as
  // it opens. Escape still closes it, and Tab moves to the buttons.
  dlg.tabIndex = -1;
  dlg.focus();
  // The page behind does not scroll (or show its scroll bar) while the window is open.
  document.documentElement.classList.add("modal-open");

  // Make (or reuse) the smaller copy with the current settings.
  title.textContent = t("compare.preparing");
  const bar = el("div", "progress");
  const fill = el("span");
  bar.append(fill);
  body.replaceChildren(bar);
  const progress = new Channel();
  progress.onmessage = (done) => { fill.style.width = `${(100 * done).toFixed(1)}%`; };
  let report;
  try {
    prepareTicket = newTicket();
    report = await invoke("prepare", { path: f.path, options: state.options, ticket: prepareTicket, progress });
    prepareTicket = null;
  } catch (e) {
    prepareTicket = null;
    if (!current()) return;
    rich(title, t("compare.title", { settings: settingsLabel() }));
    body.replaceChildren(el("p", "error", errorText(e)));
    return;
  }
  if (!current()) return;

  const a = f.analysis;
  const total = a.pages.length;
  const toolbar = el("div", "cmp-tools");
  const scroller = el("div", "cmp-stage");
  const captions = el("div", "cmp-captions");
  const note = el("p", "hint");
  // Moving between pages: "‹ Page 3 of 12 ›" centred under the page, and the arrow keys.
  // "Only pages with issues" is the page list's own switch: the arrows then skip clean pages, and
  // the list behind follows.
  const pager = el("div", "cmp-pager");
  const pageOf = el("span", "cmp-page-of");
  const onlyIssues = el("button", "icon-btn");
  onlyIssues.type = "button";
  onlyIssues.dataset.focus = "compare:onlyIssues";
  onlyIssues.setAttribute("aria-label", t("pages.onlyIssues"));
  onlyIssues.dataset.tip = t("pages.onlyIssues");
  onlyIssues.append(icon("issues"));
  onlyIssues.onclick = () => {
    $("only-issues").setAttribute("aria-pressed", String(!pressed("only-issues")));
    state.shown = PAGE_BATCH;
    renderPages(a);
    draw();
  };
  pager.append(onlyIssues, el("span", "tool-sep"), prev, pageOf, next);
  const pagerRow = el("div", "cmp-pager-row");
  pagerRow.append(pager);
  body.replaceChildren(toolbar, scroller, pagerRow, captions, note);
  // The next page in direction d (1 or -1) that the arrows may show, or 0 when there is none.
  const target = (d) => {
    const only = pressed("only-issues");
    for (let n = view.page + d; n >= 1 && n <= total; n += d) if (!only || a.pages[n - 1].issues.length) return n;
    return 0;
  };
  function step(d) {
    const n = target(d);
    if (!n) return;
    view.page = n;
    view.aspect = 0;
    draw();
  }
  dlg.onkeydown = (e) => {
    // The swipe divider and other inputs use the arrow keys themselves.
    if (e.target.closest("input, select, textarea") || e.metaKey || e.ctrlKey || e.altKey) return;
    const b = e.key === "ArrowLeft" ? prev : e.key === "ArrowRight" ? next : null;
    if (b && !b.disabled) { e.preventDefault(); b.click(); }
  };

  const segmented = (items, current, set) => {
    const g = el("div", "segmented");
    for (const [value, label, iconName] of items) {
      const b = el("button", value === current ? "on" : "", label);
      if (iconName) b.prepend(icon(iconName));
      b.type = "button";
      b.dataset.focus = `segment:${items.map(([v]) => v).join("/")}:${value}`;
      b.setAttribute("aria-pressed", String(value === current));
      b.onclick = () => { set(value); draw(); };
      g.append(b);
    }
    return g;
  };

  async function draw() {
    const p = a.pages[view.page - 1];
    rich(title, t("compare.title", { settings: settingsLabel() }));
    onlyIssues.setAttribute("aria-pressed", String(pressed("only-issues")));
    prev.disabled = !target(-1);
    next.disabled = !target(1);

    const diffToggle = el("label", "cmp-check");
    const box = el("input");
    box.type = "checkbox";
    box.dataset.focus = "compare:diff";
    box.checked = view.diff;
    box.onchange = () => { view.diff = box.checked; draw(); };
    diffToggle.append(box, ` ${t("compare.diff")}`);
    keepFocus(() => toolbar.replaceChildren(
      segmented([["swipe", t("compare.swipe")], ["side", t("compare.side")]], view.mode, (v) => { view.mode = v; }),
      // A new magnification centres the view and puts the divider in the middle of the screen.
      segmented(ZOOMS.map((z) => [z, z === 1 ? t("compare.zoomFit") : `${z}×`, z === 1 ? "fit" : "zoomIn"]), view.zoom, (v) => {
        view.zoom = v;
        view.split = 50;
        view.recentre = true;
      }),
      diffToggle,
    ));
    pageOf.textContent = t("compare.pageOf", { page: view.page, total });

    const lean = report.keptOriginal;
    const pageSaving = state.custom ? 0 : p.savings[presetIndex()];
    captions.replaceChildren(
      el("span", "", lean ? t("compare.lean") : t("compare.summaryFile", { settings: settingsLabel(), before: { bytes: report.inputBytes }, after: { bytes: report.outputBytes } })),
      el("span", "good", pageSaving > 0 ? t("compare.summaryPage", { size: { bytes: pageSaving } }) : ""),
    );
    note.textContent = view.diff ? "" : t(view.mode === "side" ? "compare.hintSide" : "compare.hint");

    const waiting = el("p", "cmp-loading");
    waiting.append(el("span", "spinner"), ` ${t("compare.rendering")}`);
    scroller.replaceChildren(waiting);
    scroller.className = `cmp-stage ${view.diff ? "diff" : view.mode}`;
    view.recentreDivider = null;
    view.placeKnob = null;
    scroller.style.setProperty("--zoom", view.zoom);
    // Render at the size the page will be shown, so it is sharp but no larger than needed.
    view.aspect = view.aspect || (p.width > 0 && p.height > 0 ? p.width / p.height : 0.75);
    view.drawnFor = fitWidth();
    const width = Math.min(4096, Math.round(view.drawnFor * (window.devicePixelRatio || 1) * view.zoom));
    const shownPage = view.page;
    const [before, after] = await Promise.all([renderPage(f, false, shownPage, width), renderPage(f, true, shownPage, width)]);
    if (!current() || view.page !== shownPage) return;
    if (before === "tooLarge" || after === "tooLarge") {
      scroller.replaceChildren(el("p", "hint", t("error.tooLargeToShow")));
      return;
    }
    if (!before || !after) {
      scroller.replaceChildren(el("p", "error", t("compare.renderFailed")));
      return;
    }

    const image = (src, cls, alt) => {
      const i = el("img", cls);
      i.src = src;
      i.alt = alt;
      // The rendered page's own proportions win over the page box (rotated pages, for one).
      i.onload = () => {
        const aspect = i.naturalWidth / i.naturalHeight;
        if (cls === "cmp-base" && aspect > 0 && Math.abs(aspect - view.aspect) > 0.01) { view.aspect = aspect; layout(); }
      };
      return i;
    };
    const frame = () => el("div", "cmp-frame");

    if (view.diff) {
      const fr = frame();
      const base = image(before, "cmp-base", t("compare.original"));
      fr.prepend(base);
      scroller.replaceChildren(fr);
      const result = await differences(before, after);
      if (!current() || view.page !== shownPage || !view.diff) return;
      if (result.blank) {
        note.textContent = t("compare.diffFailed");
        return;
      }
      const overlay = el("canvas", "cmp-overlay");
      overlay.width = result.maskWidth;
      overlay.height = result.maskHeight;
      overlay.getContext("2d").putImageData(result.mask, 0, 0);
      fr.insertBefore(overlay, fr.children[1] || null);
      // Measured on the render at this zoom: zooming in shows finer detail, and finer differences.
      const share = (100 * result.changed) / (result.width * result.height);
      note.textContent = result.changed === 0
        ? t("compare.diffNone")
        : t("compare.diffSome", { percent: share < 0.1 ? t("compare.under", { percent: formatPct(0.1) }) : formatPct(share) });
    } else if (view.mode === "swipe") {
      const fr = frame();
      const top = image(after, "cmp-after", settingsLabel());
      top.style.clipPath = `inset(0 0 0 ${view.split}%)`;
      const line = el("div", "cmp-line");
      line.style.left = `${view.split}%`;
      // A round handle on the divider, kept at the vertical middle of what is visible.
      const knob = el("div", "cmp-knob");
      knob.append(icon("drag"));
      knob.style.left = `${view.split}%`;
      const slider = el("input", "cmp-slider");
      Object.assign(slider, { type: "range", min: 0, max: 100, value: view.split });
      slider.setAttribute("aria-label", t("compare.hint"));
      const setSplit = (v) => {
        view.split = Math.max(0, Math.min(100, v));
        top.style.clipPath = `inset(0 0 0 ${view.split}%)`;
        line.style.left = `${view.split}%`;
        knob.style.left = `${view.split}%`;
        slider.value = view.split;
      };
      view.placeKnob = () => {
        // The middle of the part of the page in view: the page's own middle when all of it shows.
        const top = Math.max(scroller.scrollTop, fr.offsetTop);
        const bottom = Math.min(scroller.scrollTop + scroller.clientHeight, fr.offsetTop + fr.clientHeight);
        const middle = (top + bottom) / 2 - fr.offsetTop;
        knob.style.top = `${Math.max(20, Math.min(fr.clientHeight - 20, middle))}px`;
      };
      slider.oninput = () => setSplit(Number(slider.value));
      view.recentreDivider = () => {
        // When scrolling takes the divider out of sight, bring it to the middle of the view.
        const left = fr.offsetLeft + (fr.clientWidth * view.split) / 100;
        const margin = scroller.clientWidth * 0.02;
        if (left < scroller.scrollLeft + margin || left > scroller.scrollLeft + scroller.clientWidth - margin) {
          setSplit((100 * (scroller.scrollLeft + scroller.clientWidth / 2 - fr.offsetLeft)) / fr.clientWidth);
        }
      };
      fr.prepend(image(before, "cmp-base", t("compare.original")), top);
      fr.append(line, knob, slider);
      // Which side is which, pinned to the visible corners while zoomed and scrolled.
      const tags = el("div", "cmp-tags");
      tags.append(el("span", "cmp-tag", t("compare.original")), el("span", "cmp-tag", settingsLabel()));
      scroller.replaceChildren(tags, fr);
    } else {
      const side = (src, label) => {
        const fig = el("figure");
        const fr = frame();
        fr.prepend(image(src, "cmp-base", label));
        fig.append(fr, el("figcaption", "", label));
        return fig;
      };
      scroller.replaceChildren(side(before, t("compare.original")), side(after, settingsLabel()));
    }
    layout();
    requestAnimationFrame(() => view.placeKnob?.());
    if (view.recentre) {
      view.recentre = false;
      requestAnimationFrame(() => {
        scroller.scrollLeft = (scroller.scrollWidth - scroller.clientWidth) / 2;
        scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) / 2;
      });
    }
  }

  // "Fit" shows the whole page in the space available; 2× and 4× scale from there.
  function columns() {
    return view.mode === "side" && !view.diff ? 2 : 1;
  }
  function fitWidth() {
    const cols = columns();
    const pad = 16;
    const availW = ((scroller.clientWidth || 1000) - pad - (cols - 1) * 8) / cols;
    const availH = (scroller.clientHeight || 600) - pad - (cols === 2 ? 24 : 0);
    return Math.max(80, Math.min(availW, availH * (view.aspect || 0.75)));
  }
  function layout() {
    const w = `${Math.round(fitWidth() * view.zoom)}px`;
    scroller.querySelectorAll(".cmp-frame").forEach((fr) => {
      fr.style.width = w;
      if (fr.parentElement.tagName === "FIGURE") fr.parentElement.style.width = w;
    });
  }

  // The window can be resized. The page is laid out again at once; when it grows by more than
  // a quarter, it is also drawn again at the new size so it stays sharp.
  let resizeTimer = null;
  let scrollFrame = 0;
  scroller.addEventListener("scroll", () => {
    cancelAnimationFrame(scrollFrame);
    scrollFrame = requestAnimationFrame(() => { view.recentreDivider?.(); view.placeKnob?.(); });
  }, { passive: true });

  resizeWatch = new ResizeObserver(() => {
    layout();
    view.placeKnob?.();
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (current() && view.drawnFor && fitWidth() > view.drawnFor * 1.25) draw();
    }, 250);
  });
  resizeWatch.observe(scroller);
  draw();
}

// Marks pixels that differ visibly between two renders of the same page. Works through the
// page in strips with two small canvases, freed at the end, and returns a mask at most 2048
// pixels across: WebKit caps the memory all canvases may use together, and page-sized canvases
// at 4× ran into it and came back blank. A blank render is reported, never compared.
const DIFF_VISIBLE = 32; // of 255, in any channel; less is rounding and resampling noise
async function differences(beforeSrc, afterSrc) {
  const load = (src) => new Promise((resolve, reject) => { const i = new Image(); i.onload = () => resolve(i); i.onerror = reject; i.src = src; });
  const [b, a] = await Promise.all([load(beforeSrc), load(afterSrc)]);
  const width = Math.min(b.naturalWidth, a.naturalWidth);
  const height = Math.min(b.naturalHeight, a.naturalHeight);
  const scale = Math.max(1, Math.ceil(Math.max(width, height) / 2048));
  const maskWidth = Math.ceil(width / scale);
  const maskHeight = Math.ceil(height / scale);
  const mask = new ImageData(maskWidth, maskHeight);
  const rows = Math.max(scale, Math.floor(1_000_000 / width / scale) * scale);
  const canvas = () => {
    const c = document.createElement("canvas");
    c.width = width;
    c.height = rows;
    return [c, c.getContext("2d", { willReadFrequently: true })];
  };
  const [cb, xb] = canvas();
  const [ca, xa] = canvas();
  let changed = 0;
  let blank = false;
  try {
    for (let y0 = 0; y0 < height; y0 += rows) {
      // Let the window breathe between strips: a 4× page is tens of millions of pixels.
      if (y0) await new Promise((r) => setTimeout(r, 0));
      const h = Math.min(rows, height - y0);
      xb.clearRect(0, 0, width, rows);
      xa.clearRect(0, 0, width, rows);
      xb.drawImage(b, 0, y0, width, h, 0, 0, width, h);
      xa.drawImage(a, 0, y0, width, h, 0, 0, width, h);
      const pb = xb.getImageData(0, 0, width, h).data;
      const pa = xa.getImageData(0, 0, width, h).data;
      // Pages are drawn on opaque white, so a transparent pixel means the canvas failed.
      if (pb[3] === 0 || pa[3] === 0) { blank = true; break; }
      for (let y = 0; y < h; y++) {
        const m = Math.floor((y0 + y) / scale) * maskWidth;
        for (let x = 0; x < width; x++) {
          const i = (y * width + x) * 4;
          const d = Math.max(Math.abs(pb[i] - pa[i]), Math.abs(pb[i + 1] - pa[i + 1]), Math.abs(pb[i + 2] - pa[i + 2]));
          if (d < DIFF_VISIBLE) continue;
          changed++;
          // Stronger differences are marked more strongly.
          const j = (m + Math.floor(x / scale)) * 4;
          const alpha = Math.min(230, 70 + (d - DIFF_VISIBLE) * 2);
          if (alpha > mask.data[j + 3]) {
            mask.data[j] = 230; mask.data[j + 1] = 40; mask.data[j + 2] = 90; mask.data[j + 3] = alpha;
          }
        }
      }
    }
  } finally {
    for (const c of [cb, ca]) { c.width = 0; c.height = 0; }
  }
  return { width, height, mask, maskWidth, maskHeight, changed, blank };
}

// ---- wiring ----

// A compact language switcher: a globe and a short label, with the full names in its menu.
function renderLanguagePicker() {
  const button = $("language");
  const menu = $("language-menu");
  const label = () => {
    button.replaceChildren(globe(), el("span", "", LANGUAGES[i18n.lang].short));
    button.setAttribute("aria-label", `${t("language.label")}: ${LANGUAGES[i18n.lang].name}`);
    button.title = LANGUAGES[i18n.lang].name;
  };
  const items = () => menu.replaceChildren(...Object.entries(LANGUAGES).map(([code, l]) => {
    const item = el("button", code === i18n.lang ? "on" : "", l.name);
    item.type = "button";
    item.setAttribute("role", "menuitemradio");
    item.setAttribute("aria-checked", String(code === i18n.lang));
    item.lang = code;
    item.onclick = async () => {
      closeMenus();
      await setLanguage(code);
      save("language", code);
      label();
      renderThemePicker.relabel?.();
      renderUpdate.relabel?.();
      renderSettings();
      renderFiles();
      renderReport();
      renderSaveButton();
    };
    return item;
  }));
  label();
  button.onclick = (e) => {
    e.stopPropagation();
    const opening = menu.hidden;
    closeMenus();
    if (opening) {
      items();
      menu.hidden = false;
      button.setAttribute("aria-expanded", "true");
      focusFirst(menu);
    }
  };
  document.addEventListener("click", () => closeMenus());
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenus(true); });
  menuKeys(menu);
}

// Line icons on a 16 × 16 grid, drawn with the current text colour.
const ICONS = {
  globe: ["M8 1.5a6.5 6.5 0 1 0 0 13a6.5 6.5 0 1 0 0-13", "M1.5 8h13", "M8 1.5c2 2 2.6 4.2 2.6 6.5S10 12.5 8 14.5C6 12.5 5.4 10.3 5.4 8S6 3.5 8 1.5"],
  issues: ["M8 2L14.5 13.5H1.5Z", "M8 6.5v3.2", "M8 11.6v.1"],
  largest: ["M2.5 4h11", "M2.5 8h7.5", "M2.5 12h4"],
  expand: ["M4.5 6.5L8 3l3.5 3.5", "M4.5 9.5L8 13l3.5-3.5"],
  collapse: ["M4.5 2.5L8 6l3.5-3.5", "M4.5 13.5L8 10l3.5 3.5"],
  compare: ["M2 3.5h12v9H2z", "M8 2v12", "M4 6.5h2", "M10 9.5h2"],
  drag: ["M6 4.5L2.5 8L6 11.5", "M10 4.5L13.5 8L10 11.5"],
  system: ["M2 3h12v8H2z", "M6 14h4", "M8 11v3"],
  light: ["M8 5.2a2.8 2.8 0 1 0 0 5.6a2.8 2.8 0 1 0 0-5.6", "M8 1.5v1.5", "M8 13v1.5", "M1.5 8H3", "M13 8h1.5", "M3.4 3.4l1 1", "M11.6 11.6l1 1", "M3.4 12.6l1-1", "M11.6 4.4l1-1"],
  dark: ["M13 9.5A5.5 5.5 0 1 1 6.5 3a4.5 4.5 0 0 0 6.5 6.5Z"],
  update: ["M13.2 6A5.5 5.5 0 1 0 13.5 8", "M13.5 2.5V6H10"],
  fit: ["M2 5.5V2h3.5", "M10.5 2H14v3.5", "M14 10.5V14h-3.5", "M5.5 14H2v-3.5"],
  zoomIn: ["M7 2.5a4.5 4.5 0 1 0 0 9a4.5 4.5 0 1 0 0-9", "M10.3 10.3L13.5 13.5", "M7 5v4", "M5 7h4"],
};

// ---- theme: System (the default) follows the operating system; Light and Dark override it ----

const THEMES = ["system", "light", "dark"];

function applyTheme(theme) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  // The window frame (title bar) follows too, where the platform allows it.
  try {
    window.__TAURI__.window?.getCurrentWindow().setTheme(theme === "system" ? null : theme).catch(() => {});
  } catch { /* not available */ }
}

function closeMenus(returnFocus = false) {
  for (const [b, m] of [["update", "update-menu"], ["theme", "theme-menu"], ["language", "language-menu"]]) {
    // Escape gives focus back to the button of the menu it closed.
    if (returnFocus && !$(m).hidden) $(b).focus();
    $(m).hidden = true;
    $(b).setAttribute("aria-expanded", "false");
  }
}

// Arrow keys move between a menu's items; the first item takes focus when it opens.
function menuKeys(menu) {
  menu.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...menu.querySelectorAll("button")];
    if (!items.length) return;
    e.preventDefault();
    const at = items.indexOf(document.activeElement);
    const next = e.key === "ArrowDown" ? (at + 1) % items.length : (at - 1 + items.length) % items.length;
    items[next].focus();
  });
}
const focusFirst = (menu) => requestAnimationFrame(() => menu.querySelector("button")?.focus());

function renderThemePicker() {
  const button = $("theme");
  const menu = $("theme-menu");
  let current = THEMES.includes(load("theme", "system")) ? load("theme", "system") : "system";
  const label = () => {
    button.replaceChildren(icon(current));
    button.setAttribute("aria-label", `${t("theme.label")}: ${t(`theme.${current}`)}`);
    button.title = `${t("theme.label")}: ${t(`theme.${current}`)}`;
  };
  const items = () => menu.replaceChildren(...THEMES.map((theme) => {
    const item = el("button", theme === current ? "on" : "");
    item.type = "button";
    item.setAttribute("role", "menuitemradio");
    item.setAttribute("aria-checked", String(theme === current));
    item.append(icon(theme), el("span", "", t(`theme.${theme}`)));
    item.onclick = () => {
      current = theme;
      save("theme", theme);
      applyTheme(theme);
      closeMenus();
      label();
    };
    return item;
  }));
  applyTheme(current);
  label();
  button.onclick = (e) => {
    e.stopPropagation();
    const opening = menu.hidden;
    closeMenus();
    if (opening) {
      items();
      menu.hidden = false;
      button.setAttribute("aria-expanded", "true");
      focusFirst(menu);
    }
  };
  menuKeys(menu);
  // Relabelled when the language changes.
  renderThemePicker.relabel = label;
}

// ---- update check: only when asked, and only the version number goes back and forth ----

const SITE = "https://yusungchang.com/pdf-epsilon";
const LATEST = "https://api.github.com/repos/yusungchang/pdf-epsilon/releases/latest";

// "v1.2.3" or "1.2.3" to [1, 2, 3]; anything else is not trusted.
function parseVersion(v) {
  const m = /^v?(\d{1,4})\.(\d{1,4})\.(\d{1,4})$/.exec(String(v || "").trim());
  return m ? m.slice(1).map(Number) : null;
}
function newer(a, b) {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}

// The newest published release, or null when GitHub cannot be reached.
async function latestVersion() {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 8000);
  try {
    const r = await fetch(LATEST, { signal: ctl.signal, headers: { Accept: "application/vnd.github+json" }, credentials: "omit", referrerPolicy: "no-referrer" });
    if (!r.ok) return null;
    return parseVersion((await r.json()).tag_name);
  } catch { return null; } finally { clearTimeout(timer); }
}

function renderUpdate() {
  const button = $("update");
  const menu = $("update-menu");
  let mine = null;
  const label = () => {
    button.replaceChildren(icon("update"));
    button.setAttribute("aria-label", t("update.label"));
    button.title = t("update.label");
  };
  const show = (...lines) => menu.replaceChildren(
    el("p", "update-version", t("update.current", { version: mine ? mine.join(".") : "?" })),
    ...lines);
  const siteButton = (primary) => {
    const b = el("button", primary ? "primary" : "link", t("update.download"));
    b.type = "button";
    b.onclick = () => { window.__TAURI__.opener.openUrl(SITE).catch(() => {}); closeMenus(); };
    return b;
  };
  async function check() {
    show(el("p", "update-status", t("update.checking")));
    if (!mine) mine = parseVersion(await window.__TAURI__.app.getVersion().catch(() => ""));
    const latest = await latestVersion();
    if (menu.hidden) return;
    if (!latest || !mine) show(el("p", "update-status", t("update.failed")), siteButton(false));
    else if (newer(latest, mine)) show(el("p", "update-status new", t("update.available", { version: latest.join(".") })), siteButton(true));
    else show(el("p", "update-status", t("update.latest")));
  }
  label();
  button.onclick = (e) => {
    e.stopPropagation();
    const opening = menu.hidden;
    closeMenus();
    if (opening) {
      menu.hidden = false;
      button.setAttribute("aria-expanded", "true");
      check();
    }
  };
  menu.addEventListener("click", (e) => e.stopPropagation());
  renderUpdate.relabel = label;
}

function icon(name, cls = "icon") {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add(cls);
  for (const d of ICONS[name]) {
    const path = document.createElementNS(ns, "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  return svg;
}

const globe = () => icon("globe", "globe");

async function init() {
  await setLanguage(pickLanguage());
  window.__TAURI__.app.getVersion().then((v) => { $("version").textContent = v; }).catch(() => {});
  renderUpdate();
  renderThemePicker();
  renderLanguagePicker();
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

  for (const id of ["images-on", "dpi", "quality", "dedupe", "subset-fonts", "font-tables", "thumbs", "private", "metadata"]) {
    // The resolution is read when it is done (Enter or leaving the field), so typing "300" is
    // not corrected to the minimum after its first digit.
    $(id).addEventListener(id === "dpi" ? "change" : "input", readAdvanced);
  }
  $("out-same").addEventListener("click", () => { state.outputDir = null; renderSettings(); persist(); });
  $("out-pick").addEventListener("click", async () => {
    const dir = await open({ directory: true, multiple: false, title: t("dialog.folder") });
    if (dir) { state.outputDir = dir; renderSettings(); persist(); }
  });
  $("save").addEventListener("click", saveCopy);
  $("save-all").addEventListener("click", saveAll);
  $("save-cancel").addEventListener("click", () => cancelWork(state.files[state.current]?.saveTicket));
  document.querySelectorAll("[data-icon]").forEach((b) => b.prepend(icon(b.dataset.icon)));
  for (const id of ["only-issues", "largest"]) {
    $(id).addEventListener("click", () => {
      $(id).setAttribute("aria-pressed", String(!pressed(id)));
      state.shown = PAGE_BATCH;
      renderPages(state.files[state.current].analysis);
    });
  }
  $("more-pages").addEventListener("click", () => { state.shown += PAGE_BATCH; renderPages(state.files[state.current].analysis); });
  $("expand-all").addEventListener("click", () => {
    const a = state.files[state.current].analysis;
    const pages = pressed("only-issues") ? a.pages.filter((p) => p.issues.length) : a.pages;
    for (const p of pages) state.open.add(p.number);
    renderPages(a);
  });
  // Opens the comparison at page 1; it moves between pages itself.
  $("compare-pages").addEventListener("click", () => {
    const f = state.files[state.current];
    if (f?.status === "ready") openCompare(f, 1);
  });
  $("collapse-all").addEventListener("click", () => { state.open.clear(); renderPages(state.files[state.current].analysis); });

  $("drop").addEventListener("click", async () => {
    const files = await open({ multiple: true, title: t("dialog.choose"), filters: [{ name: t("dialog.pdf"), extensions: ["pdf", "PDF"] }] });
    if (files) addFiles(Array.isArray(files) ? files : [files]);
  });

  // PDFs opened from outside: Open With, the Dock icon, or a second launch on Windows and Linux.
  const takeOpened = async () => {
    const paths = await invoke("take_opened").catch(() => []);
    if (paths.length) addFiles(paths);
  };
  await window.__TAURI__.event.listen("files-opened", takeOpened);
  takeOpened();

  await getCurrentWebview().onDragDropEvent((event) => {
    const { type, paths } = event.payload;
    if (type === "enter" || type === "over") $("drop").classList.add("over");
    else if (type === "leave") $("drop").classList.remove("over");
    else if (type === "drop") { $("drop").classList.remove("over"); addFiles(paths || []); }
  });
}

init().catch((e) => { const s = $("status"); s.hidden = false; s.textContent = t("error.start", { detail: String(e) }); });

// PDF Epsilon interface. Plain script: Tauri's APIs come from window.__TAURI__.
// Every word shown comes from ui/locales/<language>.json through t(); see "Languages" below.
const { invoke } = window.__TAURI__.core;
const { open, ask } = window.__TAURI__.dialog;
const { revealItemInDir } = window.__TAURI__.opener;
const { getCurrentWebview } = window.__TAURI__.webview;

// To add a language, add ui/locales/<code>.json with the same keys as en.json and list it here,
// under its own name for itself.
const LANGUAGES = { en: "English", ko: "한국어" };

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
  document.querySelectorAll("[data-i18n-label]").forEach((e) => { e.setAttribute("aria-label", t(e.dataset.i18nLabel)); });
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
    ? t("presets.custom", { preset: presetName(state.preset) })
    : t(`presetNote.${state.preset}`);

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
      f.error = e;
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
    if (f.status === "waiting") sub = t("files.waiting");
    else if (f.status === "analysing") sub = t("files.analysing");
    else if (f.status === "error") sub = t("files.cantRead");
    else {
      const best = Math.max(...f.analysis.presets.map((p) => f.analysis.file.bytes - p.outputBytes));
      sub = best > 0 ? t("files.upTo", { percent: formatPct(pct(best, f.analysis.file.bytes)) }) : t("files.alreadyLean");
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
        status.append(el("strong", "", basename(f.path)), el("br"), el("span", "error", errorText(f.error)));
      } else {
        status.append(el("span", "spinner"), ` ${t("status.analysing", { name: basename(f.path) })}`);
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
}

function renderWarnings(a) {
  const box = $("warnings");
  const warnings = a.findings.filter((x) => x.status === "warning");
  box.replaceChildren(...warnings.map((w) => {
    const d = el("div", "warning");
    d.append(el("strong", "", say(w.title)), el("span", "", ` ${w.detail.map(say).join(" ")}`));
    return d;
  }));
  const unreadable = a.file.unreadablePages;
  if (unreadable.length) {
    const d = el("div", "warning");
    d.append(el("strong", "", t("warning.unreadable.title")),
      el("span", "", ` ${t("warning.unreadable.detail", { count: unreadable.length, pages: unreadable.slice(0, 10).map(String) })}`));
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
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(p.preset === state.preset));
    b.append(el("span", "p-name", presetName(p.preset)));
    if (p.error) {
      b.append(el("span", "p-size error", t("presets.failed")));
      b.title = p.error;
    } else if (saved <= 0) {
      b.append(el("span", "p-size", formatBytes(total)), el("span", "p-save none", t("presets.noSaving")));
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
    btn.onclick = () => revealItemInDir(output);
    box.append(btn);
  }
}

async function saveCopy() {
  const f = state.files[state.current];
  if (!f || f.status !== "ready") return;
  if (f.analysis.file.signed) {
    const go = await ask(t("save.signed.message"),
      { title: t("save.signed.title"), kind: "warning", okLabel: t("save.signed.ok"), cancelLabel: t("save.signed.cancel") });
    if (!go) return;
  }
  const btn = $("save");
  btn.disabled = true;
  btn.textContent = t("save.saving");
  try {
    f.saved = await invoke("compress", { job: { input: f.path, outputDir: state.outputDir, options: state.options } });
  } catch (e) {
    f.saved = { error: e };
  }
  btn.disabled = false;
  btn.textContent = t("save.button");
  if (state.files[state.current] === f) renderSaveResult(f);
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
  if (x.status === "kept") return ["muted", t("pill.kept")];
  return ["bad", t("pill.warning")];
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
    head.append(el("span", "f-title", say(x.title)), el("span", "f-bytes", formatBytes(x.bytes)), el("span", `pill ${tone}`, text));
    li.append(head, el("p", "f-detail", x.detail.map(say).join(" ")));
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
  legend.replaceChildren(...PARTS.map(([, cls, key]) => {
    const s = el("span", "lg");
    s.append(el("span", `swatch c-${cls}`), t(key));
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
  if (!pages.length) box.append(el("p", "empty", t("pages.empty")));
  const more = $("more-pages");
  const remaining = pages.length - state.shown;
  more.hidden = remaining <= 0;
  more.textContent = t("pages.more", { count: Math.min(PAGE_BATCH, remaining), remaining });
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
    s.title = `${t(label)}: ${formatBytes(p[key])}`;
    bar.append(s);
  }
  const saving = state.custom ? 0 : p.savings[i];
  row.append(
    el("span", "p-no", String(p.number)),
    bar,
    el("span", "p-bytes", formatBytes(p.bytes)),
    el("span", `p-saving${saving > 0 ? " good" : ""}`, saving > 0 ? formatBytes(-saving) : "–"),
    el("span", `p-issues${p.issues.some((x) => x.status === "notYet") ? " warn" : ""}`,
      p.issues.length ? t("pages.issues", { count: p.issues.length }) : ""),
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
  const size = t("page.size", { width: Math.round(p.width / 72 * 25.4), height: Math.round(p.height / 72 * 25.4) });
  const parts = PARTS.filter(([k]) => p[k]).map(([k, , label]) => t("page.part", { part: t(label), size: { bytes: p[k] } }));
  d.append(el("p", "pd-facts", `${size} · ${parts.join(" · ") || t("page.empty")}`));

  if (p.issues.length) {
    const ul = el("ul", "pd-issues");
    for (const x of p.issues) {
      const li = el("li", `st-${x.status}`);
      li.append(el("span", "pd-dot"), el("span", "", say(x.message)));
      ul.append(li);
    }
    d.append(ul);
  }

  const imgs = p.imageObjs.map((o) => images.get(o)).filter((im) => im && !im.isMask);
  if (imgs.length) {
    const tbl = el("table", "pd-table");
    const head = el("tr");
    const withPreset = state.custom ? t("table.withCustom") : t("table.with", { preset: presetName(state.preset) });
    for (const h of [t("table.image"), t("table.storedAs"), t("table.size"), t("table.shown"), withPreset]) head.append(el("th", "", h));
    tbl.append(head);
    for (const im of imgs) {
      const tr = el("tr");
      const r = im.results[i];
      let after = t("image.unchanged");
      if (im.unsupported) after = t("image.kept", { reason: { term: `imageReason.${im.unsupported}` } });
      else if (r && !state.custom) {
        after = r.downsampled
          ? t("image.downsampled", { width: r.width, height: r.height, size: { bytes: r.bytes } })
          : t("image.reencoded", { size: { bytes: r.bytes } });
      }
      const shown = [im.ppi ? t("image.ppi", { ppi: Math.round(im.ppi) }) : t("image.notDrawn")];
      if (im.visible !== null && im.visible < 0.995) shown.push(t("image.visible", { percent: Math.round(im.visible * 100) }));
      if (im.pages.length > 1) shown.push(t("image.onPages", { count: im.pages.length }));
      const label = { width: im.width, height: im.height, colour: { term: `colour.${im.colour}` } };
      tr.append(
        el("td", "", t(im.hasSoftMask ? "image.labelMask" : "image.label", label)),
        el("td", "", im.encoding.replace(/Decode/g, "")),
        el("td", "num", formatBytes(im.bytes)),
        el("td", "", shown.join(", ")),
        el("td", r && !state.custom ? "good" : "muted", after),
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

// ---- wiring ----

function renderLanguagePicker() {
  const pick = $("language");
  pick.replaceChildren(...Object.entries(LANGUAGES).map(([code, name]) => {
    const o = el("option", "", name);
    o.value = code;
    return o;
  }));
  pick.value = i18n.lang;
  pick.addEventListener("change", async () => {
    await setLanguage(pick.value);
    save("language", pick.value);
    renderSettings();
    renderFiles();
    renderReport();
  });
}

async function init() {
  await setLanguage(pickLanguage());
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
    $(id).addEventListener("input", readAdvanced);
  }
  $("out-same").addEventListener("click", () => { state.outputDir = null; renderSettings(); persist(); });
  $("out-pick").addEventListener("click", async () => {
    const dir = await open({ directory: true, multiple: false, title: t("dialog.folder") });
    if (dir) { state.outputDir = dir; renderSettings(); persist(); }
  });
  $("save").addEventListener("click", saveCopy);
  for (const id of ["only-issues", "largest"]) {
    $(id).addEventListener("input", () => { state.shown = PAGE_BATCH; renderPages(state.files[state.current].analysis); });
  }
  $("more-pages").addEventListener("click", () => { state.shown += PAGE_BATCH; renderPages(state.files[state.current].analysis); });

  $("drop").addEventListener("click", async () => {
    const files = await open({ multiple: true, title: t("dialog.choose"), filters: [{ name: t("dialog.pdf"), extensions: ["pdf", "PDF"] }] });
    if (files) addFiles(Array.isArray(files) ? files : [files]);
  });

  await getCurrentWebview().onDragDropEvent((event) => {
    const { type, paths } = event.payload;
    if (type === "enter" || type === "over") $("drop").classList.add("over");
    else if (type === "leave") $("drop").classList.remove("over");
    else if (type === "drop") { $("drop").classList.remove("over"); addFiles(paths || []); }
  });
}

init().catch((e) => { const s = $("status"); s.hidden = false; s.textContent = t("error.start", { detail: String(e) }); });

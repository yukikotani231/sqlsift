import { EXAMPLES } from './examples.js';

const DEBOUNCE_MS = 200;
const DIALECTS = ['postgresql', 'mysql', 'sqlite'];

const $ = (sel) => document.querySelector(sel);

// ---------------------------------------------------------------------------
// Editor: a <textarea> with a line-number gutter and a highlight backdrop.
// ---------------------------------------------------------------------------

function escapeHtml(s) {
  return s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
}

class Editor {
  constructor(name) {
    this.name = name;
    this.root = document.querySelector(`.editor[data-editor="${name}"]`);
    this.ta = this.root.querySelector('textarea');
    this.gutter = this.root.querySelector('.gutter-inner');
    this.hl = this.root.querySelector('.highlights');
    this.diags = [];
    this.active = -1;
    this.lineCount = 0;

    this.ta.addEventListener('scroll', () => this.syncScroll(), { passive: true });
    this.ta.addEventListener('input', () => {
      // Positions of existing diagnostics are stale until the next analysis.
      this.diags = [];
      this.active = -1;
      this.render();
    });
  }

  get value() {
    return this.ta.value;
  }

  setValue(text) {
    this.ta.value = text;
    this.diags = [];
    this.active = -1;
    this.ta.scrollTop = 0;
    this.ta.scrollLeft = 0;
    this.render();
  }

  setDiagnostics(diags) {
    this.diags = diags;
    this.active = -1;
    this.render();
  }

  lineStarts() {
    const text = this.ta.value;
    const starts = [0];
    for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) starts.push(i + 1);
    return starts;
  }

  // 1-indexed line / character column -> UTF-16 offset into the textarea value.
  offsetOf(starts, line, column) {
    const text = this.ta.value;
    if (line < 1) return 0;
    if (line > starts.length) return text.length;
    const lineStart = starts[line - 1];
    const lineEnd = line < starts.length ? starts[line] - 1 : text.length;
    let off = lineStart;
    let cp = 1;
    while (off < lineEnd && cp < column) {
      const code = text.charCodeAt(off);
      off += code >= 0xd800 && code <= 0xdbff ? 2 : 1;
      cp++;
    }
    return Math.min(off, lineEnd);
  }

  rangeOf(diag, starts = this.lineStarts()) {
    if (!diag.line) return null;
    const start = this.offsetOf(starts, diag.line, diag.column || 1);
    let end = this.offsetOf(starts, diag.end_line || diag.line, diag.end_column || (diag.column || 1) + 1);
    if (end <= start) end = Math.min(start + 1, this.ta.value.length);
    return { start, end };
  }

  render() {
    const text = this.ta.value;
    const starts = this.lineStarts();

    // Gutter
    const lineSeverity = new Map();
    for (const d of this.diags) {
      if (!d.line) continue;
      const prev = lineSeverity.get(d.line);
      if (prev !== 'error') lineSeverity.set(d.line, d.severity);
    }
    let gutter = '';
    for (let i = 1; i <= starts.length; i++) {
      const sev = lineSeverity.get(i);
      gutter += sev ? `<div class="has-${sev}">${i}</div>` : `<div>${i}</div>`;
    }
    this.gutter.innerHTML = gutter;
    this.lineCount = starts.length;

    // Highlights (non-overlapping, in document order)
    const ranges = [];
    this.diags.forEach((d, i) => {
      const r = this.rangeOf(d, starts);
      if (r) ranges.push({ ...r, i, severity: d.severity });
    });
    ranges.sort((a, b) => a.start - b.start || b.end - a.end);
    let html = '';
    let pos = 0;
    for (const r of ranges) {
      const start = Math.max(r.start, pos);
      if (start >= r.end) continue;
      html += escapeHtml(text.slice(pos, start));
      const cls = `${r.severity}${r.i === this.active ? ' active' : ''}`;
      html += `<mark class="${cls}" data-i="${r.i}">${escapeHtml(text.slice(start, r.end))}</mark>`;
      pos = r.end;
    }
    html += escapeHtml(text.slice(pos));
    // Keep a trailing newline from collapsing so heights match the textarea.
    this.hl.innerHTML = html + '\n ';
    this.syncScroll();
  }

  syncScroll() {
    const { scrollTop, scrollLeft } = this.ta;
    this.hl.style.transform = `translate(${-scrollLeft}px, ${-scrollTop}px)`;
    this.gutter.style.transform = `translateY(${-scrollTop}px)`;
  }

  reveal(index) {
    const diag = this.diags[index];
    this.active = index;
    this.render();
    this.root.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    this.ta.focus({ preventScroll: true });
    const range = diag && this.rangeOf(diag);
    if (!range) return;
    this.ta.setSelectionRange(range.start, range.end);

    const style = getComputedStyle(this.ta);
    const lineHeight = parseFloat(style.lineHeight) || 20;
    const padTop = parseFloat(style.paddingTop) || 0;
    const y = padTop + (diag.line - 1) * lineHeight;
    if (y < this.ta.scrollTop || y + lineHeight > this.ta.scrollTop + this.ta.clientHeight) {
      this.ta.scrollTop = Math.max(0, y - this.ta.clientHeight / 3);
    }
    const mark = this.hl.querySelector(`mark[data-i="${index}"]`);
    if (mark) {
      const x = mark.offsetLeft;
      if (x < this.ta.scrollLeft || x + mark.offsetWidth > this.ta.scrollLeft + this.ta.clientWidth - 28) {
        this.ta.scrollLeft = Math.max(0, x - 40);
      }
    }
    this.syncScroll();
  }
}

// ---------------------------------------------------------------------------
// Analyzer worker
// ---------------------------------------------------------------------------

let worker = null;
let workerReady = false;
let requestId = 0;
let pendingRequest = null;

function startWorker() {
  workerReady = false;
  worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
  worker.onmessage = (event) => {
    const msg = event.data;
    if (msg.type === 'ready') {
      workerReady = true;
      $('#version').textContent = `v${msg.version}`;
      if (pendingRequest) post(pendingRequest);
    } else if (msg.type === 'fatal') {
      setStatus(`Failed to load the analyzer: ${msg.message}`, true);
    } else if (msg.id === requestId) {
      if (msg.type === 'result') showResult(msg.result, msg.ms);
      else if (msg.type === 'crash') {
        showCrash(msg.message);
        worker.terminate();
        startWorker();
      }
    }
  };
  worker.onerror = (event) => {
    setStatus(`Failed to load the analyzer${event.message ? `: ${event.message}` : ''}`, true);
  };
}

function post(req) {
  pendingRequest = req;
  if (workerReady) {
    worker.postMessage(req);
    pendingRequest = null;
  }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

const editors = { schema: new Editor('schema'), query: new Editor('query') };
const dialectEl = $('#dialect');
const exampleEl = $('#example');
const listEl = $('#diag-list');
let currentDiags = [];

function setStatus(text, isError = false) {
  const el = $('#status');
  el.textContent = text;
  el.classList.toggle('is-error', isError);
}

function plural(n, word) {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function analyze() {
  requestId += 1;
  post({
    id: requestId,
    schema: editors.schema.value,
    query: editors.query.value,
    dialect: dialectEl.value,
  });
  if (!workerReady) setStatus('Loading analyzer…');
}

let timer = 0;
function scheduleAnalyze() {
  clearTimeout(timer);
  timer = setTimeout(analyze, DEBOUNCE_MS);
}

const CHECK_ICON =
  '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0m3.53 5.47a.75.75 0 0 0-1.06 0L7 8.94 5.53 7.47a.75.75 0 0 0-1.06 1.06l2 2a.75.75 0 0 0 1.06 0l4-4a.75.75 0 0 0 0-1.06"/></svg>';
const ALERT_ICON =
  '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0m0 3.5a.9.9 0 0 0-.9.95l.2 4.3a.7.7 0 0 0 1.4 0l.2-4.3A.9.9 0 0 0 8 3.5M8 10.5a1 1 0 1 0 0 2 1 1 0 0 0 0-2"/></svg>';

function showResult(result, ms) {
  const diags = result.diagnostics || [];
  currentDiags = diags;

  const bySource = { schema: [], query: [] };
  diags.forEach((d, i) => bySource[d.source]?.push({ ...d, globalIndex: i }));
  editors.schema.setDiagnostics(bySource.schema);
  editors.query.setDiagnostics(bySource.query);

  for (const source of ['schema', 'query']) {
    const badge = $(`#${source}-count`);
    const items = bySource[source];
    const errors = items.filter((d) => d.severity === 'error').length;
    badge.hidden = items.length === 0;
    badge.textContent = errors ? plural(errors, 'error') : plural(items.length, 'warning');
    badge.classList.toggle('is-warning', errors === 0);
  }

  const errors = diags.filter((d) => d.severity === 'error').length;
  const warnings = diags.filter((d) => d.severity === 'warning').length;
  const summary = $('#diag-summary');
  if (diags.length === 0) {
    summary.textContent = 'No problems';
  } else {
    const parts = [];
    if (errors) parts.push(`<span class="n-error">${plural(errors, 'error')}</span>`);
    if (warnings) parts.push(`<span class="n-warning">${plural(warnings, 'warning')}</span>`);
    const infos = diags.length - errors - warnings;
    if (infos) parts.push(plural(infos, 'note'));
    summary.innerHTML = parts.join(', ');
  }
  setStatus(`Checked in ${ms < 1 ? ms.toFixed(2) : ms.toFixed(1)} ms`);

  if (diags.length === 0) {
    const msg = editors.query.value.trim()
      ? 'No problems found. The query is valid against this schema.'
      : 'Write a query to check it against the schema.';
    listEl.innerHTML = `<li class="diag-empty">${CHECK_ICON}<span>${msg}</span></li>`;
    return;
  }

  listEl.innerHTML = diags
    .map((d, i) => {
      const loc = d.line ? `${d.source}:${d.line}:${d.column}` : `${d.source} (no location)`;
      const help = d.help
        ? `<div class="diag-help"><span class="label">= help:</span> ${escapeHtml(d.help)}</div>`
        : '';
      return `<li><button type="button" class="diag ${d.severity}" data-i="${i}">
<div class="diag-title"><span class="diag-sev ${d.severity}">${d.severity}[${d.code}]</span>: ${escapeHtml(d.message)}</div>
<div class="diag-loc"><span class="arrow">--&gt;</span> ${loc}</div>${help}</button></li>`;
    })
    .join('');
}

function showCrash(message) {
  currentDiags = [];
  editors.schema.setDiagnostics([]);
  editors.query.setDiagnostics([]);
  $('#schema-count').hidden = true;
  $('#query-count').hidden = true;
  $('#diag-summary').textContent = '';
  setStatus('Analyzer crashed', true);
  listEl.innerHTML = `<li class="diag-empty crash">${ALERT_ICON}<span>sqlsift hit an internal error on this input (${escapeHtml(
    message,
  )}). Please <a href="https://github.com/yukikotani231/sqlsift/issues/new" rel="noopener">report it</a> with a Share link.</span></li>`;
}

listEl.addEventListener('click', (event) => {
  const btn = event.target.closest('button.diag');
  if (!btn) return;
  const i = Number(btn.dataset.i);
  const diag = currentDiags[i];
  if (!diag) return;
  listEl.querySelectorAll('.diag.active').forEach((el) => el.classList.remove('active'));
  btn.classList.add('active');
  const editor = editors[diag.source];
  const local = editor.diags.findIndex((d) => d.globalIndex === i);
  editor.reveal(local);
});

// ---------------------------------------------------------------------------
// Examples, dialect, sharing
// ---------------------------------------------------------------------------

const CUSTOM = '__custom__';

function populateExamples() {
  const opts = EXAMPLES.map((ex) => `<option value="${ex.id}">${escapeHtml(ex.name)}</option>`);
  opts.push(`<option value="${CUSTOM}" hidden>Custom</option>`);
  exampleEl.innerHTML = opts.join('');
}

function loadExample(id) {
  const ex = EXAMPLES.find((e) => e.id === id) || EXAMPLES[0];
  exampleEl.value = ex.id;
  dialectEl.value = ex.dialect;
  editors.schema.setValue(ex.schema);
  editors.query.setValue(ex.query);
  analyze();
}

function markCustom() {
  exampleEl.value = CUSTOM;
}

function toBase64Url(str) {
  const bytes = new TextEncoder().encode(str);
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function fromBase64Url(b64) {
  const bin = atob(b64.replace(/-/g, '+').replace(/_/g, '/'));
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

function shareUrl() {
  const payload = JSON.stringify({
    v: 1,
    dialect: dialectEl.value,
    schema: editors.schema.value,
    query: editors.query.value,
  });
  const url = new URL(location.href);
  url.hash = `code=${toBase64Url(payload)}`;
  return url.toString();
}

function loadFromHash() {
  const match = location.hash.match(/^#code=([A-Za-z0-9_-]+)$/);
  if (!match) return false;
  try {
    const data = JSON.parse(fromBase64Url(match[1]));
    if (typeof data.schema !== 'string' || typeof data.query !== 'string') return false;
    dialectEl.value = DIALECTS.includes(data.dialect) ? data.dialect : 'postgresql';
    editors.schema.setValue(data.schema);
    editors.query.setValue(data.query);
    markCustom();
    analyze();
    return true;
  } catch (err) {
    console.warn('Could not decode shared playground link', err);
    return false;
  }
}

let toastTimer = 0;
function toast(text) {
  const el = $('#toast');
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), 2200);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

$('#share').addEventListener('click', async () => {
  const url = shareUrl();
  history.replaceState(null, '', url);
  toast((await copyText(url)) ? 'Link copied to clipboard' : 'Link is in the address bar');
});

$('#copy-install').addEventListener('click', async () => {
  toast((await copyText('npx sqlsift-cli')) ? 'Copied: npx sqlsift-cli' : 'npx sqlsift-cli');
});

exampleEl.addEventListener('change', () => {
  if (exampleEl.value === CUSTOM) return;
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
  loadExample(exampleEl.value);
});

dialectEl.addEventListener('change', () => analyze());

for (const editor of Object.values(editors)) {
  editor.ta.addEventListener('input', () => {
    markCustom();
    scheduleAnalyze();
  });
}

window.addEventListener('hashchange', () => loadFromHash());

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

populateExamples();
startWorker();
if (!loadFromHash()) loadExample(EXAMPLES[0].id);

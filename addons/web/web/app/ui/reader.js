// The reader (drawer tab "reader"): the last answers of a session, rendered for reading, and the
// formulas / lens items the session produced.
import { html } from 'htm/preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { model, reqQuiet } from '../store.js';
import { defineAction } from '../palette.js';
import { renderAnswer, formulaNode } from '../render/answer.js';
import { answerFormulas } from '../render/markdown.js';
import { registerDrawerTab, toggleDrawer } from './drawer.js';
import { toast } from './toasts.js';

const copied = () => toast({ text: 'Copied.', level: 'info' });

async function copy(text) {
  try { await navigator.clipboard.writeText(text); copied(); } catch { toast({ text: 'The browser refused to copy.', level: 'warn' }); }
}

// The body of one answer: markdown into a ref, re-rendered when the text changes.
export function Prose({ markdown, class: cls = '' }) {
  const ref = useRef(null);
  useEffect(() => { if (ref.current) renderAnswer(ref.current, markdown, { onCopy: copied }).catch(() => {}); }, [markdown]);
  return html`<div class=${`prose ${cls}`} ref=${ref}></div>`;
}

export function updatedText(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 5) return 'updated just now';
  if (s < 60) return `updated ${s} s ago`;
  return `updated ${Math.floor(s / 60)} min ago`;
}

function firstLine(s) {
  const l = String(s ?? '').trim().split('\n')[0];
  return l.length > 140 ? `${l.slice(0, 140)}…` : l || '(empty prompt)';
}

function AnswerCard({ a, i, count }) {
  return html`<article class="reader-answer" data-testid="reader-answer" data-index=${i}>
    <div class="answer-bar">
      <span class="answer-no">${i === 0 ? 'Latest answer' : `${i + 1} of ${count}`}</span>
      <button class="btn btn-sm" data-testid="copy-answer" onClick=${() => copy(a.text)}>Copy answer</button>
    </div>
    <details class="reader-prompt">
      <summary title="Show the prompt"><span class="prompt-mark">you</span>${firstLine(a.prompt)}</summary>
      <pre>${a.prompt}</pre>
    </details>
    <${Prose} markdown=${a.text} />
  </article>`;
}

const renderCache = new Map();

function LensItem({ item }) {
  const [out, setOut] = useState(renderCache.get(`${item.tag}\u0000${item.text}`) ?? null);
  useEffect(() => {
    const key = `${item.tag}\u0000${item.text}`;
    if (renderCache.has(key)) { setOut(renderCache.get(key)); return; }
    let dead = false;
    reqQuiet('extensions.render', { lens: item.tag, kind: 'tag', source: item.text })
      .then((r) => {
        const text = r.block?.length ? r.block.join('\n') : (r.inline ?? item.text);
        renderCache.set(key, text);
        if (!dead) setOut(text);
      })
      .catch(() => { if (!dead) setOut(item.text); });
    return () => { dead = true; };
  }, [item.tag, item.text]);
  return html`<li class="lens-item" data-testid="lens-item"><pre>${out ?? item.text}</pre></li>`;
}

function Formula({ tex }) {
  const ref = useRef(null);
  useEffect(() => {
    let dead = false;
    formulaNode(tex, true).then((n) => { if (!dead && ref.current) ref.current.replaceChildren(n); });
    return () => { dead = true; };
  }, [tex]);
  return html`<li class="formula" data-testid="formula">
    <div class="formula-math" ref=${ref}></div>
    <div class="formula-foot"><code>${tex}</code><button class="btn btn-sm" onClick=${() => copy(tex)}>Copy TeX</button></div>
  </li>`;
}

// `fromAnswers`: the formulas found in the rendered answers, for a host that does not send them
// (no `lens` feature) or a session whose list is still empty.
export function FormulasView({ sid, fromAnswers = [] }) {
  const lens = model.value.lens[sid];
  const items = lens?.items ?? [];
  const latex = items.filter((x) => x.tag === 'latex').map((x) => x.text);
  const fromLens = latex.length ? latex : (lens?.formulas ?? []);
  const formulas = fromLens.length ? fromLens : fromAnswers;
  const others = new Map();
  for (const it of items) if (it.tag !== 'latex') others.set(it.tag, [...(others.get(it.tag) ?? []), it]);
  if (!formulas.length && !others.size) return html`<p class="reader-empty">No formulas or lens items in this conversation yet. Claude writes formulas inside <code>${'<latex>'}</code> tags.</p>`;
  return html`<div class="lens-view" data-testid="formulas">
    ${formulas.length ? html`<section><h3>Formulas <span class="count">${formulas.length}</span></h3>
      <ol class="formula-list">${formulas.map((t, i) => html`<${Formula} key=${`${i}:${t}`} tex=${t} />`)}</ol></section>` : null}
    ${[...others].map(([tag, list]) => html`<section key=${tag}><h3>${tag} <span class="count">${list.length}</span></h3>
      <ul class="lens-list">${list.map((it, i) => html`<${LensItem} key=${`${i}:${it.text}`} item=${it} />`)}</ul></section>`)}
  </div>`;
}

export function ReaderPanel({ sid }) {
  const s = model.value.sessions[sid];
  const turns = s?.turns ?? null;
  // an answer lands at Stop: also refresh when the session settles (idle or waiting for input)
  const settled = s && (s.state === 'idle' || s.state === 'needs_input') ? s.state : 'busy';
  const [data, setData] = useState({ loading: true, answers: [], error: null, at: null });
  const [view, setView] = useState('answers');
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let dead = false;
    setData((d) => ({ ...d, loading: true, error: null }));
    reqQuiet('reader.answers', { sid })
      .then((r) => { if (!dead) setData({ loading: false, answers: r.answers ?? [], error: null, at: Date.now() }); })
      .catch((e) => { if (!dead) setData({ loading: false, answers: [], error: e.message, at: null }); });
    return () => { dead = true; };
  }, [sid, turns, settled, nonce]);
  const [tick, setTick] = useState(Date.now());
  useEffect(() => { const t = setInterval(() => setTick(Date.now()), 5000); return () => clearInterval(t); }, []);
  const derived = useMemo(() => answerFormulas(data.answers), [data.answers]);
  if (!s) return null;
  const lens = model.value.lens[sid];
  const nLens = ((lens?.items?.length ?? 0) || (lens?.formulas?.length ?? 0)) || derived.length;
  return html`<div class="reader" data-testid="reader" data-view=${view}>
    <div class="reader-bar">
      <div class="seg" role="tablist">
        <button class="seg-btn" role="tab" aria-selected=${view === 'answers'} data-testid="reader-view-answers" onClick=${() => setView('answers')}>Answers</button>
        <button class="seg-btn" role="tab" aria-selected=${view === 'formulas'} data-testid="reader-view-formulas" onClick=${() => setView('formulas')}>Formulas${nLens ? html` <span class="count">${nLens}</span>` : null}</button>
      </div>
      <span class="reader-updated" data-testid="reader-updated">${data.at ? updatedText(tick - data.at) : data.loading ? 'reading…' : ''}</span>
      <button class="icon-btn" title="Reload the answers" aria-label="Reload the answers" onClick=${() => setNonce(nonce + 1)}>↻</button>
    </div>
    <div class="reader-scroll">
      ${view === 'formulas' ? html`<${FormulasView} sid=${sid} fromAnswers=${derived} />`
        : s.kind !== 'claude' ? html`<p class="reader-empty">A terminal tab has no answers to read.</p>`
        : data.error ? html`<p class="reader-empty reader-error" role="alert">Could not read the answers: ${data.error}</p>`
        : data.loading && !data.answers.length ? html`<p class="reader-empty">Reading the conversation…</p>`
        : !data.answers.length ? html`<p class="reader-empty">No answers yet. They appear here after Claude replies.</p>`
        : data.answers.map((a, i) => html`<${AnswerCard} key=${`${i}:${a.prompt.length}:${a.text.length}`} a=${a} i=${i} count=${data.answers.length} />`)}
    </div>
  </div>`;
}

registerDrawerTab('reader', { title: 'Reader', order: 10, Component: ReaderPanel });
defineAction('reader', (sid) => toggleDrawer('reader', typeof sid === 'string' ? sid : undefined));

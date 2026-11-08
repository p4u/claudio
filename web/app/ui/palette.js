import { html } from 'htm/preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { BINDINGS } from '../keymap.js';
import { filter } from '../fuzzy.js';
import { overlay, registry, runAction, hasAction } from '../palette.js';
import { model, req, selected } from '../store.js';
import { orderedSessions } from '../model.js';
import { settings, updateSettings, THEMES } from '../settings.js';
import { confirm, prompt } from './modal.js';

const SKIP = new Set(['palette', 'palette-ctrl', 'close-overlay', 'leave-terminal', 'focus-pane-2', 'focus-pane-3', 'focus-pane-4']);

const quiet = (p) => p.catch(() => {});

export function sessionCommands(s) {
  const name = s.label || s.id.slice(0, 8);
  const tag = s.host === 'local' ? name : `${name} @${s.host}`;
  const cmds = [
    { id: `go:${s.id}`, group: 'Sessions', title: `Go to ${tag}`, run: () => { selected.value = s.id; } },
    { id: `kill:${s.id}`, group: 'Sessions', title: `Kill ${tag}`, run: async () => {
      if (await confirm({ title: 'Kill session', body: `Kill ${name}? The process ends.`, yes: 'Kill', no: 'Keep' })) await quiet(req('session.kill', { sid: s.id }));
    } },
    { id: `rename:${s.id}`, group: 'Sessions', title: `Rename ${tag}`, run: async () => {
      const v = await prompt({ title: 'Rename session', value: s.name ?? '', yes: 'Rename' });
      if (v !== null) await quiet(req('session.rename', { sid: s.id, name: v.trim() === '' ? null : v.trim() }));
    } },
  ];
  if (s.kind === 'claude') {
    cmds.push({ id: `restart:${s.id}`, group: 'Sessions', title: `Restart ${tag} (resume)`, run: () => quiet(req('session.respawn', { sid: s.id, fresh: false, rows: 24, cols: 80 })) });
    for (const [action, label] of [['reader', 'Reader'], ['changes', 'Changes'], ['history', 'History'], ['stats', 'Stats'], ['session', 'Session info']]) {
      cmds.push({ id: `${action}:${s.id}`, group: 'Sessions', title: `${label}: ${tag}`, when: () => hasAction(action), run: () => { selected.value = s.id; runAction(action, s.id); } });
    }
    if (s.proxy) cmds.push({ id: `switch:${s.id}`, group: 'Sessions', title: `Switch subscription: ${tag}`, run: () => quiet(req('proxy.switch', { sid: s.id })) });
  }
  return cmds;
}

export function buildCommands(m, cfg) {
  const out = [];
  for (const b of BINDINGS) {
    if (SKIP.has(b.action)) continue;
    out.push({ id: `action:${b.action}`, group: 'Actions', title: b.help[0].toUpperCase() + b.help.slice(1), keys: b.keys, when: () => hasAction(b.action), run: () => runAction(b.action) });
  }
  out.push({ id: 'settings', group: 'App', title: 'Settings', run: () => { overlay.value = 'settings'; } });
  for (const t of THEMES) {
    if (t !== cfg.theme) out.push({ id: `theme:${t}`, group: 'App', title: `Theme: ${t.replace('-', ' ')}`, run: () => updateSettings({ theme: t }) });
  }
  for (const s of orderedSessions(m)) out.push(...sessionCommands(s));
  return out;
}

export function Palette() {
  const [q, setQ] = useState('');
  const [sel, setSel] = useState(0);
  const input = useRef(null);
  const list = useRef(null);
  const m = model.value;
  const cfg = settings.value;
  const all = useMemo(
    () => [...buildCommands(m, cfg), ...registry.value].filter((c) => !c.when || c.when()),
    [m, cfg, registry.value],
  );
  const items = useMemo(() => filter(all, q, (c) => c.title).slice(0, 60), [all, q]);
  useLayoutEffect(() => {
    const prev = document.activeElement;
    input.current?.focus();
    return () => { if (prev && prev.isConnected) prev.focus(); };
  }, []);
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }); }, [sel, items]);
  const close = () => { overlay.value = null; };
  const run = (c) => { close(); c.run(); };
  const keydown = (e) => {
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); setSel((sel + 1) % Math.max(items.length, 1)); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setSel((sel - 1 + items.length) % Math.max(items.length, 1)); }
    else if (e.key === 'Enter') { e.preventDefault(); if (items[sel]) run(items[sel]); }
  };
  let last = null;
  return html`<div class="scrim" onMouseDown=${(e) => { if (e.target === e.currentTarget) close(); }}>
    <div class="palette popover" data-testid="palette" role="dialog" aria-modal="true" aria-label="Command palette" onKeyDown=${keydown}>
      <div class="palette-input-row">
        <span class="prompt" aria-hidden="true">${">"}</span>
        <input class="palette-input" data-testid="palette-input" ref=${input} role="combobox" aria-expanded="true" aria-controls="palette-list"
          placeholder="Type a command or a session" autocomplete="off" spellcheck=${false} value=${q}
          onInput=${(e) => { setQ(e.target.value); setSel(0); }} />
      </div>
      <ul class="palette-list" id="palette-list" role="listbox" ref=${list}>
        ${items.length === 0 ? html`<li class="palette-empty">No match</li>` : items.map((c, i) => {
          const head = !q && c.group && c.group !== last ? html`<li class="palette-group" role="presentation">${c.group}</li>` : null;
          last = c.group;
          return html`${head}<li key=${c.id} class="palette-item" data-testid="palette-item" role="option" aria-selected=${i === sel}
            onMouseMove=${() => sel !== i && setSel(i)} onClick=${() => run(c)}>
            <span class="title">${c.title}</span>
            ${c.keys ? html`<kbd>${c.keys}</kbd>` : null}
          </li>`;
        })}
      </ul>
      <div class="palette-foot"><span><kbd>↑</kbd> <kbd>↓</kbd> move</span><span><kbd>Enter</kbd> run</span><span><kbd>Esc</kbd> close</span></div>
    </div>
  </div>`;
}

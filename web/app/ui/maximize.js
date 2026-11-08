// "Fill the window": the focused pane covers the whole browser viewport. Nothing is unmounted (the
// other panes are only hidden by CSS), so terminals keep their state and attachments. Per browser
// tab, never saved. Esc is not bound: it keeps going to claude.
import { signal, effect } from '@preact/signals';
import { model } from '../store.js';
import { defineAction, register } from '../palette.js';
import { refitTerminal } from '../term.js';
import { toast } from './toasts.js';
import { boardShown } from './session-actions.js';
import { captureKeys } from './capture.js';

export const maximized = signal(null); // the pane index that fills the window, or null

export function toggleMaximize(pane) {
  if (maximized.value !== null) { maximized.value = null; return; }
  const lay = model.value.layout;
  const i = pane ?? lay.focus;
  if (boardShown(lay) || !lay.panes[i]) { toast({ text: 'Focus a pane with a session first.', level: 'warn' }); return; }
  maximized.value = i;
}

export function installMaximize() {
  defineAction('fullscreen-pane', () => toggleMaximize());
  register({ id: 'fullscreen-pane', group: 'App', title: 'Fill the window with this pane', keys: 'Alt+Shift+F', run: () => toggleMaximize() });
  register({ id: 'fullscreen-browser', group: 'App', title: 'Fullscreen browser (and fill the window)', run: async () => {
    if (maximized.value === null) toggleMaximize();
    if (maximized.value !== null) await captureKeys();
  } });
  // the root attribute drives the CSS; the maximized pane follows the focus, and ends when it has no session
  let last;
  effect(() => {
    const i = maximized.value;
    const lay = model.value.layout;
    if (i !== null) {
      if (boardShown(lay) || !lay.panes[lay.focus]) { maximized.value = null; return; }
      if (lay.focus !== i) { maximized.value = lay.focus; return; }
    }
    document.documentElement.toggleAttribute('data-maximized', i !== null);
    if (i === last) return;
    last = i;
    const sid = lay.panes[lay.focus];
    if (sid) setTimeout(() => refitTerminal(sid), 0);
  });
}

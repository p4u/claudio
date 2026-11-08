import { html } from 'htm/preact';
import { BINDINGS } from '../keymap.js';
import { overlay } from '../palette.js';
import { Dialog } from './modal.js';

function Keys({ keys }) {
  return html`<span>${keys.split('+').map((k, i) => html`${i > 0 ? ' ' : ''}<kbd>${k}</kbd>`)}</span>`;
}

export function Help() {
  const rows = BINDINGS.filter((b) => !b.hide && !/^focus-pane-[234]$/.test(b.action));
  return html`<${Dialog} title="Keys" testid="help" wide onClose=${() => { overlay.value = null; }}>
    <div class="help-list">
      ${rows.map((b) => html`<${Keys} keys=${b.action === 'focus-pane-1' ? '1' : b.keys} />
        <span>${b.action === 'focus-pane-1' ? 'focus pane 1 to 4' : b.help}</span>`)}
      <${Keys} keys="Ctrl+K" /><span>command palette, also in text fields (not in the terminal)</span>
    </div>
    <p class="help-note">Single keys work in app mode, when no text field has focus. In terminal mode every key goes to the terminal; only <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Space</kbd> comes back here.</p>
  <//>`;
}

import { html } from 'htm/preact';
import { BINDINGS, TUI_BINDINGS } from '../keymap.js';
import { overlay } from '../palette.js';
import { Dialog } from './modal.js';
import { keyCheck, keyCheckLog } from './key-check.js';
import { captureKeys, captured } from './capture.js';

function Keys({ keys }) {
  return html`<span>${keys.split('+').map((k, i) => html`${i > 0 ? ' ' : ''}<kbd>${k}</kbd>`)}</span>`;
}

export function Help() {
  const tui = TUI_BINDINGS.filter((b) => !/^goto-session-([2-9]|10)$/.test(b.action));
  const rows = BINDINGS.filter((b) => !b.hide && !/^focus-pane-[234]$/.test(b.action));
  return html`<${Dialog} title="Keys" testid="help" wide onClose=${() => { overlay.value = null; }}>
    <div class="help-list">
      ${rows.map((b) => html`<${Keys} keys=${b.action === 'focus-pane-1' ? '1' : b.keys} />
        <span>${b.action === 'focus-pane-1' ? 'focus pane 1 to 4' : b.help}</span>`)}
      <${Keys} keys="Ctrl+K" /><span>command palette, also in text fields (not in the terminal)</span>
    </div>
    <h3 class="help-h">Anywhere, in a terminal too (the same keys as the TUI)</h3>
    <div class="help-list" data-testid="help-tui">
      ${tui.map((b) => html`<${Keys} keys=${b.action === 'goto-session-1' ? 'Alt+Shift+1…9, 0' : b.keys.replace('Alt+←', 'Alt+Left').replace('Alt+→', 'Alt+Right')} /><span>${b.help}</span>`)}
    </div>
    <p class="help-note">Browsers keep Ctrl+T, Ctrl+W, Ctrl+N and Ctrl+Tab in a normal tab. Use Capture all keys (fullscreen), or install claudio as an app (browser menu, Install claudio) and capture there too. Shift+Enter inserts a newline in claude.</p>
    <p><button type="button" class="btn btn-sm" data-testid="capture-keys-help" aria-pressed=${captured.value} onClick=${captureKeys}>Capture all keys</button></p>
    <section data-testid="key-check">
      <h3 class="help-h">Key check</h3>
      <p class="help-note">Turn it on, then press keys: each one is described here and nothing is sent to a session. Esc closes the help.</p>
      <button type="button" class="btn btn-sm" data-testid="key-check-toggle" aria-pressed=${keyCheck.value} onClick=${() => { keyCheck.value = !keyCheck.value; keyCheckLog.value = ''; }}>${keyCheck.value ? 'Stop key check' : 'Start key check'}</button>
      <pre class="key-check-out" data-testid="key-check-out" aria-live="polite">${keyCheck.value ? keyCheckLog.value || 'Press a key…' : ''}</pre>
    </section>
    <p class="help-note">Single keys work in app mode, when no text field has focus. In terminal mode every key goes to the terminal; only <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Space</kbd> comes back here.</p>
  <//>`;
}

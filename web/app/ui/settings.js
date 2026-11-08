import { html } from 'htm/preact';
import { settings, updateSettings, TEXT_SIZES } from '../settings.js';
import { overlay } from '../palette.js';
import { toast } from './toasts.js';
import { Dialog } from './modal.js';
import { captureKeys, captured } from './capture.js';

async function setNotifications(on) {
  if (!on) { updateSettings({ notifications: false }); return; }
  if (!globalThis.Notification) { toast({ text: 'This browser has no notifications.', level: 'warn' }); return; }
  const p = await Notification.requestPermission();
  if (p === 'granted') updateSettings({ notifications: true });
  else toast({ text: 'Notifications are blocked for this site.', level: 'warn' });
}

export function Settings() {
  const s = settings.value;
  return html`<${Dialog} title="Settings" testid="settings" wide onClose=${() => { overlay.value = null; }}>
    <label class="set-row"><span>Text size<small>the interface text; the terminal has its own size</small></span>
      <select class="select" data-testid="text-size-select" data-autofocus value=${s.textSize} onChange=${(e) => updateSettings({ textSize: e.target.value })}>
        ${TEXT_SIZES.map((t) => html`<option value=${t} selected=${t === s.textSize}>${t[0].toUpperCase() + t.slice(1)}</option>`)}
      </select></label>
    <label class="set-row"><span>Terminal font size<small>pixels</small></span>
      <input class="input" type="number" min="10" max="24" value=${s.fontSize} onChange=${(e) => updateSettings({ fontSize: e.target.value })} /></label>
    <label class="set-row"><span>Render formulas in terminals<small>show each matched ${"<latex>"} tag typeset, over the text</small></span>
      <input class="toggle" type="checkbox" data-testid="render-formulas" checked=${s.renderFormulas} onChange=${(e) => updateSettings({ renderFormulas: e.target.checked })} /></label>
    <label class="set-row"><span>Show a composer under each pane<small>off: claude's own input is the only one; Compose (Alt+O) opens an editor</small></span>
      <input class="toggle" type="checkbox" data-testid="inline-composer" checked=${s.inlineComposer} onChange=${(e) => updateSettings({ inlineComposer: e.target.checked })} /></label>
    <label class="set-row"><span>Sound<small>off by default</small></span>
      <input class="toggle" type="checkbox" checked=${s.sound} onChange=${(e) => updateSettings({ sound: e.target.checked })} /></label>
    <div class="set-row"><span>Extensions<small>lenses, actions and proxy plugins</small></span>
      <button type="button" class="btn btn-sm" data-testid="open-extensions" onClick=${() => { overlay.value = 'extensions'; }}>Manage…</button></div>
    <div class="set-row"><span>Capture all keys<small>fullscreen, so Ctrl+T, Ctrl+W, Ctrl+N and Ctrl+Tab reach the terminal</small></span>
      <button type="button" class="btn btn-sm" data-testid="capture-keys" aria-pressed=${captured.value} onClick=${captureKeys}>${captured.value ? 'Release' : 'Capture'}</button></div>
    <label class="set-row"><span>Notifications<small>when a background session needs you</small></span>
      <input class="toggle" type="checkbox" checked=${s.notifications} onChange=${(e) => setNotifications(e.target.checked)} /></label>
  <//>`;
}

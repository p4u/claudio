// User settings in localStorage['claudio.settings'].
import { signal } from '@preact/signals';
import { clampDrawer, DRAWER_DEFAULT } from './layout.js';

export const TEXT_SIZES = ['small', 'normal', 'large'];
export const DEFAULTS = { fontSize: 15, textSize: 'normal', sound: false, notifications: false, drawerWidth: DRAWER_DEFAULT, railCollapsed: false, renderFormulas: true, inlineComposer: false };
const KEY = 'claudio.settings';

export function sanitize(raw) {
  const o = raw && typeof raw === 'object' ? raw : {};
  const size = Number(o.fontSize);
  return {
    textSize: TEXT_SIZES.includes(o.textSize) ? o.textSize : DEFAULTS.textSize,
    fontSize: Number.isFinite(size) ? Math.min(24, Math.max(10, Math.round(size))) : DEFAULTS.fontSize,
    sound: o.sound === true,
    drawerWidth: clampDrawer(o.drawerWidth ?? DRAWER_DEFAULT),
    railCollapsed: o.railCollapsed === true,
    renderFormulas: o.renderFormulas !== false,
    inlineComposer: o.inlineComposer === true,
    notifications: o.notifications === true && globalThis.Notification?.permission === 'granted',
  };
}

function load() {
  try { return sanitize(JSON.parse(localStorage.getItem(KEY))); } catch { return { ...DEFAULTS }; }
}

export const settings = signal(load());

export function updateSettings(patch) {
  const next = sanitize({ ...settings.value, ...patch });
  settings.value = next;
  try { localStorage.setItem(KEY, JSON.stringify(next)); } catch { /* private mode */ }
}

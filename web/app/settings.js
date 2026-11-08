// User settings in localStorage['claudio.settings'].
import { signal } from '@preact/signals';
import { clampDrawer, DRAWER_DEFAULT } from './layout.js';

export const THEMES = ['arcade-dark', 'arcade-light', 'plain-dark', 'plain-light'];
export const DEFAULTS = { theme: 'arcade-dark', scanlines: true, fontSize: 14, sound: false, notifications: false, drawerWidth: DRAWER_DEFAULT, railCollapsed: false, renderFormulas: true };
const KEY = 'claudio.settings';

export function sanitize(raw) {
  const o = raw && typeof raw === 'object' ? raw : {};
  const size = Number(o.fontSize);
  return {
    theme: THEMES.includes(o.theme) ? o.theme : DEFAULTS.theme,
    scanlines: typeof o.scanlines === 'boolean' ? o.scanlines : DEFAULTS.scanlines,
    fontSize: Number.isFinite(size) ? Math.min(24, Math.max(10, Math.round(size))) : DEFAULTS.fontSize,
    sound: o.sound === true,
    drawerWidth: clampDrawer(o.drawerWidth ?? DRAWER_DEFAULT),
    railCollapsed: o.railCollapsed === true,
    renderFormulas: o.renderFormulas !== false,
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

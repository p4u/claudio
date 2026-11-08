// Applies settings to <html>: data-theme, scanlines and the terminal font size.
import { effect } from '@preact/signals';
import { settings } from './settings.js';

export function applyTheme(s = settings.value) {
  const root = document.documentElement;
  root.dataset.theme = s.theme;
  root.dataset.text = s.textSize;
  root.dataset.scanlines = s.scanlines && s.theme.startsWith('arcade') ? 'on' : 'off';
  root.style.setProperty('--term-font-size', `${s.fontSize}px`);
}

export function watchTheme() {
  return effect(() => applyTheme(settings.value));
}

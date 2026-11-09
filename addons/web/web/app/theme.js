// Applies settings to <html>: the text size and the terminal font size. The UI is dark only:
// data-theme is always "dark" (DESIGN.md), kept because the terminal and the renderers read it.
import { effect } from '@preact/signals';
import { settings } from './settings.js';

export function applyTheme(s = settings.value) {
  const root = document.documentElement;
  root.dataset.theme = 'dark';
  root.dataset.text = s.textSize;
  root.style.setProperty('--term-font-size', `${s.fontSize}px`);
}

export function watchTheme() {
  return effect(() => applyTheme(settings.value));
}

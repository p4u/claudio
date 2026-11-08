// "Capture all keys": fullscreen plus the Keyboard Lock API, so Ctrl+T/W/N/Tab reach the terminal.
import { signal } from '@preact/signals';
import { defineAction, register } from '../palette.js';
import { toast } from './toasts.js';

export const captured = signal(false);

export async function captureKeys() {
  const de = document.documentElement;
  if (captured.value || document.fullscreenElement) {
    if (document.fullscreenElement) await document.exitFullscreen().catch(() => {});
    return;
  }
  if (!de.requestFullscreen) { toast({ text: 'This browser cannot go fullscreen, so it cannot capture keys. Install claudio as an app instead (browser menu).', level: 'warn' }); return; }
  try {
    await de.requestFullscreen();
  } catch (e) {
    toast({ text: `Fullscreen was refused: ${e.message}`, level: 'warn' });
    return;
  }
  if (!navigator.keyboard?.lock) {
    toast({ text: 'Fullscreen on, but this browser cannot lock keys: Ctrl+T, Ctrl+W and Ctrl+N stay with the browser (Chromium has it). Install claudio as an app instead.', level: 'warn' });
    return;
  }
  try {
    await navigator.keyboard.lock();
    captured.value = true;
    toast({ text: 'All keys go to claudio now, Ctrl+T, Ctrl+W, Ctrl+N and Ctrl+Tab included. Hold Esc to leave fullscreen.' });
  } catch (e) {
    toast({ text: `Key lock was refused: ${e.message}`, level: 'warn' });
  }
}

document.addEventListener('fullscreenchange', () => {
  if (document.fullscreenElement) return;
  if (captured.value) navigator.keyboard?.unlock?.();
  captured.value = false;
});

defineAction('capture-keys', captureKeys);
register({ id: 'capture-keys', group: 'App', title: 'Capture all keys (fullscreen)', run: captureKeys });

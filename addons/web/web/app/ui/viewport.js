// The window width as a signal, for the layout plan (rail strip, drawer width).
import { signal } from '@preact/signals';

export const viewportW = signal(globalThis.innerWidth ?? 1280);
let raf = 0;
globalThis.addEventListener?.('resize', () => {
  if (raf) return;
  raf = requestAnimationFrame(() => { raf = 0; viewportW.value = globalThis.innerWidth; });
});

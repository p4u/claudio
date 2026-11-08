// The narrow layout (< 760 px): the board only, no panes.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';

const query = globalThis.matchMedia?.('(max-width: 759px)');
export const narrow = signal(query?.matches ?? false);
query?.addEventListener?.('change', (e) => { narrow.value = e.matches; });

// The board imports `narrow` from this file, so app.js passes it in.
export function NarrowView({ Board }) {
  return html`<main class="narrow" data-testid="narrow">
    <${Board} />
  </main>`;
}

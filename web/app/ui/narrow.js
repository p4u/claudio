// The narrow layout (< 760 px): the board and the queue only, no panes.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';

const query = globalThis.matchMedia?.('(max-width: 759px)');
export const narrow = signal(query?.matches ?? false);
query?.addEventListener?.('change', (e) => { narrow.value = e.matches; });

// The queue and the board import `narrow` from this file, so app.js passes them in.
export function NarrowView({ Queue, Board }) {
  return html`<main class="narrow" data-testid="narrow">
    <section class="rail-section narrow-queue">
      <${Queue} />
    </section>
    <${Board} />
  </main>`;
}

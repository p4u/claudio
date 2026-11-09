// A session's addon segments (docs/web-protocol.md §4 "Addons"): chips by priority, coloured by
// their tone through the tokens. Every text is an addon's: text nodes only.
import { html } from 'htm/preact';
import { sortSegments, tone } from '../addon-view.js';

export function Segments({ s, testid = 'segments' }) {
  const list = sortSegments(s?.segments);
  if (!list.length) return null;
  return html`<span class="segments" data-testid=${testid}>
    ${list.map((g) => html`<span key=${`${g.addon}/${g.id}`} class="chip segment" data-testid="segment" data-segment=${`${g.addon}/${g.id}`} data-tone=${tone(g.tone)} title=${`${g.addon}: ${g.text}`}>${g.text}</span>`)}
  </span>`;
}

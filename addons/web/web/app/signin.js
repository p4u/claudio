// The sign-in ticket rules of docs/web-protocol.md §1 ("Sign-in tickets"). Imports nothing.

/** What the page shows when a ticket was refused and the server offers no other way in. */
export const TICKET_EXPIRED = 'This sign-in link has expired. Run `claudio web` again.';

/**
 * The ticket a fragment carries: null when it is not a ticket fragment (no `#k=`), else
 * `{ ticket }`, where `ticket` is null unless the fragment is exactly `#k=<64 lowercase hex>`
 * (a refused ticket, never posted).
 */
export function ticketOf(hash) {
  if (!hash.startsWith('#k=')) return null;
  const m = /^#k=([0-9a-f]{64})$/.exec(hash);
  return { ticket: m ? m[1] : null };
}

/**
 * After a refused or failed ticket, what `/api/me` leads to: 'start' (a live cookie), 'password'
 * (the server offers the password) or 'expired'.
 */
export function afterRefusal(meStatus, methods) {
  if (meStatus === 204) return 'start';
  if (methods.includes('password')) return 'password';
  return 'expired';
}

// Reads of server state that the page also changes optimistically (the extensions screen's
// toggles). A read whose answer may predate a write is stale: it started before a write, or
// while one was in flight (the server may answer it first), or a write started meanwhile.
// Its answer is dropped; each write's end reads again. Imports nothing.

export function makeReads() {
  let epoch = 0; // writes started
  let busy = 0; // writes in flight
  return {
    // A read starts: its ticket.
    begin() {
      return { epoch, busy: busy > 0 };
    },
    // Whether the answer to the read of `ticket` may be shown.
    fresh(ticket) {
      return !ticket.busy && busy === 0 && ticket.epoch === epoch;
    },
    // A write starts (the page already shows its result).
    write() {
      epoch += 1;
      busy += 1;
    },
    // A write ended (done or failed): the caller reads again.
    done() {
      busy = Math.max(0, busy - 1);
    },
  };
}

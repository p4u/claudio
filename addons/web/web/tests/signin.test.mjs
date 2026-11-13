import test from 'node:test';
import assert from 'node:assert/strict';
import { ticketOf, afterRefusal, TICKET_EXPIRED } from '../app/signin.js';

const HEX = '0123456789abcdef'.repeat(4);

test('only #k= followed by 64 lowercase hex is a ticket to post', () => {
  assert.deepEqual(ticketOf(`#k=${HEX}`), { ticket: HEX });
  for (const bad of ['#k=', `#k=${HEX.slice(1)}`, `#k=${HEX}0`, `#k=${HEX.toUpperCase()}`, `#k=${HEX}&x`, `#k=${HEX.slice(2)}zz`]) {
    assert.deepEqual(ticketOf(bad), { ticket: null }, bad);
  }
});

test('other fragments are not ticket fragments', () => {
  for (const hash of ['', '#', `#t=${HEX}`, `#K=${HEX}`, '#k', `#x#k=${HEX}`]) assert.equal(ticketOf(hash), null, hash);
});

test('a refused ticket starts the app with a cookie, else the password, else expires', () => {
  assert.equal(afterRefusal(204, []), 'start');
  assert.equal(afterRefusal(401, ['token', 'password']), 'password');
  assert.equal(afterRefusal(401, ['password']), 'password');
  assert.equal(afterRefusal(401, ['token']), 'expired');
  assert.equal(afterRefusal(500, []), 'expired');
  assert.match(TICKET_EXPIRED, /expired\. Run `claudio web` again\.$/);
});

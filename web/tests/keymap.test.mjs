import test from 'node:test';
import assert from 'node:assert/strict';
import { BINDINGS, isLeaveTerminal, appAction, forbidden } from '../app/keymap.js';

const ev = (key, o = {}) => ({ key, code: o.code ?? (key.length === 1 ? 'Key' + key.toUpperCase() : key), ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...o });
const app = { typing: false, overlay: false };

test('only Ctrl+Shift+Space leaves the terminal', () => {
  assert.equal(isLeaveTerminal(ev(' ', { code: 'Space', ctrlKey: true, shiftKey: true })), true);
  assert.equal(isLeaveTerminal(ev(' ', { code: 'Space', ctrlKey: true })), false);
  assert.equal(isLeaveTerminal(ev(' ', { code: 'Space', shiftKey: true })), false);
  assert.equal(isLeaveTerminal(ev('k', { ctrlKey: true })), false);
  assert.equal(isLeaveTerminal(ev('Escape')), false);
  assert.equal(isLeaveTerminal(ev(' ', { code: 'Space', ctrlKey: true, shiftKey: true, altKey: true })), false);
});

test('app mode maps k, Ctrl+K, a, n, t, 1-4, r, g, ?, Enter, Escape', () => {
  const want = [
    [ev('k'), 'palette'], [ev('k', { ctrlKey: true }), 'palette'], [ev('a'), 'next-attention'],
    [ev('n'), 'new-session'], [ev('t'), 'new-terminal'],
    [ev('1'), 'focus-pane-1'], [ev('2'), 'focus-pane-2'], [ev('3'), 'focus-pane-3'], [ev('4'), 'focus-pane-4'],
    [ev('ArrowDown'), 'rail-next'], [ev('ArrowUp'), 'rail-prev'],
    [ev('r'), 'reader'], [ev('g'), 'changes'], [ev('?', { shiftKey: true }), 'help'],
    [ev('Enter'), 'enter-terminal'], [ev('Escape'), 'close-overlay'],
  ];
  for (const [e, a] of want) assert.equal(appAction(e, app), a, a);
  assert.equal(appAction(ev('x'), app), null);
  assert.equal(appAction(ev('k', { altKey: true }), app), null);
  assert.equal(appAction(ev('a', { metaKey: true }), app), null);
  assert.equal(appAction(ev('5'), app), null);
});

test('typing in an input suppresses single-key actions', () => {
  const typing = { typing: true, overlay: false };
  for (const k of ['k', 'a', 'n', 't', '1', 'r', 'g', '?', 'Enter', 'ArrowDown']) assert.equal(appAction(ev(k), typing), null, k);
  assert.equal(appAction(ev('k', { ctrlKey: true }), typing), 'palette');
  const overlay = { typing: false, overlay: true };
  assert.equal(appAction(ev('Escape'), overlay), 'close-overlay');
  assert.equal(appAction(ev('n'), overlay), null);
});

test('no binding is forbidden', () => {
  assert.ok(BINDINGS.length >= 14);
  for (const b of BINDINGS) {
    assert.ok(b.action && b.keys && b.help, JSON.stringify(b));
    assert.equal(forbidden(b.ev), false, b.keys);
  }
  assert.equal(forbidden(ev('a', { altKey: true })), true);
  assert.equal(forbidden(ev('w', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('t', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('n', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('Tab', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('k', { ctrlKey: true })), false);
});

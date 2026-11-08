import test from 'node:test';
import assert from 'node:assert/strict';
import { BINDINGS, TUI_BINDINGS, tuiBinding, shiftEnterBytes, terminalBytes, describeKey, isLeaveTerminal, appAction, forbidden } from '../app/keymap.js';

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

test('no binding is forbidden, and Alt+letter is allowed because the TUI parity keys use it', () => {
  assert.ok(BINDINGS.length >= 14);
  for (const b of BINDINGS) {
    assert.ok(b.action && b.keys && b.help, JSON.stringify(b));
    assert.equal(forbidden(b.ev), false, b.keys);
  }
  // Alt+letter is no longer forbidden: the TUI's own bindings are exactly these (see tuiBinding tests).
  assert.equal(forbidden(ev('a', { altKey: true })), false);
  for (const b of TUI_BINDINGS) assert.equal(forbidden(b.ev), false, b.keys);
  assert.equal(forbidden(ev('w', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('t', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('n', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('Tab', { ctrlKey: true })), true);
  assert.equal(forbidden(ev('k', { ctrlKey: true })), false);
});

const alt = (code, o = {}) => ({ key: code, code, ctrlKey: false, shiftKey: false, altKey: true, metaKey: false, ...o });

test('every TUI binding maps from ev.code', () => {
  const want = {
    ArrowLeft: 'session-prev', ArrowRight: 'session-next', KeyN: 'new-session', KeyC: 'terminal-beside',
    KeyR: 'rename-focused', KeyX: 'kill-focused', KeyE: 'restart-focused', KeyA: 'next-attention',
    KeyG: 'board', KeyS: 'stats', KeyL: 'history', KeyU: 'reader', KeyI: 'extensions', KeyH: 'help', KeyZ: 'palette',
  };
  for (const [code, action] of Object.entries(want)) assert.equal(tuiBinding(alt(code)), action, code);
  // layout independent: the key value is whatever the layout makes of it
  assert.equal(tuiBinding(alt('KeyU', { key: 'ü' })), 'reader');
  assert.equal(tuiBinding(alt('KeyN', { key: 'N' })), 'new-session');
});

test('Alt+Shift+digit goes to session 1..10', () => {
  for (let n = 1; n <= 9; n++) assert.equal(tuiBinding(alt('Digit' + n, { shiftKey: true, key: '!' })), `goto-session-${n}`);
  assert.equal(tuiBinding(alt('Digit0', { shiftKey: true, key: ')' })), 'goto-session-10');
  assert.equal(tuiBinding(alt('Digit1')), null);
});

test('Alt+q, Alt+Enter, Ctrl+Alt+n, AltGr-like and Alt+Shift+letter are not bindings', () => {
  assert.equal(tuiBinding(alt('KeyQ')), null);
  assert.equal(tuiBinding(alt('Enter')), null);
  assert.equal(tuiBinding(alt('KeyN', { ctrlKey: true })), null);
  assert.equal(tuiBinding(alt('KeyU', { ctrlKey: true })), null);
  assert.equal(tuiBinding(alt('KeyN', { metaKey: true })), null);
  assert.equal(tuiBinding(alt('KeyN', { shiftKey: true })), null);
  assert.equal(tuiBinding(alt('ArrowLeft', { shiftKey: true })), null);
  assert.equal(tuiBinding({ key: 'n', code: 'KeyN', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false }), null);
});

test('the table has one row per action and the help can list it', () => {
  const seen = new Set();
  for (const b of TUI_BINDINGS) {
    assert.ok(b.keys.startsWith('Alt+') && b.help && b.action, b.keys);
    assert.equal(seen.has(b.action), false, b.action);
    seen.add(b.action);
  }
  assert.ok(!TUI_BINDINGS.some((b) => b.keys === 'Alt+q'));
});

test('Shift+Enter sends ESC CR, plain Enter stays CR', () => {
  assert.equal(shiftEnterBytes(ev('Enter', { shiftKey: true })), '\x1b\r');
  assert.equal(shiftEnterBytes(ev('Enter')), null);
  assert.equal(shiftEnterBytes(ev('Enter', { altKey: true })), null);
  assert.equal(shiftEnterBytes(ev('Enter', { shiftKey: true, ctrlKey: true })), null);
  assert.equal(shiftEnterBytes(ev('a', { shiftKey: true })), null);
  assert.equal(terminalBytes(ev('Enter')), '\r');
  assert.equal(terminalBytes(ev('Enter', { shiftKey: true })), '\x1b\r');
});

test('key check says what claudio does with a key', () => {
  assert.match(describeKey(alt('KeyU')), /claudio: reader/);
  assert.match(describeKey(ev('c', { ctrlKey: true })), /sent to the session: \\x03/);
  assert.match(describeKey(ev('Tab', { shiftKey: true })), /sent to the session: \\x1b\[Z/);
  assert.match(describeKey(ev('F5')), /goes to the page/);
  assert.match(describeKey(ev('b', { altKey: true })), /\\x1bb/);
  assert.match(describeKey(ev('Shift', { shiftKey: true })), /goes to the page/);
  assert.match(describeKey(ev('a')), /key=a/);
});

test('Alt+O opens Compose and Alt+Shift+F fills the window', () => {
  assert.equal(tuiBinding(alt('KeyO', { key: 'o' })), 'compose');
  assert.equal(tuiBinding(alt('KeyF', { shiftKey: true, key: 'F' })), 'fullscreen-pane');
  assert.equal(tuiBinding(alt('KeyF')), null);
  assert.equal(tuiBinding(alt('KeyO', { shiftKey: true })), null);
  assert.equal(tuiBinding(alt('KeyF', { shiftKey: true, ctrlKey: true })), null);
  // Claude Code's own Alt keys stay with the session
  for (const c of ['KeyP', 'KeyT', 'KeyM', 'KeyB', 'KeyF', 'KeyD']) assert.equal(tuiBinding(alt(c)), null, c);
  assert.equal(tuiBinding(alt('Escape')), null);
});

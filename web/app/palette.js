// Command registry for the palette. Later tasks register their commands here.
import { signal } from '@preact/signals';

export const registry = signal([]);
const handlers = new Map();

// register({id, title, keys?, run, when?}) -> unregister
export function register(command) {
  registry.value = [...registry.value.filter((c) => c.id !== command.id), command];
  return () => { registry.value = registry.value.filter((c) => c !== command); };
}

// Action names (BINDINGS) are run through handlers set by the screen that owns them.
export function defineAction(name, fn) {
  handlers.set(name, fn);
  return () => { if (handlers.get(name) === fn) handlers.delete(name); };
}

export function hasAction(name) {
  return handlers.has(name);
}

export function runAction(name, arg) {
  const fn = handlers.get(name);
  if (!fn) return false;
  fn(arg);
  return true;
}

// Which overlay is open: 'palette' | 'help' | 'settings' | null.
export const overlay = signal(null);

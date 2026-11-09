// The help's key check: while on, each key is described and nothing runs (app.js's capture handler).
import { signal } from '@preact/signals';

export const keyCheck = signal(false);
export const keyCheckLog = signal('');

// The Compose drafts of this browser: unsent text per session (kept across reloads in localStorage)
// and the Recent ring. One store, shared by the dialog, the inline composer and the library panel.
import { createDraftStore } from '../draftstore.js';

export const drafts = createDraftStore({ storage: typeof localStorage === 'undefined' ? null : localStorage });

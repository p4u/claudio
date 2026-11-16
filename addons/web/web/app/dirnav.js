// The new-session wizard's folder field, pure part: a typed path split into the folder it lists
// and the fragment being completed, the rows shown under it, and the edits its keys make.
// Imports nothing (node --test runs it without a browser).

// The daemon's cap on one listing: a listing this long may be cut even when it does not say so
// (an older daemon cut at 5000 without setting `truncated`).
export const MAX_LISTED = 5000;

// Whether the text browses folders: a path, starting with `/` or `~`.
export const isPath = (text) => /^[/~]/.test(String(text ?? '').trim());

// `{ parent, fragment }`: `parent` is the text up to and including its last `/` (the folder that
// is listed, as typed), `fragment` what follows it. A bare `~` lists home.
export function splitPath(text) {
  const t = String(text ?? '').trim();
  if (t === '~') return { parent: '~/', fragment: '' };
  const i = t.lastIndexOf('/');
  return { parent: t.slice(0, i + 1), fragment: t.slice(i + 1) };
}

// The folder the text denotes: the text itself without a trailing `/` (`~/src/` is `~/src`,
// `/` stays `/`). Never the parent in place of the typed text.
export function useTarget(text) {
  const t = String(text ?? '').trim();
  const trimmed = t.replace(/\/+$/, '');
  return trimmed || (t.startsWith('/') ? '/' : t);
}

// `~` and `~/…` made absolute with `home` (the host's, from `fs.list {path: "~"}`); null while the
// home is unknown. `~user` is left as it is (the daemon does not expand it either).
export function resolveTilde(text, home) {
  const t = String(text ?? '').trim();
  if (t !== '~' && !t.startsWith('~/')) return t;
  if (!home) return null;
  return home.replace(/\/+$/, '') + t.slice(1) || '/';
}

// `path` with `home` shown as `~` (for a hit's folder).
export function abbreviateHome(path, home) {
  const h = String(home ?? '').replace(/\/+$/, '');
  if (!h || !path) return path;
  if (path === h) return '~';
  return path.startsWith(`${h}/`) ? `~${path.slice(h.length)}` : path;
}

// What the field's text starts in: `{ cwd }` (absolute, `~` resolved) or `{ why }` (shown under
// the field; null for an empty field). `homeError`: why the home could not be read.
export function startFolder(text, home, homeError = null) {
  const t = String(text ?? '').trim();
  if (!t) return { why: null };
  if (!isPath(t)) return { why: 'Type a path (it starts with / or ~)' };
  if (t.startsWith('~') && t !== '~' && !t.startsWith('~/')) return { why: '~user is not expanded: type the full path' };
  const cwd = resolveTilde(useTarget(t), home);
  if (cwd !== null) return { cwd };
  return { why: homeError ? `cannot read the home folder: ${homeError}` : 'Reading the home folder…' };
}

const byName = (a, b) => {
  const x = a.toLowerCase();
  const y = b.toLowerCase();
  if (x !== y) return x < y ? -1 : 1;
  return a < b ? -1 : a > b ? 1 : 0;
};

// The folder rows under a typed path: the listing's folders whose name starts with the fragment
// (case-insensitively), sorted case-insensitively (an older daemon sends files and folders in
// directory order). Hidden ones only with `showHidden` or a fragment starting with `.`.
export function folderRows(entries, fragment, showHidden = false) {
  const f = String(fragment ?? '').toLowerCase();
  const hidden = showHidden || f.startsWith('.');
  return (entries ?? [])
    .filter((e) => e.dir && e.name.toLowerCase().startsWith(f) && (hidden || !e.name.startsWith('.')))
    .map((e) => e.name)
    .sort(byName);
}

// The "▸ use <folder>" row: the folder the text denotes, and whether it is known not to exist.
// "(not found)" needs a whole listing of its parent: the reply for that parent arrived, without an
// error, not cut, with fewer than MAX_LISTED entries, and no folder of that name.
export function useRow(text, listing) {
  const target = useTarget(text);
  const { parent, fragment } = splitPath(text);
  const whole = !!listing && listing.parent === parent && !listing.error && listing.entries !== null
    && !listing.truncated && listing.entries.length < MAX_LISTED;
  const notFound = !!fragment && whole && !listing.entries.some((e) => e.dir && e.name.toLowerCase() === fragment.toLowerCase());
  return { target, notFound };
}

// The highlighted row in [use, ...folders]: the "use" row (0) with an empty fragment or no match,
// else the first matching folder (1), as the TUI does.
export const firstHighlight = (fragment, rows) => (fragment && rows.length ? 1 : 0);

// The text after completing folder `name` of `parent` (Tab, →, Enter or a click on its row).
export const complete = (parent, name) => `${parent}${name}/`;

// Backspace with the caret at the end of a text ending in `/`: the text without its last segment
// (`~/a/b/` → `~/a/`); `~/` becomes the home's parent, absolute (null while the home is unknown);
// `/` stays. null when the text does not end in `/` (an ordinary Backspace).
export function backspaceUp(text, home) {
  if (text === '/') return '/';
  if (!text.endsWith('/')) return null;
  if (text === '~/') {
    if (!home) return null;
    const h = home.replace(/\/+$/, '');
    return h ? h.slice(0, h.lastIndexOf('/') + 1) : '/';
  }
  const t = text.slice(0, -1);
  return t.slice(0, t.lastIndexOf('/') + 1);
}

// The recent folders (`fs.projects`) a non-path text shows: those whose path contains it,
// case-insensitively; all of them for an empty text.
export function recentRows(dirs, text) {
  const q = String(text ?? '').trim().toLowerCase();
  return (dirs ?? []).filter((d) => !q || d.path.toLowerCase().includes(q));
}

// Request numbering for listings: `next()` numbers a request, `fresh(n)` says whether the reply
// to request `n` is still wanted (it answers the latest one), `drop()` makes every one stale
// (the host changed).
export function sequencer() {
  let last = 0;
  return { next: () => ++last, fresh: (n) => n === last, drop: () => { last += 1; } };
}

// A `fs.list` reply as stored. `truncated` here means folders may be missing: the daemon lists
// folders first, so a cut reply whose last entry is a file left no folder out.
export function listingOf(parent, r) {
  const entries = r?.entries ?? [];
  const cut = !!r?.truncated && (!entries.length || entries[entries.length - 1].dir);
  return { parent, entries, truncated: cut, error: null };
}

// The line shown under the field about a listing: its error, or that it was cut, else null.
export function listNote(listing) {
  if (!listing) return null;
  if (listing.error) return listing.error;
  return listing.truncated ? "list cut: type the folder's full name" : null;
}

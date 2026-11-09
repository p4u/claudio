// New-session wizard, pure part. Imports only the fuzzy matcher and the launch options.
import { filter } from './fuzzy.js';
import { flagsAllowed, selectFrom } from './launch.js';

// `decls`: the addons' declarations (addons.list), for the launch options and flags.
export function initialWizard({ hosts, decls = [], recent, focusedHost, kind = 'claude', cwd = '', resume = null }) {
  const names = [...hosts];
  const host = names.includes(focusedHost) ? focusedHost : (names.includes('local') ? 'local' : names[0] ?? 'local');
  return { hosts: names, host, kind, cwd, resume, launch: 'default', flags: [], name: '', prompt: '', recent: recent ?? [], decls };
}

// The request that starts what the wizard holds. `defer`: the checklists a review put off for
// this launch ([{addon, id}], docs/web-protocol.md §5 "Review").
export function spawnRequest(state, size, defer = []) {
  const base = { host: state.host, cwd: state.cwd, rows: size.rows, cols: size.cols };
  const name = String(state.name ?? '').trim();
  if (state.kind === 'shell') return { op: 'session.shell', args: { ...base, ...(name ? { name } : {}) } };
  const flags = flagsAllowed(state.launch, state.decls ?? []) ? state.flags ?? [] : [];
  const args = { ...base, launch: selectFrom(state.launch, flags) };
  if (defer.length) args.defer = defer.map((d) => ({ addon: d.addon, id: d.id }));
  if (state.resume) args.resume = state.resume;
  if (name) args.name = name;
  if (state.prompt && state.prompt.trim() !== '') args.prompt = state.prompt;
  return { op: 'session.spawn', args };
}

// Fuzzy over the directories' paths; a typed absolute path that is not listed comes first.
export function filterDirs(dirs, typed) {
  const q = String(typed ?? '').trim();
  if (!q) return dirs;
  const hits = filter(dirs, q, (d) => d.path);
  if (q.startsWith('/') && !dirs.some((d) => d.path === q)) return [{ path: q, typed: true }, ...hits];
  return hits;
}

// New-session wizard, pure part. Imports only the fuzzy matcher.
import { filter } from './fuzzy.js';

export function initialWizard({ hosts, profiles, recent, focusedHost, kind = 'claude', cwd = '', resume = null }) {
  const names = [...hosts];
  const host = names.includes(focusedHost) ? focusedHost : (names.includes('local') ? 'local' : names[0] ?? 'local');
  return { hosts: names, host, kind, cwd, resume, proxy: 'default', firstParty: false, name: '', prompt: '', skip: [], recent: recent ?? [], profiles };
}

export function proxyOptions(profiles) {
  return [
    { value: 'default', label: `default (${profiles.default ?? 'direct'})` },
    { value: 'direct', label: 'direct (no proxy)' },
    ...(profiles.names ?? []).map((n) => ({ value: `profile:${n}`, label: n })),
  ];
}

export function proxyArg(value) {
  if (value === 'direct') return { kind: 'direct' };
  if (typeof value === 'string' && value.startsWith('profile:')) return { kind: 'profile', name: value.slice(8) };
  return { kind: 'default' };
}

// The profile a choice resolves to, or null for direct.
export function resolvedProfile(value, profiles) {
  const a = proxyArg(value);
  if (a.kind === 'profile') return a.name;
  if (a.kind === 'default') return profiles.default ?? null;
  return null;
}

export function firstPartyAllowed(value, profiles) {
  return resolvedProfile(value, profiles) !== null;
}

export function spawnRequest(state, size) {
  const base = { host: state.host, cwd: state.cwd, rows: size.rows, cols: size.cols };
  const name = String(state.name ?? '').trim();
  if (state.kind === 'shell') return { op: 'session.shell', args: { ...base, ...(name ? { name } : {}) } };
  const args = { ...base, proxy: proxyArg(state.proxy) };
  if (state.firstParty && firstPartyAllowed(state.proxy, state.profiles ?? { names: [], default: null })) args.first_party = true;
  if (state.resume) args.resume = state.resume;
  if (name) args.name = name;
  if (state.prompt && state.prompt.trim() !== '') args.prompt = state.prompt;
  if (state.skip?.length) args.skip_plugins = [...state.skip];
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

// Pure reducers over plain objects (docs/web-protocol.md §6). Imports nothing.
export function emptyModel() {
  return {
    hosts: {}, sessions: {}, order: [],
    layout: { grid: 1, panes: [null], focus: 0 },
    lens: {}, stats: {}, claude: {},
  };
}

const keyed = (list, key) => Object.fromEntries((list ?? []).map((x) => [x[key], x]));
const without = (obj, k) => { const { [k]: _, ...rest } = obj; return rest; };

export function applySnapshot(model, snap) {
  return {
    ...emptyModel(),
    hosts: keyed(snap.hosts, 'name'),
    sessions: keyed(snap.sessions, 'id'),
    order: [...(snap.order ?? [])],
    layout: snap.layout ?? emptyModel().layout,
  };
}

function lensAt(model, sid, seq, patch) {
  const cur = model.lens[sid];
  if (cur && seq < cur.seq) return model;
  return { ...model, lens: { ...model.lens, [sid]: { items: [], formulas: [], ...cur, ...patch, seq } } };
}

export function applyEvent(model, ev) {
  switch (ev.ev) {
    case 'host':
      return { ...model, hosts: { ...model.hosts, [ev.host.name]: ev.host } };
    case 'host_removed':
      return { ...model, hosts: without(model.hosts, ev.name), stats: without(model.stats, ev.name), claude: without(model.claude, ev.name) };
    case 'host_stats':
      return { ...model, stats: { ...model.stats, [ev.host]: { cpu: ev.cpu, mem_used: ev.mem_used, mem_total: ev.mem_total } } };
    case 'session': {
      const s = ev.session;
      let lens = model.lens;
      if (s.state === 'starting' && lens[s.id]) lens = { ...lens, [s.id]: { ...lens[s.id], seq: -1 } };
      return { ...model, sessions: { ...model.sessions, [s.id]: s }, lens };
    }
    case 'session_removed':
      return { ...model, sessions: without(model.sessions, ev.sid), lens: without(model.lens, ev.sid), order: model.order.filter((x) => x !== ev.sid) };
    case 'attached':
      return model.lens[ev.sid] ? { ...model, lens: { ...model.lens, [ev.sid]: { ...model.lens[ev.sid], seq: -1 } } } : model;
    case 'lens':
      return lensAt(model, ev.sid, ev.seq, { items: ev.items ?? [] });
    case 'latex':
      return lensAt(model, ev.sid, ev.seq, { formulas: ev.formulas ?? [] });
    case 'claude': {
      const { ev: _e, t: _t, ...rest } = ev;
      return { ...model, claude: { ...model.claude, [ev.host]: rest } };
    }
    case 'order':
      return { ...model, order: [...ev.sids] };
    case 'layout':
      return { ...model, layout: ev.layout };
    default:
      return model;
  }
}

export function orderedSessions(model) {
  const seen = new Set();
  const out = [];
  for (const id of model.order) {
    const s = model.sessions[id];
    if (s && !seen.has(id)) { seen.add(id); out.push(s); }
  }
  const rest = Object.values(model.sessions).filter((s) => !seen.has(s.id));
  rest.sort((a, b) => (a.created_at - b.created_at) || (a.id < b.id ? -1 : 1));
  return out.concat(rest);
}

export function groupSessions(sessions) {
  const hosts = new Map();
  for (const s of sessions) {
    if (!hosts.has(s.host)) hosts.set(s.host, new Map());
    const g = hosts.get(s.host);
    if (!g.has(s.cwd)) g.set(s.cwd, []);
    g.get(s.cwd).push(s);
  }
  const names = [...hosts.keys()].sort((a, b) => (a === 'local' ? -1 : b === 'local' ? 1 : a < b ? -1 : a > b ? 1 : 0));
  return names.map((host) => ({
    host,
    groups: [...hosts.get(host)].map(([cwd, list]) => ({
      cwd, label: cwd.replace(/\/+$/, '').split('/').pop() || cwd, sessions: list,
    })),
  }));
}

// A repo gets its own sub-group in the rail only when it holds two sessions or more on that host;
// the rest are listed flat. -> { singles: Session[], groups: group[] }
export function railGroups(groups) {
  const singles = [];
  const multi = [];
  for (const g of groups) (g.sessions.length >= 2 ? multi : singles).push(g);
  return { singles: singles.flatMap((g) => g.sessions), groups: multi };
}

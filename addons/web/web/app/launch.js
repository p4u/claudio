// Launch options of the addons (docs/web-protocol.md §4 "Addons", §5 "Review"), pure. Imports
// nothing. A pick is a string: 'default', 'direct' or optionValue(addon, option).

const PREFIX = 'option:';

// The select value of an addon's option. Addon names are [a-z0-9-], so the first ':' after the
// prefix ends the name; the option id may hold anything.
export const optionValue = (addon, option) => `${PREFIX}${addon}:${option}`;

// 'option:<addon>:<option>' -> {addon, option}, else null.
export function parseOption(value) {
  if (typeof value !== 'string' || !value.startsWith(PREFIX)) return null;
  const rest = value.slice(PREFIX.length);
  const at = rest.indexOf(':');
  if (at <= 0 || at === rest.length - 1) return null;
  return { addon: rest.slice(0, at), option: rest.slice(at + 1) };
}

// AddonDecl[] -> [{group, options: [{value, addon, id, label, detail, default}]}]: one entry per
// LaunchOption.group, groups and options in the order the addons list them.
export function optionsByGroup(decls) {
  const groups = [];
  for (const d of decls ?? []) {
    for (const o of d.launch_options ?? []) {
      let g = groups.find((x) => x.group === o.group);
      if (!g) { g = { group: o.group, options: [] }; groups.push(g); }
      g.options.push({ value: optionValue(d.name, o.id), addon: d.name, id: o.id, label: o.label, detail: o.detail ?? null, default: !!o.default });
    }
  }
  return groups;
}

// The option a 'default' pick resolves to (the first one an addon marks default), or null: direct.
export function defaultOption(decls) {
  for (const g of optionsByGroup(decls)) {
    const o = g.options.find((x) => x.default);
    if (o) return o;
  }
  return null;
}

// Every labelled launch flag, once per name: [{name, label}].
export function launchFlags(decls) {
  const out = [];
  for (const d of decls ?? []) {
    for (const f of d.launch_flags ?? []) if (!out.some((x) => x.name === f.name)) out.push({ name: f.name, label: f.label });
  }
  return out;
}

// The entries of one group's select: default, direct, then the group's options.
export function groupChoices(group, decls) {
  const def = defaultOption(decls);
  return [
    { value: 'default', label: `default (${def ? def.label : 'direct'})` },
    { value: 'direct', label: 'direct (none)' },
    ...group.options.map((o) => ({ value: o.value, label: o.label, detail: o.detail })),
  ];
}

// What a group's select shows for `pick`: the pick when it is default, direct or one of the group's
// options, else 'default'.
export function groupValue(group, pick) {
  if (pick === 'default' || pick === 'direct') return pick;
  return group.options.some((o) => o.value === pick) ? pick : 'default';
}

// Flags only go with an addon's option in effect (the server refuses them otherwise).
export function flagsAllowed(pick, decls) {
  if (parseOption(pick)) return true;
  return pick === 'default' && defaultOption(decls) !== null;
}

// A pick and the labelled flags turned on -> LaunchSelect.
export function selectFrom(pick, flags = []) {
  const f = [...new Set(flags ?? [])];
  const o = parseOption(pick);
  if (o) return { pick: 'option', addon: o.addon, option: o.option, flags: f };
  return { pick: pick === 'direct' ? 'direct' : 'default', flags: f };
}

// A Session's `launch` as one short text: 'unknown', 'direct', or the option and its flags.
export function labelFor(launch) {
  if (!launch || !launch.known) return 'unknown';
  const c = launch.choice;
  if (!c) return 'direct';
  const flags = c.flags ?? [];
  return `${c.option}${flags.length ? ` (${flags.join(', ')})` : ''}`;
}

// The review loop of a launch (D11): `spawn(defer)` sends session.spawn with that `defer` list and
// resolves with its reply; a `{review}` reply asks `ask(review)`, which resolves with 'saved'
// (spawn again), 'skip' (spawn again with that checklist deferred) or anything else (cancel:
// resolves null, nothing more is sent). Any other reply is the result.
export async function reviewLoop(spawn, ask, max = 8) {
  const defer = [];
  for (let i = 0; i < max; i++) {
    const r = await spawn(defer.map((d) => ({ ...d })));
    if (!r || !r.review) return r;
    const a = await ask(r.review);
    if (a === 'skip') {
      if (!defer.some((d) => d.addon === r.review.addon && d.id === r.review.id)) defer.push({ addon: r.review.addon, id: r.review.id });
    } else if (a !== 'saved') {
      return null;
    }
  }
  throw new Error('the launch kept asking for a review');
}

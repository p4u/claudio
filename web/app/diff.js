// Unified-diff parsing for the changes and history views. Imports nothing; the text of a patch is
// only ever data here (the UI renders every line as a text node).

const unq = (p) => {
  let s = p.trim();
  if (s.startsWith('"') && s.endsWith('"')) s = s.slice(1, -1);
  return s;
};
const side = (p, prefix) => {
  const s = unq(p.split('\t')[0]);
  if (s === '/dev/null') return null;
  return s.startsWith(prefix) ? s.slice(2) : s;
};

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

// -> {files: [{oldPath, newPath, binary, hunks: [{header, lines: [{kind, text, old, new}]}]}]}
export function parsePatch(text) {
  const files = [];
  let f = null;
  let h = null;
  let o = 0;
  let n = 0;
  const open = () => {
    f = { oldPath: null, newPath: null, binary: false, hunks: [] };
    h = null;
    files.push(f);
  };
  for (const line of String(text ?? '').split('\n')) {
    if (line.startsWith('diff --git ')) {
      open();
      const m = /^diff --git (?:"?a\/(.*?)"?) (?:"?b\/(.*?)"?)$/.exec(line);
      if (m) { f.oldPath = m[1]; f.newPath = m[2]; }
      continue;
    }
    if (h && (line.startsWith(' ') || line.startsWith('+') || line.startsWith('-') || line.startsWith('\\'))) {
      const c = line[0];
      const body = line.slice(1);
      if (c === ' ') h.lines.push({ kind: 'ctx', text: body, old: o++, new: n++ });
      else if (c === '-') h.lines.push({ kind: 'del', text: body, old: o++, new: null });
      else if (c === '+') h.lines.push({ kind: 'add', text: body, old: null, new: n++ });
      else h.lines.push({ kind: 'meta', text: line, old: null, new: null });
      continue;
    }
    const hm = HUNK.exec(line);
    if (hm) {
      if (!f) open();
      o = Number(hm[1]);
      n = Number(hm[2]);
      h = { header: line, lines: [] };
      f.hunks.push(h);
      continue;
    }
    if (line.startsWith('--- ') && (!h || !f)) {
      if (!f || f.hunks.length) open();
      f.oldPath = side(line.slice(4), 'a/');
      continue;
    }
    if (line.startsWith('+++ ') && f && !h) { f.newPath = side(line.slice(4), 'b/'); continue; }
    if (!f) continue;
    if (line.startsWith('rename from ')) f.oldPath = unq(line.slice(12));
    else if (line.startsWith('rename to ')) f.newPath = unq(line.slice(10));
    else if (line.startsWith('Binary files ') || line.startsWith('GIT binary patch')) f.binary = true;
  }
  return { files };
}

// Runs of removals followed by additions are paired row by row; a context or meta line is one row.
export function sideBySide(hunk) {
  const rows = [];
  const lines = hunk.lines;
  for (let i = 0; i < lines.length;) {
    const l = lines[i];
    if (l.kind === 'ctx' || l.kind === 'meta') { rows.push({ left: l, right: l }); i++; continue; }
    const dels = [];
    const adds = [];
    while (i < lines.length && lines[i].kind === 'del') dels.push(lines[i++]);
    while (i < lines.length && lines[i].kind === 'add') adds.push(lines[i++]);
    const len = Math.max(dels.length, adds.length);
    for (let k = 0; k < len; k++) rows.push({ left: dels[k] ?? null, right: adds[k] ?? null });
  }
  return rows;
}

// GitWorktreeStatus files -> groups. U conflicts; ? untracked; index != '.' staged; worktree != '.'
// unstaged (a file can be in both).
export function groupWorktree(files) {
  const g = { conflicts: [], staged: [], unstaged: [], untracked: [] };
  for (const f of files ?? []) {
    if (f.index === 'U' || f.worktree === 'U') g.conflicts.push(f);
    else if (f.index === '?' || f.worktree === '?') g.untracked.push(f);
    else {
      if (f.index !== '.') g.staged.push(f);
      if (f.worktree !== '.') g.unstaged.push(f);
    }
  }
  return g;
}

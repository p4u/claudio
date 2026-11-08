// First thing: record CSP violations so the e2e can assert there are none.
document.addEventListener('securitypolicyviolation', (e) => {
  (window.__claudioCsp ||= []).push(e.violatedDirective);
});

const TOKEN_BAD = "This link's token is not valid. Run `claudio web --print-url`.";
const NO_SESSION = 'Open the URL `claudio web` printed. It carries the sign-in token.';

function show(title, text) {
  const root = document.getElementById('app');
  root.replaceChildren();
  const card = document.createElement('div');
  card.className = 'boot-card';
  const h = document.createElement('h1');
  h.textContent = title;
  card.append(h);
  // The message may hold `code` spans: split on backticks, text nodes only.
  const p = document.createElement('p');
  text.split('`').forEach((part, i) => {
    if (i % 2) { const c = document.createElement('code'); c.textContent = part; p.append(c); } else p.append(part);
  });
  card.append(p);
  const wrap = document.createElement('div');
  wrap.className = 'boot';
  wrap.append(card);
  root.append(wrap);
}

async function start() {
  await import('./main.js');
}

async function boot() {
  const m = /^#t=([A-Za-z0-9._~-]{1,128})$/.exec(location.hash);
  if (location.hash.startsWith('#t=')) {
    const token = m ? m[1] : '';
    let r;
    try {
      r = await fetch('/api/login', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token }), credentials: 'same-origin',
      });
    } catch {
      show('claudio', 'Cannot reach the server.');
      return;
    }
    history.replaceState(null, '', location.pathname);
    if (r.status === 204) return start();
    if (r.status === 429) return show('claudio', 'Too many attempts: wait a minute.');
    return show('claudio', r.status === 401 || !m ? TOKEN_BAD : `Sign-in failed (${r.status}).`);
  }
  let r;
  try { r = await fetch('/api/me', { credentials: 'same-origin' }); } catch { show('claudio', 'Cannot reach the server.'); return; }
  if (r.status === 204) return start();
  show('claudio', NO_SESSION);
}

boot();

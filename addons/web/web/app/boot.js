// First thing: record CSP violations so the e2e can assert there are none.
document.addEventListener('securitypolicyviolation', (e) => {
  (window.__claudioCsp ||= []).push(e.violatedDirective);
});

const TOKEN_BAD = "This link's token is not valid. Run `claudio web --print-url`.";
const TOKEN_ELSEWHERE = 'Token links work only on the machine claudio web runs on. Log in with the password.';
const NO_SESSION = 'Open the URL `claudio web` printed. It carries the sign-in token.';
const LOGIN_FAILED = {
  401: 'Wrong password.',
  429: 'Too many attempts: wait a minute.',
  503: 'The server is busy: try again in a moment.',
};

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

/** The login methods a 401 from /api/me lists ([] for anything else). */
async function methodsOf(r) {
  if (r.status !== 401) return [];
  try {
    const body = await r.json();
    return Array.isArray(body?.login) ? body.login : [];
  } catch { return []; }
}

async function offered() {
  try { return await methodsOf(await fetch('/api/me', { credentials: 'same-origin' })); } catch { return []; }
}

/** The password form (only when the server offers a password). The password is never kept. */
function showLogin(note) {
  const root = document.getElementById('app');
  root.replaceChildren();
  const form = document.createElement('form');
  form.className = 'boot-card login';
  form.setAttribute('data-testid', 'login-form');
  form.noValidate = true;
  const h = document.createElement('h1');
  h.textContent = 'claudio';
  const label = document.createElement('label');
  label.htmlFor = 'login-password';
  label.textContent = 'Password';
  const input = document.createElement('input');
  Object.assign(input, { id: 'login-password', type: 'password', className: 'input', autocomplete: 'current-password' });
  input.setAttribute('data-testid', 'login-password');
  const error = document.createElement('p');
  error.className = 'login-error';
  error.setAttribute('role', 'alert');
  error.setAttribute('data-testid', 'login-error');
  error.textContent = note;
  const button = document.createElement('button');
  button.type = 'submit';
  button.className = 'btn btn-primary';
  button.textContent = 'Log in';
  button.setAttribute('data-testid', 'login-submit');
  form.append(h, label, input, error, button);
  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const password = input.value;
    if (!password) return;
    input.value = ''; // posted once, then the cookie; never kept
    button.disabled = true;
    let r = null;
    try {
      r = await fetch('/api/login', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password }), credentials: 'same-origin',
      });
    } catch { /* r stays null */ }
    if (r?.status === 204) return start();
    error.textContent = r ? (LOGIN_FAILED[r.status] ?? `Sign-in failed (${r.status}).`) : 'Cannot reach the server.';
    button.disabled = false;
    input.focus();
  });
  const wrap = document.createElement('div');
  wrap.className = 'boot';
  wrap.append(form);
  root.append(wrap);
  input.focus();
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
    if (r.status === 401) {
      const methods = await offered();
      if (methods.includes('password') && !methods.includes('token')) return showLogin(TOKEN_ELSEWHERE);
    }
    return show('claudio', r.status === 401 || !m ? TOKEN_BAD : `Sign-in failed (${r.status}).`);
  }
  let r;
  try { r = await fetch('/api/me', { credentials: 'same-origin' }); } catch { show('claudio', 'Cannot reach the server.'); return; }
  if (r.status === 204) return start();
  if ((await methodsOf(r)).includes('password')) return showLogin('');
  show('claudio', NO_SESSION);
}

boot();

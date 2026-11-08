// A fake claude-proxy on loopback: the routes and bodies of tests/web.rs `fake_proxy`.
import http from 'node:http';
import crypto from 'node:crypto';

export const PLUGIN = 'vocdoni-test';
export const PROXY_TOKEN = 'tok-e2e';
export const PLUGIN_SECRET = 'plugin-SECRET-value-51d2e8';

export const MANIFEST = `format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer \${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."
`;

export async function startFakeProxy() {
  const server = http.createServer();
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const addr = `127.0.0.1:${server.address().port}`;
  const sha = crypto.createHash('sha256').update(MANIFEST).digest('hex');
  const plugins = JSON.stringify({ plugins: [{
    name: PLUGIN, version: '1.0.0',
    bundle: { url: `http://${addr}/${PLUGIN}.toml`, sha256: sha },
    env: { VOCDONI_TEST_TOKEN: PLUGIN_SECRET },
  }] });
  const stats = JSON.stringify({
    version: 1, user_name: 'tester', period: '24h',
    totals: { requests: 12, errors: 1, input_tokens: 3400, output_tokens: 5600, cache_read: 0, cache_creation: 0 },
    by_model: [{ model: 'claude-opus-5-5', requests: 12, errors: 1, input_tokens: 3400, output_tokens: 5600, cache_read: 0, cache_creation: 0 }],
    limit: null,
  });
  const routes = {
    '/v1/claudio/config': JSON.stringify({ version: 1, env: {} }),
    '/v1/claudio/plugins': plugins,
    '/v1/claudio/me/stats': stats,
    '/v1/claudio/pool/health': JSON.stringify({ version: 1, providers: [{ name: 'anthropic', status: 'ok' }] }),
  };
  server.on('request', (req, res) => {
    const path = (req.url ?? '').split('?')[0];
    const body = routes[path] ?? (path === `/${PLUGIN}.toml` ? MANIFEST : null);
    res.writeHead(body === null ? 404 : 200, { 'Content-Type': 'application/json', Connection: 'close' });
    res.end(body ?? '{}');
  });
  return { addr, port: server.address().port, stop: () => new Promise((r) => { server.closeAllConnections?.(); server.close(r); }) };
}

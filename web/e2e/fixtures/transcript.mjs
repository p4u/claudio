// Writes a claude conversation transcript the daemon's reader and search find.
import fs from 'node:fs';
import path from 'node:path';

export const FIXTURE_WORD = 'fixtureword';

export const ANSWER = [
  `Here is ${FIXTURE_WORD} with math.`,
  '',
  '<latex>E = mc^2</latex>',
  '',
  'Inline <latex>a^2 + b^2</latex> works too.',
  '',
  '```mermaid',
  'graph TD; A-->B',
  '```',
  '',
  '```rust',
  'fn main() { println!("hi"); }',
  '```',
  '',
  '| a | b |',
  '|---|---|',
  '| 1 | 2 |',
].join('\n');

/** projects/<canonical cwd, every non-alphanumeric -> '-'>/<conversation>.jsonl */
export function writeTranscript(claudeConfig, cwd, conversation) {
  const canon = fs.realpathSync(cwd);
  const dir = path.join(claudeConfig, 'projects', canon.replace(/[^A-Za-z0-9]/g, '-'));
  fs.mkdirSync(dir, { recursive: true });
  const t0 = Date.now() - 60_000;
  const rec = (o) => JSON.stringify({ sessionId: conversation, cwd: canon, ...o });
  const lines = [
    rec({ type: 'user', uuid: 'u1', timestamp: new Date(t0).toISOString(), message: { role: 'user', content: `explain ${FIXTURE_WORD}` } }),
    rec({ type: 'assistant', uuid: 'a1', parentUuid: 'u1', timestamp: new Date(t0 + 5000).toISOString(),
      message: { role: 'assistant', model: 'claude-opus-5-5', content: [{ type: 'text', text: ANSWER }],
        usage: { input_tokens: 10, output_tokens: 20, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 } } }),
  ];
  const file = path.join(dir, `${conversation}.jsonl`);
  fs.writeFileSync(file, lines.join('\n') + '\n');
  return file;
}

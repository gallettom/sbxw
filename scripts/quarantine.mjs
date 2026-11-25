#!/usr/bin/env node
// The quarantine list (tests/quarantine.txt), read for scripts/run-suite.sh.
//
//   node scripts/quarantine.mjs <suite> [--file path] [--patterns]
//
// Prints one line per quarantined test of the suite, "<full test name>" (with
// --patterns: an anchored regular expression matching just that name, for
// node's --test-skip-pattern), and nothing else on stdout. Errors (a line with no ticket reference, an unknown
// suite, a bad date) go to stderr and the exit status is 1: a bad list stops
// the run, so a test cannot be hidden without a ticket.
//
// A line: `suite | full test name | ticket | date`. See tests/quarantine.txt.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const SUITES = ['crates', 'js', 'desktop'];

/// A ticket is a key like `SBXW-123`, an issue number (`#123`), or a URL.
const TICKET = /^(?:[A-Za-z][A-Za-z0-9]*-[0-9]+|#[0-9]+|https?:\/\/\S+)$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;

/// Parse the list's text: `{ entries: [{suite, name, ticket, date, line}], errors: [string] }`.
export function parseQuarantine(text) {
  const entries = [];
  const errors = [];
  String(text).split('\n').forEach((raw, i) => {
    const line = raw.trim();
    if (!line || line.startsWith('#')) return;
    const at = `line ${i + 1}`;
    const f = line.split('|').map((x) => x.trim());
    if (f.length !== 4) {
      errors.push(`${at}: want "suite | test name | ticket | date", got ${f.length} field${f.length === 1 ? '' : 's'}`);
      return;
    }
    const [suite, name, ticket, date] = f;
    const bad = [];
    if (!SUITES.includes(suite)) bad.push(`unknown suite "${suite}" (${SUITES.join(', ')})`);
    if (!name) bad.push('no test name');
    if (!ticket) bad.push('no ticket reference');
    else if (!TICKET.test(ticket)) bad.push(`"${ticket}" is not a ticket reference (SBXW-123, #123 or a URL)`);
    if (!DATE.test(date)) bad.push(`"${date}" is not a date (YYYY-MM-DD)`);
    if (bad.length) errors.push(`${at}: ${bad.join('; ')}`);
    else entries.push({ suite, name, ticket, date, line: i + 1 });
  });
  return { entries, errors };
}

const here = path.dirname(fileURLToPath(import.meta.url));

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const patterns = args.includes('--patterns') && args.splice(args.indexOf('--patterns'), 1).length > 0;
  const fi = args.indexOf('--file');
  const file = fi >= 0 ? args.splice(fi, 2)[1] : path.join(here, '..', 'tests', 'quarantine.txt');
  const suite = args[0];
  if (!SUITES.includes(suite)) {
    console.error(`quarantine: suite must be one of ${SUITES.join(', ')}`);
    process.exit(2);
  }
  let text = '';
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (e) {
    if (e.code !== 'ENOENT') {
      console.error(`quarantine: cannot read ${file}: ${e.message}`);
      process.exit(1);
    }
  }
  const { entries, errors } = parseQuarantine(text);
  if (errors.length) {
    for (const e of errors) console.error(`quarantine: ${file}: ${e}`);
    process.exit(1);
  }
  for (const e of entries) if (e.suite === suite) console.log(patterns ? `^${e.name.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')}$` : e.name);
}

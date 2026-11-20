// Load order of the web UI scripts. Every script in assets/js/singleton.js is
// evaluated, in that order, into ONE context, then main.js's load-time code
// builds the panes. No script may reference a global that only a LATER script
// defines: in a browser that is a ReferenceError at load (pane-files.js once
// called chatAttachTray, defined in chat-core.js, from paneDropMount).
//
// The DOM, window, xterm and fetch are a "deep stub" that answers anything, so
// the check is general: it does not model the page, it only watches which
// names are missing. A script that stops for another reason (a stub too thin)
// is listed in `uncovered` below and does not fail the test; a missing name
// that a later script defines always does. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');

// The list is parsed from singleton.js, never copied here.
const singleton = read('assets/js/singleton.js');
const listSrc = singleton.slice(singleton.indexOf('const SBXW_APP_SCRIPTS = ['));
const scripts = [...listSrc.slice(0, listSrc.indexOf('];')).matchAll(/^\s*'([^']+)',/gm)].map(m => m[1]);
const appScripts = scripts.filter(s => s.startsWith('/js/'));

// Top-level names a script declares (column 0 only, as the scripts are written).
function declared(src) {
  const names = new Set();
  for (const m of src.matchAll(/^(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)/gm)) names.add(m[1]);
  for (const m of src.matchAll(/^(?:const|let|var|class)\s+([A-Za-z_$][\w$]*)/gm)) names.add(m[1]);
  return names;
}

// Answers any property, call or construction; empty when iterated or converted.
function deepStub() {
  const target = function () {};
  const handler = {
    get(_, k) {
      if (k === 'then') return undefined;
      if (k === Symbol.toPrimitive) return () => '';
      if (k === Symbol.iterator) return function* () { for (let i = 0; i < 8; i++) yield stub; };
      if (k === 'length') return 0;
      return stub;
    },
    set() { return true; },
    has() { return true; },
    apply() { return stub; },
    construct() { return stub; },
  };
  const stub = new Proxy(target, handler);
  return stub;
}

test('scripts load in singleton.js order with no use of a later script', async () => {
  assert.ok(appScripts.includes('/js/pane-files.js') && appScripts.includes('/js/chat-core.js'), 'the list was parsed');

  const sources = appScripts.map(s => read(`assets${s}`));
  const defs = sources.map(declared);
  // name -> index of the first script that declares it
  const firstDef = new Map();
  defs.forEach((set, i) => set.forEach(n => { if (!firstDef.has(n)) firstDef.set(n, i); }));

  const errors = [];
  const stub = deepStub();
  const never = new Promise(() => {}); // fetch: the page never gets an answer
  const sandbox = {
    document: stub, window: stub, navigator: stub, localStorage: stub, sessionStorage: stub,
    HTMLDialogElement: stub, HTMLElement: stub, Element: stub, Node: stub, Event: stub, CustomEvent: stub, location: stub, CSS: stub, history: stub, screen: stub, Notification: stub, Audio: stub, Image: stub, URL, URLSearchParams, TextEncoder, TextDecoder, AbortController, Blob: stub, FormData: stub, SpeechSynthesisUtterance: stub, speechSynthesis: stub, matchMedia: stub, getComputedStyle: stub, performance: stub,
    Terminal: stub, FitAddon: stub, CanvasAddon: stub, EventSource: stub, WebSocket: stub,
    ResizeObserver: stub, MutationObserver: stub, IntersectionObserver: stub, BroadcastChannel: stub,
    fetch: () => never, XMLHttpRequest: stub, requestAnimationFrame: () => 0, cancelAnimationFrame() {},
    setTimeout: () => 0, setInterval: () => 0, clearTimeout() {}, clearInterval() {},
    console: { log() {}, warn() {}, info() {}, debug() {}, error: (...a) => errors.push({ kind: 'console.error', text: a.join(' ') }) },
  };
  const ctx = vm.createContext(sandbox);
  // What the daemon bakes into index.html before singleton.js runs (the
  // `__NAME__` placeholders), declared the way the page declares them.
  const page = read('assets/index.html');
  const baked = [...page.matchAll(/^const (\w+)\s*=\s*(?:__\w+__|'__host__');/gm)].map(m => m[1]);
  assert.ok(baked.includes('SBXW_VERSION') && baked.includes('TERM_COLOR_CONFIG'), 'the page constants were found');
  vm.runInContext([...page.matchAll(/^const \w+\s*=\s*(?:__\w+__|'__host__');$/gm)]
    .map(m => m[0].replace(/= __\w+__;/, "= '';")).join('\n'), ctx);
  const seen = [];
  const onReject = reason => seen.push(reason);
  process.on('unhandledRejection', onReject);

  const violations = [];
  const uncovered = [];
  const refersToLater = (err, i) => {
    const m = err?.name === 'ReferenceError' && /^(\S+) is not defined$/.exec(err.message);
    const j = m ? firstDef.get(m[1]) : undefined;
    return m && j !== undefined && j > i ? `${appScripts[i]} uses ${m[1]}, defined only by ${appScripts[j]}` : null;
  };
  try {
    for (let i = 0; i < appScripts.length; i++) {
      const before = seen.length;
      try {
        vm.runInContext(sources[i], ctx, { filename: appScripts[i] });
      } catch (err) {
        const v = refersToLater(err, i);
        if (v) violations.push(v); else uncovered.push(`${appScripts[i]}: ${err.message}`);
      }
      // Load-time code that continues after an await or runs in a promise
      // (main.js lays out the panes there) settles before the next script.
      await new Promise(r => setImmediate(r));
      for (const reason of seen.splice(before)) {
        const v = refersToLater(reason, i);
        if (v) violations.push(v); else uncovered.push(`${appScripts[i]} (async): ${reason?.message ?? reason}`);
      }
    }
  } finally {
    process.off('unhandledRejection', onReject);
  }

  assert.deepEqual(violations, [], 'a script uses a global defined only by a later script');
  assert.deepEqual(errors.filter(e => /pane/.test(e.text)), [], 'a pane feature reported a failure at load');
  // main.js built at least one pane before the page awaited its first answer.
  assert.ok(vm.runInContext('panes.length', ctx) >= 1, 'a pane was built at load');
  // Not a failure: scripts the stub could not run to the end. Printed for the reader.
  if (uncovered.length) console.log('load-order: not run to the end (stub too thin):\n  ' + uncovered.join('\n  '));
});

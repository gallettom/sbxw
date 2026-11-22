// A small stand-in DOM for the phone page (assets/remote), enough to run
// remote-core.js and remote.js in node: index.html parsed, elements, text,
// events, a fake clock for timers, a stub fetch and EventSource, a
// ResizeObserver, and a crude block layout so the page's reads of
// scrollTop, scrollHeight and getBoundingClientRect have answers.
//
// It counts what costs on a phone: elements created, nodes inserted and
// removed, innerHTML parses, and "forced layouts" (a layout read after a
// write, which the browser must answer by laying the page out again).
// The layout itself walks the whole tree, so its time grows with the page
// as a browser's does (much more cheaply). Not a browser: the numbers
// compare one version of the page with another, they are not iPhone times.
//
// Used by tests/js/remote-perf.mjs (the timing harness) and
// tests/js/remote-render.test.mjs (what renders, and what is touched).
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);
const ENT = { amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'", apos: "'", times: '×', nbsp: ' ', hellip: '…', middot: '·', rarr: '→', larr: '←' };
const decode = s => s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+\d*);/gi, (m, e) => {
  if (e[0] === '#') return String.fromCodePoint(e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10));
  return ENT[e] ?? m;
});

export function makeDom(html, opts = {}) {
  const counters = { created: 0, inserted: 0, removed: 0, innerHTML: 0, layouts: 0, layoutReads: 0, roCalls: 0, fetches: [], listeners: 0 };
  const clock = { now: opts.now ?? 1763650000000, timers: new Map(), seq: 0 };
  let dirty = true;
  const touch = () => { dirty = true; };
  let doc = null;

  class Node {
    constructor() { this.parentNode = null; this.childNodes = []; this._ls = null; }
    get parentElement() { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; }
    get firstChild() { return this.childNodes[0] || null; }
    get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
    get nextSibling() { const p = this.parentNode; if (!p) return null; return p.childNodes[p.childNodes.indexOf(this) + 1] || null; }
    get previousSibling() { const p = this.parentNode; if (!p) return null; return p.childNodes[p.childNodes.indexOf(this) - 1] || null; }
    get ownerDocument() { return doc; }
    get isConnected() { let n = this; while (n.parentNode) n = n.parentNode; return n === doc; }
    remove() {
      const p = this.parentNode;
      if (!p) return;
      p.childNodes.splice(p.childNodes.indexOf(this), 1);
      this.parentNode = null;
      counters.removed++;
      touch();
    }
    _adopt(list) {
      const out = [];
      for (const n of list) {
        if (n == null) continue;
        if (n.nodeType === 11) { out.push(...n.childNodes); for (const c of n.childNodes) c.parentNode = null; n.childNodes = []; continue; }
        out.push(typeof n === 'string' ? new Text(n) : n);
      }
      for (const n of out) if (n.parentNode) n.remove();
      return out;
    }
    append(...k) {
      const nodes = this._adopt(k);
      for (const n of nodes) { n.parentNode = this; this.childNodes.push(n); counters.inserted++; }
      touch();
    }
    appendChild(n) { this.append(n); return n; }
    prepend(...k) {
      const nodes = this._adopt(k);
      for (const n of nodes) { n.parentNode = this; counters.inserted++; }
      this.childNodes.unshift(...nodes);
      touch();
    }
    insertBefore(n, ref) {
      if (!ref) return this.appendChild(n);
      const [x] = this._adopt([n]);
      x.parentNode = this;
      this.childNodes.splice(this.childNodes.indexOf(ref), 0, x);
      counters.inserted++;
      touch();
      return x;
    }
    removeChild(n) { n.remove(); return n; }
    replaceChildren(...k) {
      const nodes = this._adopt(k);
      for (const c of this.childNodes) { c.parentNode = null; counters.removed++; }
      this.childNodes = [];
      for (const n of nodes) { n.parentNode = this; this.childNodes.push(n); counters.inserted++; }
      touch();
    }
    replaceWith(...k) {
      const p = this.parentNode;
      if (!p) return;
      const nodes = this._adopt(k.filter(n => n !== this));
      const i = p.childNodes.indexOf(this);
      p.childNodes.splice(i, 1, ...nodes);
      for (const n of nodes) { n.parentNode = p; counters.inserted++; }
      this.parentNode = null;
      counters.removed++;
      touch();
    }
    before(...k) { const p = this.parentNode; if (!p) return; for (const n of this._adopt(k)) p.insertBefore(n, this); }
    after(...k) { const p = this.parentNode; if (!p) return; const nx = this.nextSibling; for (const n of this._adopt(k)) p.insertBefore(n, nx); }
    contains(n) { while (n) { if (n === this) return true; n = n.parentNode; } return false; }
    addEventListener(type, fn, o) {
      if (!fn) return;
      this._ls ||= new Map();
      const l = this._ls.get(type) || [];
      if (!l.some(x => x.fn === fn)) { l.push({ fn, once: !!(o && o.once) }); counters.listeners++; }
      this._ls.set(type, l);
    }
    removeEventListener(type, fn) {
      const l = this._ls?.get(type);
      if (!l) return;
      const i = l.findIndex(x => x.fn === fn);
      if (i >= 0) { l.splice(i, 1); counters.listeners--; }
    }
    dispatchEvent(ev) {
      if (!ev.target) ev.target = this;
      let n = this;
      const path = [];
      while (n) { path.push(n); n = n.parentNode || (n === doc ? win : null); }
      for (const node of path) {
        ev.currentTarget = node;
        for (const l of [...(node._ls?.get(ev.type) || [])]) {
          if (l.once) node.removeEventListener(ev.type, l.fn);
          l.fn.call(node, ev);
          if (ev._stopNow) break;
        }
        if (!ev.bubbles || ev._stop) break;
      }
      return !ev.defaultPrevented;
    }
    get textContent() {
      if (this.nodeType === 3) return this.data;
      let s = '';
      const walk = n => { for (const c of n.childNodes) { if (c.nodeType === 3) s += c.data; else if (c.nodeType === 1) walk(c); } };
      walk(this);
      return s;
    }
    set textContent(v) {
      if (this.nodeType === 3) { this.data = String(v); touch(); return; }
      const s = v == null ? '' : String(v);
      this.replaceChildren(...(s ? [new Text(s)] : []));
    }
  }
  class Text extends Node {
    constructor(s) { super(); this.nodeType = 3; this.data = String(s); this.nodeName = '#text'; }
    get nodeValue() { return this.data; }
  }
  class Fragment extends Node {
    constructor() { super(); this.nodeType = 11; }
  }

  const camel = s => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  const kebab = s => s.replace(/[A-Z]/g, c => `-${c.toLowerCase()}`);
  // Properties that reflect an attribute of the same name.
  const REFLECT = ['id', 'type', 'title', 'alt', 'name', 'placeholder', 'role', 'href', 'target', 'rel', 'lang', 'for'];
  class Element extends Node {
    constructor(tag, ns) {
      super();
      this.nodeType = 1;
      this.ns = ns || null;
      this.tagName = ns ? tag : tag.toUpperCase();
      this.nodeName = this.tagName;
      this.attrs = new Map();
      this.style = makeStyle();
      this._sc = { top: 0 };
      this.value = '';
      counters.created++;
      const el = this;
      this.dataset = new Proxy({}, {
        get: (_, k) => (typeof k === 'string' ? el.attrs.get(`data-${kebab(k)}`) : undefined),
        set: (_, k, v) => { el.setAttribute(`data-${kebab(k)}`, v); return true; },
        deleteProperty: (_, k) => { el.removeAttribute(`data-${kebab(k)}`); return true; },
        has: (_, k) => el.attrs.has(`data-${kebab(k)}`),
        ownKeys: () => [...el.attrs.keys()].filter(a => a.startsWith('data-')).map(a => camel(a.slice(5))),
        getOwnPropertyDescriptor: (_, k) => (el.attrs.has(`data-${kebab(k)}`) ? { enumerable: true, configurable: true, value: el.attrs.get(`data-${kebab(k)}`) } : undefined),
      });
    }
    get localName() { return this.tagName.toLowerCase(); }
    get children() { return this.childNodes.filter(c => c.nodeType === 1); }
    get firstElementChild() { return this.children[0] || null; }
    get lastElementChild() { const c = this.children; return c[c.length - 1] || null; }
    get childElementCount() { return this.children.length; }
    setAttribute(k, v) { this.attrs.set(String(k).toLowerCase(), String(v)); touch(); }
    getAttribute(k) { const v = this.attrs.get(String(k).toLowerCase()); return v === undefined ? null : v; }
    hasAttribute(k) { return this.attrs.has(String(k).toLowerCase()); }
    removeAttribute(k) { if (this.attrs.delete(String(k).toLowerCase())) touch(); }
    toggleAttribute(k, on) { const has = this.hasAttribute(k); const want = on === undefined ? !has : !!on; if (want) this.setAttribute(k, ''); else this.removeAttribute(k); return want; }
    get attributes() { return [...this.attrs].map(([name, value]) => ({ name, value })); }
    get className() { return this.attrs.get('class') || ''; }
    set className(v) { this.setAttribute('class', v); }
    get classList() {
      const el = this;
      const get = () => el.className.split(/\s+/).filter(Boolean);
      const set = l => el.setAttribute('class', l.join(' '));
      return {
        add: (...c) => { const l = get(); for (const x of c) if (!l.includes(x)) l.push(x); set(l); },
        remove: (...c) => { set(get().filter(x => !c.includes(x))); },
        contains: c => get().includes(c),
        toggle: (c, on) => { const has = get().includes(c); const want = on === undefined ? !has : !!on; if (want && !has) set([...get(), c]); if (!want && has) set(get().filter(x => x !== c)); return want; },
        replace: (a, b) => { set(get().map(x => (x === a ? b : x))); },
        get length() { return get().length; },
      };
    }
    get hidden() { return this.attrs.has('hidden'); }
    set hidden(v) { if (v) { if (!this.attrs.has('hidden')) this.setAttribute('hidden', ''); } else this.removeAttribute('hidden'); }
    get disabled() { return this.attrs.has('disabled'); }
    set disabled(v) { if (v) { if (!this.attrs.has('disabled')) this.setAttribute('disabled', ''); } else this.removeAttribute('disabled'); }
    get checked() { return !!this._checked; }
    set checked(v) { this._checked = !!v; }
    get innerHTML() { return this.childNodes.map(serialize).join(''); }
    set innerHTML(v) {
      counters.innerHTML++;
      const frag = parse(String(v));
      this.replaceChildren(...frag.childNodes);
    }
    get outerHTML() { return serialize(this); }
    get options() { return this.tagName === 'SELECT' ? this.querySelectorAll('option') : undefined; }
    add(o) { this.append(o); }
    get selectedIndex() { return this.options ? this.options.findIndex(o => o.value === this.value) : -1; }
    closest(sel) { let n = this; while (n && n.nodeType === 1) { if (matches(n, sel)) return n; n = n.parentNode; } return null; }
    matches(sel) { return matches(this, sel); }
    querySelectorAll(sel) { const out = []; const walk = n => { for (const c of n.childNodes) if (c.nodeType === 1) { if (matches(c, sel)) out.push(c); walk(c); } }; walk(this); return out; }
    querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
    getElementsByTagName(t) { return this.querySelectorAll(t); }
    focus() { doc.activeElement = this; }
    blur() { if (doc.activeElement === this) doc.activeElement = doc.body; }
    click() { this.dispatchEvent(new Event('click', { bubbles: true })); }
    setSelectionRange() {}
    get selectionStart() { return String(this.value).length; }
    get selectionEnd() { return String(this.value).length; }
    reset() {}
    submit() {}
    scrollIntoView() {}
    setPointerCapture() {}
    releasePointerCapture() {}
    hasPointerCapture() { return false; }
    getContext() { return null; }
    animate() { return { finished: Promise.resolve(), cancel() {} }; }
    // ── Layout reads: each one after a write lays the page out again ──
    getBoundingClientRect() {
      const b = box(this);
      let top = b.top;
      for (let p = this.parentNode; p && p.nodeType === 1; p = p.parentNode) if (p._scroller) top -= p._sc.top;
      return { top, bottom: top + b.h, left: 0, right: 390, width: 390, height: b.h, x: 0, y: top };
    }
    getClientRects() { return [this.getBoundingClientRect()]; }
    get offsetWidth() { box(this); return this.hidden ? 0 : 390; }
    get offsetHeight() { return box(this).h; }
    get clientWidth() { box(this); return 390; }
    get clientHeight() { const b = box(this); return this._scroller ? this._scroller : b.h; }
    get scrollHeight() { const b = box(this); return this._scroller ? Math.max(b.content, this._scroller) : b.h; }
    get scrollTop() { box(this); return this._sc.top; }
    set scrollTop(v) {
      box(this);
      const max = this._scroller ? Math.max(0, layoutOf(this).content - this._scroller) : 0;
      const top = Math.max(0, Math.min(max, Number(v) || 0));
      // A scroll moves what is drawn, it does not lay the page out again.
      if (top !== this._sc.top) { this._sc.top = top; queueScroll(this); }
    }
    scrollTo(o) { this.scrollTop = typeof o === 'object' ? o.top : arguments[1]; }
  }
  for (const k of REFLECT) {
    if (k === 'id') {
      Object.defineProperty(Element.prototype, 'id', { get() { return this.attrs.get('id') || ''; }, set(v) { this.setAttribute('id', v); } });
      continue;
    }
    const prop = k === 'for' ? 'htmlFor' : k;
    Object.defineProperty(Element.prototype, prop, { get() { return this.attrs.get(k) ?? ''; }, set(v) { this.setAttribute(k, v); }, configurable: true });
  }
  // `src` and the image/link properties are plain properties plus the attribute.
  for (const k of ['src', 'loading', 'decoding', 'referrerPolicy', 'draggable', 'tabIndex']) {
    Object.defineProperty(Element.prototype, k, { get() { return this.attrs.get(k.toLowerCase()) ?? ''; }, set(v) { this.setAttribute(k.toLowerCase(), v); }, configurable: true });
  }

  function makeStyle() {
    const s = { _p: {} };
    s.setProperty = (k, v) => { if (s._p[k] !== String(v)) { s._p[k] = String(v); touch(); counters.styleSets = (counters.styleSets || 0) + 1; } };
    s.removeProperty = k => { delete s._p[k]; touch(); };
    s.getPropertyValue = k => s._p[k] || '';
    return s;
  }

  // ── Selectors: compound simple selectors only (tag, #id, .class,
  // [attr], [attr="v"], :not(simple)), and lists with commas.
  function matches(el, sel) {
    if (el.nodeType !== 1) return false;
    return sel.split(',').some(s => compound(el, s.trim()));
  }
  function compound(el, s) {
    if (s === '*') return true;
    const re = /(:not\(([^)]*)\))|(#[\w-]+)|(\.[\w-]+)|(\[([\w-]+)(?:="([^"]*)")?\])|(^[\w-]+)/g;
    let m;
    let used = 0;
    while ((m = re.exec(s))) {
      used += m[0].length;
      if (m[1]) { if (compound(el, m[2].trim())) return false; }
      else if (m[3]) { if (el.id !== m[3].slice(1)) return false; }
      else if (m[4]) { if (!el.className.split(/\s+/).includes(m[4].slice(1))) return false; }
      else if (m[5]) { const v = el.getAttribute(m[6]); if (v === null || (m[7] !== undefined && v !== m[7])) return false; }
      else if (m[8]) { if (el.tagName.toLowerCase() !== m[8].toLowerCase()) return false; }
    }
    if (used !== s.length) throw new Error(`selector not supported by the stand-in DOM: ${s}`);
    return true;
  }

  // ── HTML: a fragment parser for index.html and the Markdown subset ──
  function parse(src) {
    const root = new Fragment();
    const stack = [root];
    const top = () => stack[stack.length - 1];
    const re = /<!--[\s\S]*?-->|<!doctype[^>]*>|<\/([a-zA-Z][\w-]*)\s*>|<([a-zA-Z][\w-]*)((?:\s+[^\s=>/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*(\/?)>|([^<]+|<)/gi;
    let m;
    while ((m = re.exec(src))) {
      if (m[1]) {
        const t = m[1].toLowerCase();
        for (let i = stack.length - 1; i > 0; i -= 1) {
          if (stack[i].tagName.toLowerCase() === t) { stack.length = i; break; }
        }
      } else if (m[2]) {
        const t = m[2];
        const svg = t.toLowerCase() === 'svg' || stack.some(n => n.ns);
        const el = new Element(svg ? t : t.toLowerCase(), svg ? 'svg' : null);
        const ar = /([^\s=>/]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
        let a;
        while ((a = ar.exec(m[3] || ''))) el.attrs.set(a[1].toLowerCase(), decode(a[2] ?? a[3] ?? a[4] ?? ''));
        if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') el.value = el.attrs.get('value') || '';
        top().childNodes.push(el);
        el.parentNode = top();
        if (!VOID.has(t.toLowerCase()) && !m[4]) stack.push(el);
      } else if (m[5] !== undefined) {
        const tx = new Text(decode(m[5]));
        top().childNodes.push(tx);
        tx.parentNode = top();
      }
    }
    touch();
    return root;
  }
  const escT = s => s.replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
  const escA = s => s.replace(/[&"]/g, c => ({ '&': '&amp;', '"': '&quot;' }[c]));
  function serialize(n) {
    if (n.nodeType === 3) return escT(n.data);
    if (n.nodeType !== 1) return n.childNodes.map(serialize).join('');
    const t = n.tagName.toLowerCase();
    const a = [...n.attrs].map(([k, v]) => (v === '' ? ` ${k}` : ` ${k}="${escA(v)}"`)).join('');
    if (VOID.has(t)) return `<${t}${a}>`;
    return `<${t}${a}>${n.childNodes.map(serialize).join('')}</${t}>`;
  }

  // ── A crude block layout: every element stacks its children; a text is
  // lines of 40 characters, 20 px each; an image 120 px; a hidden element
  // nothing. #log scrolls (clientHeight 600). Laid out again only after a
  // write, and only when something reads it.
  const SCROLLERS = { log: 600 };
  let lay = new Map();
  function layout() {
    counters.layouts++;
    lay = new Map();
    const walk = (n, top) => {
      if (n.nodeType === 3) return Math.ceil(n.data.length / 40) * 20;
      if (n.nodeType !== 1) { let h = 0; for (const c of n.childNodes) h += walk(c, top + h); return h; }
      if (n.hidden || n.tagName === 'SCRIPT' || n.tagName === 'HEAD' || n.tagName === 'svg' || n.tagName === 'TEMPLATE') { lay.set(n, { top, h: 0, content: 0 }); return 0; }
      const pad = /\brc-msg\b/.test(n.className) ? 16 : 0;
      let h = pad;
      if (n.tagName === 'IMG') h = 120;
      else for (const c of n.childNodes) h += walk(c, top + h);
      const content = h;
      if (n._scroller) h = n._scroller;
      lay.set(n, { top, h, content });
      return h;
    };
    walk(doc.documentElement, 0);
    dirty = false;
  }
  function layoutOf(el) { if (dirty) layout(); return lay.get(el) || { top: 0, h: 0, content: 0 }; }
  function box(el) { counters.layoutReads++; return layoutOf(el); }

  // ── Timers on a fake clock ──
  function addTimer(fn, ms, args, every) {
    const id = ++clock.seq;
    clock.timers.set(id, { at: clock.now + Math.max(0, Number(ms) || 0), fn, args, every: every ? Math.max(1, Number(ms) || 0) : 0 });
    return id;
  }
  let rafs = [];
  const scrolls = new Set();
  function queueScroll(el) { scrolls.add(el); }

  // ── ResizeObserver: after a layout, each observed element whose size
  // changed gets one call, as in a browser's rendering step.
  const observers = [];
  class ResizeObserver {
    constructor(cb) { this.cb = cb; this.els = new Map(); observers.push(this); }
    observe(el) { this.els.set(el, null); }
    unobserve(el) { this.els.delete(el); }
    disconnect() { this.els.clear(); }
  }

  class Event {
    constructor(type, o = {}) { this.type = type; this.bubbles = !!o.bubbles; this.defaultPrevented = false; this.target = null; this.detail = o.detail; }
    preventDefault() { this.defaultPrevented = true; }
    stopPropagation() { this._stop = true; }
    stopImmediatePropagation() { this._stop = true; this._stopNow = true; }
  }

  const sources = [];
  class EventSource {
    constructor(url) { this.url = url; this.readyState = 0; this._ls = null; sources.push(this); }
    close() { this.readyState = 2; }
  }
  EventSource.prototype.addEventListener = Node.prototype.addEventListener;
  EventSource.prototype.removeEventListener = Node.prototype.removeEventListener;
  EventSource.prototype.dispatchEvent = Node.prototype.dispatchEvent;

  const store = new Map();
  const win = {
    console: opts.console || console,
    URL, URLSearchParams, AbortController, Blob, TextEncoder, TextDecoder, Promise, Map, Set, JSON, Math, Number, String, Array, Object, Error, RegExp, Symbol, Proxy, Reflect, Intl,
    ArrayBuffer, Uint8Array, Float32Array, DataView,
    performance: { now: () => clock.now },
    crypto: globalThis.crypto,
    Event, CustomEvent: Event, EventSource, ResizeObserver,
    setTimeout: (fn, ms, ...a) => addTimer(fn, ms, a, false),
    setInterval: (fn, ms, ...a) => addTimer(fn, ms, a, true),
    clearTimeout: id => { clock.timers.delete(id); },
    clearInterval: id => { clock.timers.delete(id); },
    requestAnimationFrame: fn => { rafs.push(fn); return rafs.length; },
    cancelAnimationFrame: () => {},
    queueMicrotask,
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {}, addListener() {} }),
    location: { hash: '', pathname: '/', search: '', href: 'https://mac.local/', origin: 'https://mac.local', reload() {} },
    history: { pushState() {}, replaceState() {}, back() {} },
    navigator: { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)', platform: 'iPhone', language: 'en', languages: ['en'] },
    localStorage: { getItem: k => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k) },
    sessionStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    CSS: { supports: () => true },
    visualViewport: { height: 844, width: 390, offsetTop: 0, _ls: null },
    innerHeight: 844, innerWidth: 390, devicePixelRatio: 3,
    isSecureContext: true,
    Option: function Option(text = '', value, defSel, sel) { const o = new Element('option'); o.textContent = text; o.value = value === undefined ? text : String(value); if (sel) o.selected = true; return o; },
    XMLHttpRequest: class { open() {} send() {} setRequestHeader() {} abort() {} addEventListener() {} get upload() { return { addEventListener() {} }; } },
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    fetch: async (path, init = {}) => {
      counters.fetches.push(`${init.method || 'GET'} ${path}`);
      const r = await (win.__route ? win.__route(path, init) : { status: 404, body: {} });
      if (r === undefined) return new Promise(() => {}); // never answered
      return { ok: r.status >= 200 && r.status < 300, status: r.status, json: async () => r.body };
    },
  };
  win.visualViewport.addEventListener = Node.prototype.addEventListener;
  win.visualViewport.dispatchEvent = Node.prototype.dispatchEvent;
  win.addEventListener = Node.prototype.addEventListener;
  win.removeEventListener = Node.prototype.removeEventListener;
  win.dispatchEvent = Node.prototype.dispatchEvent;
  win.window = win;
  win.self = win;
  win.globalThis = win;
  class FakeDate extends Date {
    constructor(...a) { super(...(a.length ? a : [clock.now])); }
    static now() { return clock.now; }
  }
  win.Date = FakeDate;

  // The document.
  doc = new Fragment();
  doc.nodeType = 9;
  const tree = parse(html);
  doc.childNodes = tree.childNodes;
  for (const c of doc.childNodes) c.parentNode = doc;
  doc.documentElement = doc.childNodes.find(c => c.nodeType === 1 && c.tagName === 'HTML');
  doc.body = doc.documentElement.querySelector('body');
  doc.activeElement = doc.body;
  doc.visibilityState = 'visible';
  doc.hidden = false;
  doc.getElementById = id => {
    const walk = n => { for (const c of n.childNodes) if (c.nodeType === 1) { if (c.attrs.get('id') === id) return c; const f = walk(c); if (f) return f; } return null; };
    return walk(doc);
  };
  doc.createElement = t => new Element(String(t).toLowerCase());
  doc.createElementNS = (ns, t) => new Element(t, 'svg');
  doc.createTextNode = s => new Text(s);
  doc.createDocumentFragment = () => new Fragment();
  doc.querySelectorAll = sel => doc.documentElement.querySelectorAll(sel);
  doc.querySelector = sel => doc.documentElement.querySelector(sel);
  doc.addEventListener = Node.prototype.addEventListener;
  doc.removeEventListener = Node.prototype.removeEventListener;
  doc.dispatchEvent = Node.prototype.dispatchEvent;
  doc.hasFocus = () => true;
  win.document = doc;
  for (const [id, h] of Object.entries(SCROLLERS)) { const el = doc.getElementById(id); if (el) el._scroller = h; }

  // One rendering step: animation frames, a layout, scroll events, then the
  // ResizeObserver calls (again while sizes keep changing, 8 rounds at most).
  function frame() {
    const list = rafs;
    rafs = [];
    for (const fn of list) fn(clock.now);
    for (let round = 0; round < 8; round += 1) {
      for (const el of [...scrolls]) { scrolls.delete(el); el.dispatchEvent(new Event('scroll')); }
      let called = false;
      for (const ro of observers) {
        const changed = [];
        for (const [el, was] of ro.els) {
          const b = layoutOf(el);
          const size = `${b.h}/${b.content}`;
          if (size !== was) { ro.els.set(el, size); changed.push({ target: el }); }
        }
        if (changed.length) { counters.roCalls++; called = true; ro.cb(changed, ro); }
      }
      if (!called && !scrolls.size) break;
    }
  }
  // Runs every timer due within `ms`, in order, with the promises they start.
  async function advance(ms) {
    const end = clock.now + ms;
    for (;;) {
      await settle();
      let next = null;
      for (const [id, t] of clock.timers) if (t.at <= end && (!next || t.at < next[1].at)) next = [id, t];
      if (!next) break;
      const [id, t] = next;
      clock.now = Math.max(clock.now, t.at);
      if (t.every) t.at += t.every; else clock.timers.delete(id);
      t.fn(...t.args);
      frame();
    }
    clock.now = end;
    // Promises started by a frame (a scroll event's read) settle, then draw.
    for (let i = 0; i < 3; i += 1) { await settle(); frame(); }
  }
  async function settle() {
    for (let i = 0; i < 30; i += 1) await new Promise(r => setImmediate(r));
  }
  function load(files) {
    const ctx = vm.createContext(win);
    for (const f of files) vm.runInContext(readFileSync(f, 'utf8'), ctx, { filename: String(f) });
    return ctx;
  }
  function emit(type, data) {
    const es = sources[sources.length - 1];
    if (!es || es.readyState === 2) return false;
    if (type === 'open') es.readyState = 1;
    const ev = new Event(type);
    ev.data = typeof data === 'string' ? data : JSON.stringify(data);
    es.dispatchEvent(ev);
    return true;
  }
  function setVisible(on) {
    doc.visibilityState = on ? 'visible' : 'hidden';
    doc.hidden = !on;
    doc.dispatchEvent(new Event('visibilitychange'));
  }
  return { window: win, document: doc, counters, clock, frame, advance, settle, load, emit, sources, setVisible, serialize, Event };
}

// The page, booted and paired, with `route(path, init)` answering its
// requests ({ status, body }, or undefined: never answered).
export async function bootPage(route, opts = {}) {
  const root = new URL('../../assets/remote/', import.meta.url);
  const html = readFileSync(new URL('index.html', root), 'utf8');
  const dom = makeDom(html, opts);
  dom.window.__route = route;
  dom.load([opts.core || new URL('remote-core.js', root), opts.page || new URL('remote.js', root)]);
  await dom.advance(0);
  dom.emit('open');
  await dom.advance(0);
  return dom;
}

// ── Code map viewer ───────────────────────────────────────────────────────
//
// Reads the markdown knowledge graph a `/codemap` run leaves under
// `.sbxw-artifacts/codemap/` (see assets/codemap/) and makes it navigable the
// way Obsidian makes a vault navigable: `[[wiki links]]` are clickable,
// backlinks are listed, and the whole thing can be seen at once as a graph.
//
// Everything here is front-end only. The two endpoints the Files panel already
// uses are enough — `/artifacts` to list, `/artifacts/download` to fetch a
// file's bytes (the attachment disposition doesn't bother `fetch`) — so this
// pane needs no new server route and no access beyond `.sbxw-artifacts`.
//
// No markdown library: the map is written in a narrow, checker-enforced subset
// (headings, paragraphs, lists, fences, links), and this file ships inside the
// sbxw binary, which carries no third-party JS of its own.

(() => {
  'use strict';

  /* ── Pure core ─────────────────────────────────────────────────────────
     Parsing, link resolution and rendering are kept free of the DOM so they
     can be unit-tested under node, where there is no browser to click in. */

  const SOURCE_EXTS = ['.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.py', '.rs', '.go',
    '.c', '.h', '.java', '.kt', '.rb', '.php', '.swift', '.cs', '.scala', '.sh'];

  const isCodeTarget = (target) => {
    const head = target.split('#')[0];
    const dot = head.lastIndexOf('.');
    return dot !== -1 && SOURCE_EXTS.includes(head.slice(dot).toLowerCase());
  };

  const escapeHtml = (s) => s
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');

  const slug = (parts) => parts.join('--').toLowerCase()
    .replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'section';

  // Blank out fenced blocks only. Inline code is left in place: a heading may
  // legitimately contain a `code span`, and blanking it renames the section —
  // along with every link that addresses it.
  const maskFences = (text) => {
    let fence = null;
    return text.split('\n').map((line) => {
      const m = line.match(/^\s{0,3}(```+|~~~+)/);
      if (fence) {
        if (m && line.trimStart().startsWith(fence)) fence = null;
        return '';
      }
      if (m) { fence = m[1]; return ''; }
      return line;
    });
  };

  // Links and inline code, matched in one pass: whichever starts first wins.
  // That single rule separates `[[refs]]` — a quoted example the code span
  // swallows — from [[a#`b` c]], a real link whose heading contains code.
  const LINK_OR_CODE = /\[\[[^\]]+\]\]|`+[^`]*`+/g;
  const linksIn = (line) => {
    const found = [];
    for (const m of line.matchAll(LINK_OR_CODE)) {
      if (m[0].startsWith('[[')) found.push(m[0].slice(2, -2));
    }
    return found;
  };

  const stripFrontmatter = (text) => {
    if (!text.startsWith('---\n')) return { body: text, offset: 0 };
    const close = text.indexOf('\n---', 3);
    if (close === -1) return { body: text, offset: 0 };
    const after = text.indexOf('\n', close + 1) + 1;
    return { body: text.slice(after), offset: text.slice(0, after).split('\n').length - 1 };
  };

  // One document per markdown file: its heading tree flattened, plus every
  // link it makes, with the line each was found on.
  function parseDoc(key, path, text) {
    const { body, offset } = stripFrontmatter(text);
    const raw = body.split('\n');
    const sections = [];
    const links = [];
    const stack = [];
    // The fence mask says whether a line is a heading; the title itself is read
    // from the raw line, so a `code span` in it survives verbatim.
    maskFences(body).forEach((line, i) => {
      const h = /^\s{0,3}#{1,6}(\s|$)/.test(line)
        ? raw[i].match(/^\s{0,3}(#{1,6})\s+(.+?)\s*#*\s*$/)
        : null;
      if (h) {
        const level = h[1].length;
        const title = h[2].trim();
        while (stack.length && stack[stack.length - 1].level >= level) stack.pop();
        const chain = stack.map((s) => s.title).concat([title]);
        const section = { title, chain, level, line: i + 1 + offset, id: slug(chain) };
        sections.push(section);
        stack.push(section);
        return;
      }
      for (const found of linksIn(line)) {
        const target = found.split('|')[0].trim();
        if (!target) continue;
        links.push({
          target,
          kind: isCodeTarget(target) ? 'code' : 'md',
          line: i + 1 + offset,
          from: stack.length ? stack[stack.length - 1].chain : [],
        });
      }
    });
    const title = sections.length && sections[0].level === 1 ? sections[0].title : key.split('/').pop();
    return { key, path, text, title, sections, links };
  }

  // Resolve a markdown wiki link against the loaded docs. Mirrors the rules
  // the checker enforces (and lat.md implements): full path or unique file
  // stem, case-insensitive headings, and an H1 that may be left implicit —
  // `[[cli#mcp]]` means cli.md → "# CLI" → "## mcp".
  function resolveLink(target, docs) {
    const parts = target.split('#');
    let head = parts[0].trim().replace(/^\.\//, '').replace(/\.md$/, '');
    const chain = parts.slice(1).map((p) => p.trim()).filter(Boolean);
    let doc = docs.get(head);
    if (!doc && !head.includes('/')) {
      const hits = [...docs.values()].filter((d) => d.key.split('/').pop() === head);
      if (hits.length === 1) doc = hits[0];
      else if (hits.length > 1) return { error: 'ambiguous', candidates: hits.map((d) => d.key) };
    }
    if (!doc) return { error: 'no such file' };
    if (!chain.length) return { doc, section: null };

    const lower = chain.map((c) => c.toLowerCase());
    const match = (offset) => doc.sections.find((s) => {
      const c = s.chain.slice(offset).map((x) => x.toLowerCase());
      return c.length === lower.length && c.every((x, i) => x === lower[i]);
    });
    const direct = match(0);
    if (direct) return { doc, section: direct };
    const implicit = match(1);          // the file's H1 left out of the link
    if (implicit) return { doc, section: implicit };
    return { error: 'no such section', doc };
  }

  // Who points here. Computed once per load: for every link in every doc,
  // record it under the document (and section) it resolves to.
  function buildBacklinks(docs) {
    const back = new Map();            // docKey -> [{from, fromSection, section, target}]
    for (const doc of docs.values()) {
      for (const link of doc.links) {
        if (link.kind !== 'md') continue;
        const r = resolveLink(link.target, docs);
        if (r.error || r.doc.key === doc.key) continue;
        if (!back.has(r.doc.key)) back.set(r.doc.key, []);
        back.get(r.doc.key).push({
          from: doc.key, fromSection: link.from,
          section: r.section ? r.section.chain : null, target: link.target,
        });
      }
    }
    return back;
  }

  // Nodes are files, edges are file→file references. Section-level detail
  // lives in the backlinks panel; at graph scale it is noise.
  function buildGraph(docs) {
    const nodes = [...docs.values()].map((d) => ({
      key: d.key, title: d.title, sections: d.sections.length, x: 0, y: 0, vx: 0, vy: 0,
    }));
    const index = new Map(nodes.map((n) => [n.key, n]));
    // from -> (to -> weight). Nested rather than keyed on a joined string: a
    // file name is free-form, and no separator is safely absent from one.
    const outgoing = new Map();
    let broken = 0;
    for (const doc of docs.values()) {
      for (const link of doc.links) {
        if (link.kind !== 'md') continue;
        const r = resolveLink(link.target, docs);
        if (r.error) { broken++; continue; }
        if (r.doc.key === doc.key) continue;
        if (!outgoing.has(doc.key)) outgoing.set(doc.key, new Map());
        const row = outgoing.get(doc.key);
        row.set(r.doc.key, (row.get(r.doc.key) || 0) + 1);
      }
    }
    const edges = [];
    for (const [from, row] of outgoing) {
      for (const [to, weight] of row) {
        edges.push({ source: index.get(from), target: index.get(to), weight });
      }
    }
    return { nodes, edges, broken };
  }

  /* ── Markdown rendering ────────────────────────────────────────────────
     Deliberately small. Anything the map format doesn't use (footnotes,
     images, HTML blocks) falls through as text rather than being half-
     supported: a viewer that quietly mangles a section is worse than one
     that shows it plainly. */

  // Inline code is split out before anything else runs, so a `[[link]]` quoted
  // as an example stays text. Splitting rather than substituting a placeholder:
  // a placeholder is one more string that must never occur in prose, and the
  // spans around it are exactly where spacing bugs hide.
  // Links and code spans are consumed in document order by the same rule the
  // parser uses, so a link whose heading contains `code` survives whole — the
  // earlier `split on code spans` approach tore it in three.
  function renderInline(text, ctx) {
    const re = new RegExp(LINK_OR_CODE.source, 'g');   // own instance: renderSpans may re-enter
    const out = [];
    let last = 0;
    let m;
    while ((m = re.exec(text)) !== null) {
      out.push(renderSpans(text.slice(last, m.index), ctx));
      out.push(m[0].startsWith('[[')
        ? renderWikiLink(m[0].slice(2, -2), ctx)
        : `<code>${escapeHtml(m[0].replace(/^`+|`+$/g, ''))}</code>`);
      last = m.index + m[0].length;
    }
    out.push(renderSpans(text.slice(last), ctx));
    return out.join('');
  }

  function renderWikiLink(body, ctx) {
    const bar = body.indexOf('|');
    const target = (bar === -1 ? body : body.slice(0, bar)).trim();
    // A label carries its own code spans: [[models#`Config` shape]] should read
    // as it was written, not as raw backticks.
    const label = escapeHtml((bar === -1 ? target : body.slice(bar + 1).trim()) || target)
      .replace(/`([^`]+)`/g, '<code>$1</code>');
    if (isCodeTarget(target)) {
      // The API only reaches .sbxw-artifacts, so source can't be opened from
      // here. Copying the path is the useful half — it pastes straight into
      // the IDE's "go to file".
      return `<span class="cm-code-link" data-path="${escapeHtml(target)}"
        title="Source reference — click to copy">${label}</span>`;
    }
    const r = ctx ? resolveLink(target, ctx.docs) : { error: 'no map loaded' };
    if (r.error) {
      return `<span class="cm-link broken" title="Broken link: ${escapeHtml(r.error)}">${label}</span>`;
    }
    const anchor = r.section ? r.section.id : '';
    return `<a class="cm-link" href="#" data-doc="${escapeHtml(r.doc.key)}"
      data-anchor="${anchor}" title="${escapeHtml(target)}">${label}</a>`;
  }

  function renderSpans(text, ctx) {
    let s = escapeHtml(text);
    s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g,
      (_, label, href) => `<a class="cm-ext" href="${href}" target="_blank" rel="noreferrer">${label}</a>`);
    s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    return s.replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  }

  function renderMarkdown(text, ctx) {
    const { body } = stripFrontmatter(text);
    const lines = body.split('\n');
    const out = [];
    const chain = [];
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];

      const fence = line.match(/^\s{0,3}(```+|~~~+)\s*(\S*)/);
      if (fence) {
        const buf = [];
        i++;
        while (i < lines.length && !lines[i].trimStart().startsWith(fence[1])) buf.push(lines[i++]);
        i++;
        const lang = fence[2] ? ` data-lang="${escapeHtml(fence[2])}"` : '';
        out.push(`<pre${lang}><code>${escapeHtml(buf.join('\n'))}</code></pre>`);
        continue;
      }

      const heading = line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
      if (heading) {
        const level = heading[1].length;
        const title = heading[2].trim();
        chain.length = Math.max(0, level - 1);
        chain[level - 1] = title;
        const id = slug(chain.slice(0, level).filter(Boolean));
        out.push(`<h${level} id="${id}" class="cm-h">${renderInline(title, ctx)}</h${level}>`);
        i++;
        continue;
      }

      if (/^\s{0,3}(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { out.push('<hr>'); i++; continue; }

      if (/^\s{0,3}>/.test(line)) {
        const buf = [];
        while (i < lines.length && /^\s{0,3}>/.test(lines[i])) buf.push(lines[i++].replace(/^\s{0,3}>\s?/, ''));
        out.push(`<blockquote>${renderMarkdown(buf.join('\n'), ctx)}</blockquote>`);
        continue;
      }

      // Pipe tables: only the shape maps actually use — header, separator, rows.
      if (/^\s*\|.*\|\s*$/.test(line) && /^\s*\|[\s:|-]+\|\s*$/.test(lines[i + 1] || '')) {
        const cells = (row) => row.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim());
        const head = cells(line);
        i += 2;
        const rows = [];
        while (i < lines.length && /^\s*\|.*\|\s*$/.test(lines[i])) rows.push(cells(lines[i++]));
        out.push(`<table class="cm-table"><thead><tr>${
          head.map((c) => `<th>${renderInline(c, ctx)}</th>`).join('')
        }</tr></thead><tbody>${
          rows.map((r) => `<tr>${r.map((c) => `<td>${renderInline(c, ctx)}</td>`).join('')}</tr>`).join('')
        }</tbody></table>`);
        continue;
      }

      const bullet = line.match(/^(\s*)([-*+]|\d+\.)\s+(.*)$/);
      if (bullet) {
        const ordered = /\d/.test(bullet[2]);
        const items = [];
        let depth = null;
        while (i < lines.length) {
          const m = lines[i].match(/^(\s*)([-*+]|\d+\.)\s+(.*)$/);
          if (!m) {
            // A wrapped continuation line belongs to the item above it.
            if (items.length && lines[i].trim() && /^\s+\S/.test(lines[i])) {
              items[items.length - 1] += ' ' + lines[i].trim();
              i++;
              continue;
            }
            break;
          }
          if (depth === null) depth = m[1].length;
          if (m[1].length < depth) break;
          if (m[1].length > depth) {
            // Nested list: hand the whole indented run back to the renderer.
            const nested = [];
            while (i < lines.length && /^\s+\S/.test(lines[i]) && lines[i].match(/^(\s*)([-*+]|\d+\.)\s+/)) {
              nested.push(lines[i].slice(depth + 2));
              i++;
            }
            items[items.length - 1] += renderMarkdown(nested.join('\n'), ctx);
            continue;
          }
          items.push(renderInline(m[3], ctx));
          i++;
        }
        const tag = ordered ? 'ol' : 'ul';
        out.push(`<${tag}>${items.map((it) => `<li>${it}</li>`).join('')}</${tag}>`);
        continue;
      }

      if (!line.trim()) { i++; continue; }

      const para = [];
      while (i < lines.length && lines[i].trim()
        && !/^(#{1,6})\s/.test(lines[i]) && !/^\s{0,3}(```|~~~|>)/.test(lines[i])
        && !/^(\s*)([-*+]|\d+\.)\s/.test(lines[i])) para.push(lines[i++]);
      out.push(`<p>${renderInline(para.join(' '), ctx)}</p>`);
    }
    return out.join('\n');
  }

  /* ── Graph layout ──────────────────────────────────────────────────────
     Fruchterman-Reingold with a cooling schedule. A map is a handful of
     files, so the O(n²) repulsion pass costs nothing and buys a layout that
     is stable between openings — no quadtree, no worker. */

  function tickGraph(nodes, edges, width, height, alpha) {
    const area = width * height;
    const k = Math.sqrt(area / Math.max(nodes.length, 1)) * 0.62;
    for (const a of nodes) {
      a.vx = 0; a.vy = 0;
      for (const b of nodes) {
        if (a === b) continue;
        let dx = a.x - b.x, dy = a.y - b.y;
        let dist = Math.hypot(dx, dy);
        if (dist < 0.01) { dx = (Math.random() - 0.5) * 2; dy = (Math.random() - 0.5) * 2; dist = 1; }
        const force = (k * k) / dist;
        a.vx += (dx / dist) * force;
        a.vy += (dy / dist) * force;
      }
    }
    for (const e of edges) {
      const dx = e.target.x - e.source.x, dy = e.target.y - e.source.y;
      const dist = Math.max(Math.hypot(dx, dy), 0.01);
      const force = (dist * dist) / k * Math.min(1 + (e.weight - 1) * 0.25, 2);
      const fx = (dx / dist) * force, fy = (dy / dist) * force;
      e.source.vx += fx; e.source.vy += fy;
      e.target.vx -= fx; e.target.vy -= fy;
    }
    const cx = width / 2, cy = height / 2;
    for (const n of nodes) {
      if (n.pinned) continue;
      // Gravity keeps disconnected files from drifting off the canvas — a map
      // in progress usually has one or two.
      n.vx += (cx - n.x) * 0.035 * k;
      n.vy += (cy - n.y) * 0.035 * k;
      const speed = Math.hypot(n.vx, n.vy) || 1;
      const step = Math.min(speed, alpha * 42) / speed;
      n.x = Math.max(28, Math.min(width - 28, n.x + n.vx * step));
      n.y = Math.max(24, Math.min(height - 24, n.y + n.vy * step));
    }
  }

  function seedLayout(nodes, width, height) {
    const r = Math.min(width, height) * 0.32;
    nodes.forEach((n, i) => {
      const a = (i / Math.max(nodes.length, 1)) * Math.PI * 2;
      n.x = width / 2 + Math.cos(a) * r;
      n.y = height / 2 + Math.sin(a) * r;
    });
  }

  /* ── State ─────────────────────────────────────────────────────────────── */

  const state = {
    sandbox: null,
    root: null,          // map directory under .sbxw-artifacts, e.g. "codemap"
    docs: new Map(),     // key -> doc
    others: [],          // markdown outside the map, viewable but not linked
    backlinks: new Map(),
    graph: null,
    current: null,       // doc key on screen
    view: 'doc',         // 'doc' | 'graph'
    filter: '',
    raf: null,
    alpha: 0,
  };

  const $ = (id) => document.getElementById(id);
  const api = (path) => `/api/sandboxes/${encodeURIComponent(state.sandbox)}/artifacts${path}`;

  // Which directory holds the map: `codemap/` by convention, otherwise any
  // directory carrying the index file the format requires (dir/dir.md).
  function findMapRoot(entries) {
    const dirs = new Set(entries
      .filter((e) => e.path.includes('/'))
      .map((e) => e.path.split('/').slice(0, -1).join('/')));
    if (dirs.has('codemap')) return 'codemap';
    for (const dir of dirs) {
      const stem = dir.split('/').pop();
      if (entries.some((e) => e.path === `${dir}/${stem}.md`)) return dir;
    }
    return null;
  }

  async function loadMap(name) {
    state.sandbox = name;
    state.docs = new Map();
    state.others = [];
    state.current = null;
    state.view = 'doc';
    $('codemap-doc').innerHTML = '<p class="cm-empty">Loading…</p>';
    $('codemap-files').innerHTML = '';
    $('codemap-side').innerHTML = '';

    let entries = [];
    try {
      const res = await fetch(api(''));
      const data = await res.json();
      entries = (data.entries || []).filter((e) => /\.(md|markdown)$/i.test(e.path));
    } catch (_) {
      $('codemap-doc').innerHTML = '<p class="cm-empty cm-error">Could not reach this sandbox.</p>';
      return;
    }

    state.root = findMapRoot(entries);
    $('codemap-dir').textContent = state.root
      ? `.sbxw-artifacts/${state.root}/`
      : '.sbxw-artifacts/';

    const inMap = state.root ? entries.filter((e) => e.path.startsWith(`${state.root}/`)) : [];
    state.others = entries.filter((e) => !inMap.includes(e));

    const texts = await Promise.all(inMap.map(async (e) => {
      try {
        const res = await fetch(api(`/download?path=${encodeURIComponent(e.path)}`));
        return { entry: e, text: await res.text() };
      } catch (_) { return { entry: e, text: '' }; }
    }));
    for (const { entry, text } of texts) {
      const key = entry.path.slice(state.root.length + 1).replace(/\.md$/i, '');
      state.docs.set(key, parseDoc(key, entry.path, text));
    }

    state.backlinks = buildBacklinks(state.docs);
    state.graph = state.docs.size ? buildGraph(state.docs) : null;
    renderFileList();
    updateStats();

    if (!state.docs.size) { showEmptyState(); return; }
    // The index is the map's front door: same file name as its directory.
    const rootStem = state.root.split('/').pop();
    openDoc(state.docs.has(rootStem) ? rootStem : [...state.docs.keys()][0]);
  }

  function showEmptyState() {
    $('codemap-doc').innerHTML = `
      <div class="cm-empty">
        <p><strong>No code map in this sandbox yet.</strong></p>
        <p>A map is a few linked markdown files under
        <code>.sbxw-artifacts/codemap/</code> describing what the project does and
        why — the layer the source can't state itself.</p>
        <p>Run <code>/codemap</code> in the agent pane to write one, then reopen
        this panel.</p>
      </div>`;
    $('codemap-side').innerHTML = '';
  }

  function updateStats() {
    const links = [...state.docs.values()].reduce((n, d) => n + d.links.length, 0);
    const sections = [...state.docs.values()].reduce((n, d) => n + d.sections.length, 0);
    const broken = state.graph ? state.graph.broken : 0;
    $('codemap-stats').innerHTML = state.docs.size
      ? `${state.docs.size} files · ${sections} sections · ${links} links`
        + (broken ? ` · <span class="cm-error">${broken} broken</span>` : '')
      : '';
  }

  function renderFileList() {
    const q = state.filter.trim().toLowerCase();
    const matches = (doc) => !q
      || doc.key.toLowerCase().includes(q)
      || doc.title.toLowerCase().includes(q)
      || doc.text.toLowerCase().includes(q);

    const rootStem = state.root ? state.root.split('/').pop() : '';
    const docs = [...state.docs.values()]
      .filter(matches)
      .sort((a, b) => (a.key === rootStem ? -1 : b.key === rootStem ? 1 : a.key.localeCompare(b.key)));

    const rows = docs.map((d) => {
      const hits = q ? d.sections.filter((s) => s.title.toLowerCase().includes(q)) : [];
      return `<div class="cm-file${d.key === state.current ? ' active' : ''}" data-doc="${escapeHtml(d.key)}">
          <span class="cm-file-name">${escapeHtml(d.key)}</span>
          <span class="cm-file-meta">${d.sections.length}</span>
        </div>` + hits.slice(0, 6).map((s) => `
          <div class="cm-file-hit" data-doc="${escapeHtml(d.key)}" data-anchor="${s.id}">
            ${escapeHtml(s.chain.join(' › '))}
          </div>`).join('');
    }).join('');

    const others = state.others.filter((e) => !q || e.path.toLowerCase().includes(q));
    const otherRows = others.length ? `
      <div class="cm-side-label">Other markdown</div>
      ${others.map((e) => `<div class="cm-file plain" data-raw="${escapeHtml(e.path)}">
        <span class="cm-file-name">${escapeHtml(e.path)}</span>
      </div>`).join('')}` : '';

    $('codemap-files').innerHTML = (rows || '<div class="cm-empty-small">No match</div>') + otherRows;
  }

  function openDoc(key, anchor) {
    const doc = state.docs.get(key);
    if (!doc) return;
    state.current = key;
    state.view = 'doc';
    syncViewButtons();
    const ctx = { docs: state.docs };
    $('codemap-doc').innerHTML = `<article class="cm-article">${renderMarkdown(doc.text, ctx)}</article>`;
    $('codemap-doc').scrollTop = 0;
    renderSide(doc);
    renderFileList();
    if (anchor) {
      const el = $('codemap-doc').querySelector(`[id="${CSS.escape(anchor)}"]`);
      if (el) el.scrollIntoView({ block: 'start' });
    }
  }

  // A file outside the map: rendered, but with no graph or backlinks to show.
  async function openRaw(path) {
    state.current = null;
    state.view = 'doc';
    syncViewButtons();
    $('codemap-doc').innerHTML = '<p class="cm-empty">Loading…</p>';
    let text = '';
    try {
      const res = await fetch(api(`/download?path=${encodeURIComponent(path)}`));
      text = await res.text();
    } catch (_) { text = '*Could not read this file.*'; }
    $('codemap-doc').innerHTML = `<article class="cm-article">${renderMarkdown(text, null)}</article>`;
    $('codemap-side').innerHTML = `<div class="cm-side-label">Outside the map</div>
      <p class="cm-empty-small">${escapeHtml(path)} isn't part of the graph, so it has no backlinks.</p>`;
    renderFileList();
  }

  function renderSide(doc) {
    const back = state.backlinks.get(doc.key) || [];
    const outline = doc.sections.map((s) => `
      <div class="cm-outline l${s.level}" data-anchor="${s.id}">${escapeHtml(s.title)}</div>`).join('');

    const backRows = back.length ? back.map((b) => `
      <div class="cm-back" data-doc="${escapeHtml(b.from)}"
           data-anchor="${b.fromSection.length ? slug(b.fromSection) : ''}">
        <span class="cm-back-from">${escapeHtml(b.from)}</span>
        ${b.fromSection.length ? `<span class="cm-back-sec">${escapeHtml(b.fromSection.join(' › '))}</span>` : ''}
      </div>`).join('') : '<p class="cm-empty-small">Nothing links here yet.</p>';

    const codeRefs = doc.links.filter((l) => l.kind === 'code');
    const codeRows = codeRefs.length ? `
      <div class="cm-side-label">Code referenced</div>
      ${codeRefs.map((l) => `<div class="cm-code-link" data-path="${escapeHtml(l.target)}"
        title="Click to copy">${escapeHtml(l.target)}</div>`).join('')}` : '';

    $('codemap-side').innerHTML = `
      <div class="cm-side-label">Referenced by (${back.length})</div>
      ${backRows}
      <div class="cm-side-label">Outline</div>
      ${outline || '<p class="cm-empty-small">No headings.</p>'}
      ${codeRows}`;
  }

  /* ── Graph view ───────────────────────────────────────────────────────── */

  const SVG_NS = 'http://www.w3.org/2000/svg';
  const svgEl = (tag, attrs) => {
    const el = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs || {})) el.setAttribute(k, v);
    return el;
  };

  function showGraph() {
    if (!state.graph || !state.graph.nodes.length) return;
    state.view = 'graph';
    syncViewButtons();
    const host = $('codemap-doc');
    host.innerHTML = '<svg class="cm-graph" id="codemap-graph"></svg>';
    const svg = $('codemap-graph');
    const width = host.clientWidth || 640;
    const height = host.clientHeight || 460;
    svg.setAttribute('viewBox', `0 0 ${width} ${height}`);

    const { nodes, edges } = state.graph;
    seedLayout(nodes, width, height);

    const edgeLayer = svgEl('g', { class: 'cm-graph-edges' });
    const nodeLayer = svgEl('g', { class: 'cm-graph-nodes' });
    svg.append(edgeLayer, nodeLayer);

    const lines = edges.map((e) => {
      const line = svgEl('line', { 'stroke-width': Math.min(1 + e.weight * 0.5, 3) });
      edgeLayer.appendChild(line);
      return { e, line };
    });

    const painted = nodes.map((n) => {
      const g = svgEl('g', { class: 'cm-node' + (n.key === state.current ? ' current' : '') });
      const r = 9 + Math.min(n.sections, 14) * 0.9;
      const circle = svgEl('circle', { r });
      const label = svgEl('text', { 'text-anchor': 'middle', y: r + 13 });
      label.textContent = n.key;
      g.append(circle, label);
      g.addEventListener('click', () => openDoc(n.key));
      g.addEventListener('pointerdown', (ev) => startDrag(ev, n, svg, width, height));
      nodeLayer.appendChild(g);
      return { n, g };
    });

    const paint = () => {
      for (const { e, line } of lines) {
        line.setAttribute('x1', e.source.x); line.setAttribute('y1', e.source.y);
        line.setAttribute('x2', e.target.x); line.setAttribute('y2', e.target.y);
      }
      for (const { n, g } of painted) g.setAttribute('transform', `translate(${n.x},${n.y})`);
    };

    state.alpha = 1;
    const step = () => {
      tickGraph(nodes, edges, width, height, state.alpha);
      paint();
      state.alpha *= 0.94;
      state.raf = state.alpha > 0.02 || state.dragging
        ? requestAnimationFrame(step)
        : null;
    };
    if (typeof requestAnimationFrame === 'function') step();
    else { for (let i = 0; i < 220; i++) tickGraph(nodes, edges, width, height, 1 - i / 240); paint(); }
  }

  function startDrag(ev, node, svg, width, height) {
    ev.preventDefault();
    state.dragging = node;
    node.pinned = true;
    const box = svg.getBoundingClientRect();
    const move = (e) => {
      node.x = Math.max(20, Math.min(width - 20, (e.clientX - box.left) / box.width * width));
      node.y = Math.max(20, Math.min(height - 20, (e.clientY - box.top) / box.height * height));
      if (state.alpha < 0.25) {
        state.alpha = 0.25;
        if (!state.raf && typeof requestAnimationFrame === 'function') showGraphResume();
      }
    };
    const up = () => {
      state.dragging = null;
      node.pinned = false;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  // Re-enter the cooling loop after a drag re-heated a settled graph.
  function showGraphResume() {
    if (state.view !== 'graph') return;
    const svg = $('codemap-graph');
    if (!svg) return;
    const host = $('codemap-doc');
    const width = host.clientWidth || 640, height = host.clientHeight || 460;
    const { nodes, edges } = state.graph;
    const step = () => {
      tickGraph(nodes, edges, width, height, state.alpha);
      const lines = svg.querySelectorAll('.cm-graph-edges line');
      edges.forEach((e, i) => {
        const line = lines[i];
        if (!line) return;
        line.setAttribute('x1', e.source.x); line.setAttribute('y1', e.source.y);
        line.setAttribute('x2', e.target.x); line.setAttribute('y2', e.target.y);
      });
      svg.querySelectorAll('.cm-node').forEach((g, i) => {
        const n = nodes[i];
        if (n) g.setAttribute('transform', `translate(${n.x},${n.y})`);
      });
      state.alpha *= 0.94;
      state.raf = state.alpha > 0.02 || state.dragging ? requestAnimationFrame(step) : null;
    };
    state.raf = requestAnimationFrame(step);
  }

  function syncViewButtons() {
    $('codemap-view-doc').classList.toggle('active', state.view === 'doc');
    $('codemap-view-graph').classList.toggle('active', state.view === 'graph');
    if (state.view !== 'graph' && state.raf) {
      cancelAnimationFrame(state.raf);
      state.raf = null;
    }
  }

  /* ── Wiring ───────────────────────────────────────────────────────────── */

  function open(name) {
    $('codemap-modal-overlay').classList.remove('hidden');
    $('codemap-name').textContent = name;
    $('codemap-search').value = '';
    state.filter = '';
    loadMap(name);
  }

  function close() {
    $('codemap-modal-overlay').classList.add('hidden');
    if (state.raf) { cancelAnimationFrame(state.raf); state.raf = null; }
  }

  function init() {
    const overlay = $('codemap-modal-overlay');
    if (!overlay) return;               // markup absent (a test harness, say)

    $('codemap-close').addEventListener('click', close);
    $('codemap-close2').addEventListener('click', close);
    overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !overlay.classList.contains('hidden')) close();
    });
    $('codemap-refresh').addEventListener('click', () => { if (state.sandbox) loadMap(state.sandbox); });
    $('codemap-view-doc').addEventListener('click', () => {
      if (state.current) openDoc(state.current);
      else if (state.docs.size) openDoc([...state.docs.keys()][0]);
    });
    $('codemap-view-graph').addEventListener('click', showGraph);
    $('codemap-search').addEventListener('input', (e) => {
      state.filter = e.target.value;
      renderFileList();
    });

    // One delegated handler for everything clickable inside the panel: the
    // rendered document is replaced wholesale on every navigation, so per-node
    // listeners would have to be rebound each time.
    overlay.addEventListener('click', (e) => {
      const link = e.target.closest('.cm-link[data-doc]');
      if (link) {
        e.preventDefault();
        openDoc(link.dataset.doc, link.dataset.anchor || undefined);
        return;
      }
      const file = e.target.closest('.cm-file, .cm-file-hit, .cm-back');
      if (file) {
        if (file.dataset.raw) openRaw(file.dataset.raw);
        else if (file.dataset.doc) openDoc(file.dataset.doc, file.dataset.anchor || undefined);
        return;
      }
      const outline = e.target.closest('.cm-outline[data-anchor]');
      if (outline) {
        const el = $('codemap-doc').querySelector(`[id="${CSS.escape(outline.dataset.anchor)}"]`);
        if (el) el.scrollIntoView({ block: 'start', behavior: 'smooth' });
        return;
      }
      const code = e.target.closest('.cm-code-link[data-path]');
      if (code && navigator.clipboard) {
        navigator.clipboard.writeText(code.dataset.path).then(() => {
          code.classList.add('copied');
          setTimeout(() => code.classList.remove('copied'), 900);
        }, () => {});
      }
    });
  }

  if (typeof document !== 'undefined') init();
  if (typeof window !== 'undefined') window.openCodemapModal = open;
  // Node can require this file to exercise the parsing and layout directly.
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = {
      parseDoc, resolveLink, buildBacklinks, buildGraph, renderMarkdown, renderInline,
      findMapRoot, tickGraph, seedLayout, slug, isCodeTarget,
    };
  }
})();

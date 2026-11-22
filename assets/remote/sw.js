// sbxw: the service worker. It exists only to show a notification when a push
// arrives with the phone locked. Two listeners, nothing else: it reads no
// data, stores nothing and makes no request. The only text it ever shows is
// in the table below (French, chosen by the person), with at most one thing
// from the push in it: the sandbox's name, and only when it is 1 to 63
// letters, digits and hyphens (plain text: a notification renders no HTML).
'use strict';

const TITLE = 'sbxw';
const TEXT_PLAN = 'Un plan vous attend';
const TEXT_MERGE = 'Un merge vous attend';
const TEXT_AGENT = 'Un agent a besoin de vous';
const TEXT_OTHER = 'Quelque chose vous attend';

// The sandbox's name, or null: anything but a sbx name is never shown.
function nameFor(n) {
  return typeof n === 'string' && /^[A-Za-z0-9-]{1,63}$/.test(n) ? n : null;
}

// The kind ("p", "m" or "n", API-push.md) picks the text: with the sandbox's
// name when the push carries a valid one, else the fixed text. Anything
// else, an empty or unreadable push included, still shows the fallback,
// because a push that shows nothing makes iOS revoke the permission.
function bodyFor(kind, name, lead) {
  const n = nameFor(name);
  if (kind === 'p') return n ? `Un plan vous attend dans ${n}` : TEXT_PLAN;
  if (kind === 'm') return n ? `Un merge vous attend dans ${n}` : TEXT_MERGE;
  if (kind === 'n') return n ? `${lead === true ? 'Team leader ' : ''}${n} a besoin de vous` : TEXT_AGENT;
  return TEXT_OTHER;
}

// The count for the app icon: a whole number, 0 to 99, else not touched.
function badgeFor(n) {
  if (typeof n !== 'number' || !Number.isInteger(n)) return null;
  return Math.max(0, Math.min(99, n));
}

async function setBadge(n) {
  try {
    if (n === null) return;
    if (n > 0 && typeof self.navigator.setAppBadge === 'function') await self.navigator.setAppBadge(n);
    else if (n === 0 && typeof self.navigator.clearAppBadge === 'function') await self.navigator.clearAppBadge();
  } catch (_) { /* a badge that fails never costs the notification */ }
}

self.addEventListener('push', event => {
  let kind = null;
  let name = null;
  let lead = false;
  let badge = null;
  try {
    const d = event.data ? event.data.json() : null;
    if (d && typeof d === 'object') {
      kind = d.kind;
      name = d.name;
      lead = d.lead;
      badge = badgeFor(d.badge);
    }
  } catch (_) { /* unreadable: the fallback text */ }
  event.waitUntil((async () => {
    // The notification first, always; the same tag, so a newer one replaces
    // the older one.
    let shown = null;
    try {
      shown = self.registration.showNotification(TITLE, { body: bodyFor(kind, name, lead), tag: TITLE, renotify: true, lang: 'fr' });
    } catch (_) { /* nothing more to do */ }
    await setBadge(badge);
    try { await shown; } catch (_) { /* nothing more to do */ }
  })());
});

self.addEventListener('notificationclick', event => {
  event.notification.close();
  event.waitUntil((async () => {
    const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    for (const w of wins) {
      if (typeof w.focus === 'function') return w.focus();
    }
    return self.clients.openWindow('/');
  })());
});

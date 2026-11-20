// sbxw: the service worker. It exists only to show a notification when a push
// arrives with the phone locked. Two listeners, nothing else: it reads no
// data, stores nothing and makes no request. The only text it ever shows is
// in the table below (French, chosen by the person); nothing from the push
// is shown as text, whatever it carries.
'use strict';

const TITLE = 'sbxw';
const TEXT_PLAN = 'Un plan vous attend';
const TEXT_AGENT = 'Un agent a besoin de vous';
const TEXT_OTHER = 'Quelque chose vous attend';

// The kind ("p" or "n", API-push.md) picks one fixed text; anything else,
// an empty or unreadable push included, still shows the fallback, because a
// push that shows nothing makes iOS revoke the permission.
function bodyFor(kind) {
  if (kind === 'p') return TEXT_PLAN;
  if (kind === 'n') return TEXT_AGENT;
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
  let badge = null;
  try {
    const d = event.data ? event.data.json() : null;
    if (d && typeof d === 'object') {
      kind = d.kind;
      badge = badgeFor(d.badge);
    }
  } catch (_) { /* unreadable: the fallback text */ }
  event.waitUntil((async () => {
    // The notification first, always; the same tag, so a newer one replaces
    // the older one.
    let shown = null;
    try {
      shown = self.registration.showNotification(TITLE, { body: bodyFor(kind), tag: TITLE, renotify: true, lang: 'fr' });
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

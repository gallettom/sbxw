// ── sbxw extension: options ───────────────────────────────────────────────
// One setting: where the sbxw daemon listens. `sbxw extension` writes the
// configured `web_addr` into defaults.js; this overrides it, for a daemon
// started with another config.

const input = document.getElementById('daemon');
const note = document.getElementById('note');

chrome.storage.local.get('daemon').then(({ daemon }) => {
  input.value = daemon || SBXW_DAEMON_DEFAULT;
});

document.getElementById('form').addEventListener('submit', async e => {
  e.preventDefault();
  let origin;
  try {
    const u = new URL(input.value.trim());
    if (!/^https?:$/.test(u.protocol)) throw new Error();
    origin = u.origin;
  } catch (_) {
    note.textContent = 'An address like http://127.0.0.1:7681';
    return;
  }
  // The default is stored as "nothing", so a later `sbxw extension` with a new
  // web_addr still takes effect.
  if (origin === new URL(SBXW_DAEMON_DEFAULT).origin) await chrome.storage.local.remove('daemon');
  else await chrome.storage.local.set({ daemon: origin });
  input.value = origin;
  note.textContent = 'Saved. Open panels reconnect on their own.';
});

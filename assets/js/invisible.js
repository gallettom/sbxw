// The one list of invisible and bidirectional characters
// (docs/security.md "Invisible characters"): the same inclusive ranges as
// INVISIBLE in crates/core/src/invisible.rs, which the daemon and the desktop
// use (tests/js/invisible.test.mjs compares them). U+FE0E and U+FE0F are left
// out on purpose, so emoji like ❤️ pass. Every place of the web UI that strips,
// refuses or shows invisible characters asks this list, each with its own
// action and its own rule for control characters: oscPreview (panes.js),
// paneTypeLine and paneTypePaths (pane-files.js), chatShortName (chat-core.js;
// the phone's copy reads RC_INVISIBLE_RANGES, the same ranges).
// Loaded first (singleton.js, and the side panel page), before any script
// that uses it.
const INVISIBLE_RANGES = [
  [0x00AD, 0x00AD], [0x034F, 0x034F], [0x0600, 0x0605], [0x061C, 0x061C], [0x06DD, 0x06DD],
  [0x070F, 0x070F], [0x0890, 0x0891], [0x08E2, 0x08E2], [0x115F, 0x1160], [0x17B4, 0x17B5],
  [0x180B, 0x180F], [0x200B, 0x200F], [0x2028, 0x2029], [0x202A, 0x202E], [0x2060, 0x206F],
  [0x3164, 0x3164], [0xFE00, 0xFE0D], [0xFEFF, 0xFEFF], [0xFFA0, 0xFFA0], [0xFFF9, 0xFFFB],
  [0x110BD, 0x110BD], [0x110CD, 0x110CD], [0x13430, 0x1343F], [0x1BCA0, 0x1BCA3], [0x1D173, 0x1D17A],
  [0xE0000, 0xE0FFF],
];
// The list as the inside of a regexp character class (flag u).
const INVISIBLE_CLASS = INVISIBLE_RANGES.map(([a, b]) => `\\u{${a.toString(16)}}-\\u{${b.toString(16)}}`).join('');
// Is code point `n` on the list?
function invisibleCode(n) {
  return INVISIBLE_RANGES.some(([a, b]) => n >= a && n <= b);
}

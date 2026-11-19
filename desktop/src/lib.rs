//! sbxw desktop: a native client of the sbxw daemon, drawn with Rune.
//!
//! * [`client`]: every call to the daemon, behind one trait, and the link that keeps it up.
//! * [`shell`]: the window, its panes, and the intents screens ask for.
//! * [`screen`]: draws a Luau screen with Rune elements and reloads it on change.
//! * [`luau`]: the sandboxed VM that runs screen files, and the frozen element set.
//! * [`feat`]: the features (network, create, settings...), each declared in one place.
//! * [`intent`]: the closed list of what a screen may ask.
//! * [`approval`]: the core's own approval dialogs (Jarvis proposals, clipboard offers).
//! * [`jarvis`]: Jarvis's conversation, state and proposal rule; [`markdown`]: the `markdown` element.
//! * [`voice`]: the audio seam; [`audio`]: native audio behind it (thread desktop-audio).
//! * [`builtin`]: the built-in screens written out for the daemon.
//! * [`theme`], [`icons`]: the web UI's tokens; the icons screens may name.
//! * [`fixture`]: daemon data from JSON files, for headless screens.
//! * [`surface`]: picture surfaces drawn by the core, input back to features; [`socket`]: features' WebSockets.
//! * [`files`]: downloads, opens, reveals and uploads the core runs; [`safe`]: its file-name and open-by-type rules.
//! * [`terminal`]: a terminal emulator drawn with Rune's grid (thread desktop-terminal).

pub mod approval;
pub mod audio;
pub mod builtin;
pub mod client;
pub mod emphasis;
pub mod feat;
pub mod files;
pub mod fixture;
pub mod icons;
pub mod intent;
pub mod jarvis;
pub mod luau;
pub mod markdown;
pub mod mem;
pub mod perf;
pub mod picture;
pub mod safe;
pub mod scope;
pub mod switch;
pub mod screen;
pub mod shell;
pub mod socket;
pub mod surface;
pub mod terminal;
pub mod theme;
pub mod voice;

//! sbxw desktop: a native client of the sbxw daemon, drawn with Rune.
//!
//! * [`client`]: every call to the daemon, behind one trait, and the link that keeps it up.
//! * [`shell`]: the window, its panes, and the intents screens ask for.
//! * [`screen`]: draws a Luau screen with Rune elements and reloads it on change.
//! * [`luau`]: the sandboxed VM that runs screen files, and the frozen element set.
//! * [`intent`]: the closed list of what a screen may ask.
//! * [`approval`]: where the core's own approval dialogs will go (wave 4).
//! * [`theme`], [`icons`]: the web UI's tokens; the icons screens may name.
//! * [`fixture`]: daemon data from JSON files, for headless screens.
//! * [`terminal`]: a terminal emulator drawn with Rune's grid (thread desktop-terminal).

pub mod approval;
pub mod audio;
pub mod client;
pub mod fixture;
pub mod icons;
pub mod intent;
pub mod luau;
pub mod screen;
pub mod shell;
pub mod terminal;
pub mod theme;

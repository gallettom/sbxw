//! sbxw desktop: a native client of the sbxw daemon (spike, Rune version).
//!
//! * [`client`]: every call to the daemon, behind one trait.
//! * [`terminal`]: a terminal emulator (alacritty_terminal) drawn with Rune's grid element.
//! * [`luau`]: the sandboxed VM that runs screen files.
//! * [`screen`]: draws a Luau screen with Rune elements and reloads it on change.

pub mod client;
pub mod luau;
pub mod screen;
pub mod terminal;

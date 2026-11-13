//! sbxw desktop: a native client of the sbxw daemon (spike).
//!
//! * [`client`]: every call to the daemon, behind one trait.
//! * [`terminal`]: a terminal emulator (alacritty_terminal) drawn with GPUI.
//! * [`luau`]: the sandboxed VM that runs screen files.
//! * [`screen`]: a GPUI view that draws a Luau screen and reloads it on change.

pub mod client;
pub mod luau;
pub mod screen;
pub mod terminal;

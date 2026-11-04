//! Mirroring kits to a host: each kit is copied once per content hash
//! (`~/.sbxw/kits/<hash>/` on the host), so a sandbox there is created from
//! the same bytes as locally and an unchanged kit is never sent twice.
//!
//! Empty for now: phase 2's kit-mirroring thread fills it.

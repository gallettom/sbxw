//! Monitor: follow every sandbox's sessions in one calm view over the pane
//! area, instead of reading each agent's terminal.
//!
//! STAGE 1 (the proposal, `.sbxw-artifacts/desktop/MONITOR.md`): three
//! throwaway concept screens, read only, no intents, no rail entry. They are
//! opened from a fixture (`app.panels`) to be shot, and removed in stage 2
//! (`fixtures/monitor/proposal/`).

use super::{Feature, ScreenDecl, Slot};

pub const FEATURE: Feature = Feature {
    name: "monitor",
    screens: &[
        ScreenDecl { name: "monitor-board", src: include_str!("../../../fixtures/monitor/proposal/board.luau"), slot: Slot::Overlay { title: "Monitor: Board", icon: "grid" } },
        ScreenDecl { name: "monitor-inbox", src: include_str!("../../../fixtures/monitor/proposal/inbox.luau"), slot: Slot::Overlay { title: "Monitor", icon: "threads" } },
        ScreenDecl { name: "monitor-timeline", src: include_str!("../../../fixtures/monitor/proposal/timeline.luau"), slot: Slot::Overlay { title: "Monitor: Timeline", icon: "history" } },
    ],
    ..Feature::EMPTY
};

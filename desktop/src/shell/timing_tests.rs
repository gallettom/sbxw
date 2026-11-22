//! Timings for the data handed to the screens, on the performance runs' big
//! fixture (`scripts/perf-fixture.py`, about 8 MB). Ignored by default: run
//!
//!   python3 scripts/perf-fixture.py > /tmp/big.json
//!   SBXW_PERF_FIXTURE=/tmp/big.json cargo test --release timing_ -- --ignored --nocapture
//!
//! Each prints the median of its runs; nothing is asserted on time (a loaded
//! machine would make that flaky). The numbers are quoted in the Rust
//! quality report (.sbxw-artifacts/reports/rust-quality-desktop.md).

use super::*;
use std::time::Instant;

fn big() -> Option<String> {
    std::env::var("SBXW_PERF_FIXTURE").ok().filter(|p| std::path::Path::new(p).exists())
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn time<F: FnMut()>(n: usize, mut f: F) -> f64 {
    median(
        (0..n)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    )
}

/// `App::screen_data` (what every push builds) on the big fixture.
#[test]
#[ignore]
fn timing_screen_data() {
    let Some(path) = big() else { return eprintln!("SBXW_PERF_FIXTURE not set: skipped") };
    let a = App::new(Args { fixture: Some(path), ..Args::default() });
    let bytes = a.screen_data().to_string().len();
    let ms = time(15, || {
        std::hint::black_box(a.screen_data());
    });
    eprintln!("timing: screen_data {ms:.2} ms median of 15 ({bytes} bytes)");
}

/// One render of the sandbox list on new data, through the path a push
/// takes (`set_data_shown`), on the big fixture's data.
#[test]
#[ignore]
fn timing_screen_render() {
    let Some(path) = big() else { return eprintln!("SBXW_PERF_FIXTURE not set: skipped") };
    let a = App::new(Args { fixture: Some(path), ..Args::default() });
    let d = a.screen_data();
    let bytes = d.to_string().len();
    let mut other = d.clone();
    other["sandboxes"][0]["cpu"] = serde_json::json!(99.5);
    let (one, two) = (std::rc::Rc::new(d), std::rc::Rc::new(other));
    let dir = std::env::temp_dir().join("sbxw-no-overrides");
    let mut s = screen::ScreenState::new("sandboxes", &dir, (*one).clone());
    let mut flip = false;
    let before = s.renders;
    let ms = time(21, || {
        flip = !flip;
        s.set_data_shown(if flip { two.clone() } else { one.clone() }, true);
    });
    assert_eq!(s.renders - before, 21, "every change rendered");
    assert!(s.error.is_none(), "{:?}", s.error);
    eprintln!("timing: sandboxes render {ms:.2} ms median of 21 (data {bytes} bytes)");
}

/// The core's Markdown element for 400 messages of 884 bytes each (a
/// long conversation): what each `view` rebuild pays for Markdown nodes,
/// without the parse cache (`view_blocks` on a fresh parse) and with it
/// (`view`, every text already seen: the steady state while a conversation
/// is drawn). Then the same for the perf fixture's shorter replies.
/// Runs without the fixture.
#[test]
#[ignore]
fn timing_markdown_view() {
    let long = "## Plan\n\nThe **first** step reads `src/main.rs` and the [docs](https://example.com/docs).\n\n- one item with `code`\n- two, *in italics*\n- three\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n".repeat(4);
    let look = crate::markdown::Look { color: 0xdddddd, size: 13.0, pills: None };
    let short: Vec<String> = (0..400).map(|i| format!("Step {i} is **done**: the copy reads better and `npm test pay` passes.\n\n- one change\n- [[Done]]")).collect();
    // Distinct texts, as in a real conversation (a cache keyed by text must not
    // win by seeing one text 400 times).
    let longs: Vec<String> = (0..400).map(|i| format!("{long}{i}")).collect();
    for (name, msgs) in [("884-byte", &longs), ("fixture", &short)] {
        let bytes = msgs[0].len();
        let parse = time(11, || {
            for m in msgs {
                std::hint::black_box(crate::markdown::parse(m));
            }
        });
        let fresh = time(11, || {
            for (i, m) in msgs.iter().enumerate() {
                std::hint::black_box(crate::markdown::view_blocks::<App>(&crate::markdown::parse(m), &look, &format!("m{i}"), None));
            }
        });
        let cached = time(11, || {
            for (i, m) in msgs.iter().enumerate() {
                std::hint::black_box(crate::markdown::view::<App>(m, &look, &format!("m{i}"), None));
            }
        });
        eprintln!("timing: markdown 400 x {bytes} bytes ({name}): parse {parse:.2} ms, element without cache {fresh:.2} ms, with cache {cached:.2} ms (median of 11)");
    }
}

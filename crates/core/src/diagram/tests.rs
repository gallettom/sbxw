use super::*;

const WORKFLOW: &str = include_str!("fixtures/thread-review.workflow.json");
const SEQUENCE: &str = include_str!("fixtures/canvas-review.sequence.json");
const LIFECYCLE: &str = include_str!("fixtures/thread-run.lifecycle.json");
const ARCHITECTURE: &str = include_str!("fixtures/codemap.architecture.json");

fn laid(json: &str) -> Diagram {
    let mut d = import::archify(json).unwrap();
    layout::place_missing(&mut d);
    d
}

#[test]
fn print_fixtures() {
    for f in [WORKFLOW, SEQUENCE, LIFECYCLE, ARCHITECTURE] {
        let d = laid(f);
        println!("== {} ({:?})", d.title, d.kind);
        for l in &d.lanes {
            println!("lane {:10} at {:6} span {:6}", l.id, l.at, l.span);
        }
        for n in &d.nodes {
            println!("{:12} lane {:8} x {:6} y {:6} w {:4} h {:3}", n.id, n.lane.clone().unwrap_or_default(), n.x, n.y, n.w, n.h);
        }
        println!("overlaps {:?}", layout::overlaps(&d));
    }
}

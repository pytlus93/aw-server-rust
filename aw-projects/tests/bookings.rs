use aw_projects::{bookings::*, *};
use uuid::Uuid;

fn span(start: &str, end: &str) -> Span {
    Span {
        start: start.parse().unwrap(),
        end: end.parse().unwrap(),
    }
}
fn interval(a: u32, b: u32) -> Span {
    span(
        &format!("2026-10-11T10:{a:02}:00Z"),
        &format!("2026-10-11T10:{b:02}:00Z"),
    )
}
fn node() -> Node {
    Node {
        id: Uuid::from_u128(1),
        workspace_id: Uuid::from_u128(100),
        parent_id: None,
        kind: NodeKind::Project,
        name: "Synthetic".into(),
        description: "".into(),
        case_id: None,
        archived: false,
        billing: BillingMode::Inherited,
        billing_terms: Default::default(),
        reporting: ReportingPolicy::Inherited,
    }
}
fn entry(id: u128, intervals: Vec<Span>) -> Entry {
    Entry {
        id: Uuid::from_u128(id),
        node_id: node().id,
        description: "Synthetic work".into(),
        intervals,
        sources: vec![],
        deleted: false,
        duration_ns: "".into(),
    }
}

#[test]
fn overlaps_union_but_disjoint_gaps_remain_with_exact_nanoseconds() {
    let mut e = entry(10, vec![interval(0, 10), interval(5, 15), interval(20, 25)]);
    e.normalize().unwrap();
    assert_eq!(e.intervals, vec![interval(0, 15), interval(20, 25)]);
    assert_eq!(e.duration_ns, "1200000000000");
    e.intervals = vec![span(
        "2026-10-11T10:00:00.000000001Z",
        "2026-10-11T10:00:00.000000004Z",
    )];
    e.normalize().unwrap();
    assert_eq!(e.duration_ns, "3");
}

#[test]
fn adjacent_bookings_are_allowed_and_overlap_is_rejected() {
    let tree = Hierarchy::new([node()]).unwrap();
    let entries = apply(&tree, &[], vec![entry(10, vec![interval(0, 10)])], &[]).unwrap();
    assert!(apply(
        &tree,
        &entries,
        vec![entry(11, vec![interval(10, 20)])],
        &[]
    )
    .is_ok());
    assert!(apply(&tree, &entries, vec![entry(11, vec![interval(5, 20)])], &[]).is_err());
    assert!(apply(
        &tree,
        &entries,
        vec![entry(11, vec![interval(10, 10)])],
        &[]
    )
    .is_err());
}

#[test]
fn split_and_move_are_atomic_and_deleted_ids_cannot_be_reused() {
    let tree = Hierarchy::new([node()]).unwrap();
    let entries = apply(&tree, &[], vec![entry(10, vec![interval(0, 20)])], &[]).unwrap();
    let split = apply(
        &tree,
        &entries,
        vec![
            entry(11, vec![interval(0, 10)]),
            entry(12, vec![interval(10, 20)]),
        ],
        &[Uuid::from_u128(10)],
    )
    .unwrap();
    assert!(split[0].deleted);
    assert_eq!(
        split
            .iter()
            .filter(|e| !e.deleted)
            .map(|e| e.duration_ns.parse::<i128>().unwrap())
            .sum::<i128>(),
        1200000000000
    );
    assert!(apply(&tree, &split, vec![entry(10, vec![interval(30, 40)])], &[]).is_err());
    assert!(apply(&tree, &split, vec![entry(11, vec![interval(15, 25)])], &[]).is_err());
}

#[test]
fn archive_blocks_new_assignments_without_blocking_existing_corrections() {
    let tree = Hierarchy::new([node()]).unwrap();
    let entries = apply(&tree, &[], vec![entry(10, vec![interval(0, 10)])], &[]).unwrap();
    let mut archived = node();
    archived.archived = true;
    let tree = Hierarchy::new([archived]).unwrap();
    assert!(apply(
        &tree,
        &entries,
        vec![entry(11, vec![interval(20, 30)])],
        &[]
    )
    .is_err());
    assert!(apply(&tree, &entries, vec![entry(10, vec![interval(0, 15)])], &[]).is_ok());
}

#[test]
fn dst_offset_intervals_measure_elapsed_time() {
    let mut e = entry(
        10,
        vec![span(
            "2026-10-25T02:30:00+02:00",
            "2026-10-25T02:30:00+01:00",
        )],
    );
    e.normalize().unwrap();
    assert_eq!(e.duration_ns, "3600000000000");
}

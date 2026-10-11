use aw_projects::*;
use chrono::NaiveDate;
use uuid::Uuid;

fn node(id: u128, parent: Option<u128>, kind: NodeKind) -> Node {
    Node {
        id: Uuid::from_u128(id),
        workspace_id: Uuid::from_u128(100),
        parent_id: parent.map(Uuid::from_u128),
        kind,
        name: format!("Synthetic {id}"),
        case_id: None,
        archived: false,
        billing: BillingMode::Inherited,
        reporting: ReportingPolicy::Inherited,
    }
}

fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).unwrap()
}

#[test]
fn depth_limit_and_duplicate_identity_are_enforced() {
    let nodes: Vec<_> = (1..=MAX_DEPTH as u128)
        .map(|id| node(id, (id > 1).then_some(id - 1), NodeKind::Project))
        .collect();
    assert!(Hierarchy::new(nodes.clone()).is_ok());
    let mut too_deep = nodes;
    too_deep.push(node(
        MAX_DEPTH as u128 + 1,
        Some(MAX_DEPTH as u128),
        NodeKind::Task,
    ));
    assert!(matches!(
        Hierarchy::new(too_deep),
        Err(DomainError::DepthLimit)
    ));
    let root = node(1, None, NodeKind::Project);
    assert_eq!(
        Hierarchy::new([root.clone(), root]).unwrap_err(),
        DomainError::DuplicateId(Uuid::from_u128(1))
    );
}

fn mapping(id: u128, owner: u128, month: u32) -> ActivityIdMapping {
    ActivityIdMapping {
        id: Uuid::from_u128(id),
        owner_node_id: Uuid::from_u128(owner),
        valid_from: date(month, 1),
        valid_until: Some(date(month + 1, 1)),
        activity_id: format!("ACT-{month}"),
    }
}

#[test]
fn nested_projects_and_tasks_resolve_independent_policies() {
    let mut root = node(1, None, NodeKind::Project);
    root.billing = BillingMode::Tm;
    let mut case = node(2, Some(1), NodeKind::Project);
    case.reporting = ReportingPolicy::SelfNode;
    let task = node(3, Some(2), NodeKind::Task);
    let mut child = node(4, Some(3), NodeKind::Task);
    child.billing = BillingMode::Fixed;
    let tree = Hierarchy::new([root, case, task, child]).unwrap();
    let policy = tree.resolve(Uuid::from_u128(4)).unwrap();
    assert_eq!(policy.billing, BillingMode::Fixed);
    assert_eq!(policy.billing_owner_id, Some(Uuid::from_u128(4)));
    assert_eq!(policy.reporting_target_id, Uuid::from_u128(2));
    assert_eq!(tree.path(Uuid::from_u128(4)).unwrap().len(), 4);
}

#[test]
fn rejects_cycles_orphans_cross_workspace_and_project_under_task() {
    assert!(matches!(
        Hierarchy::new([
            node(1, Some(2), NodeKind::Project),
            node(2, Some(1), NodeKind::Project)
        ]),
        Err(DomainError::Cycle(_))
    ));
    assert!(matches!(
        Hierarchy::new([node(1, Some(9), NodeKind::Project)]),
        Err(DomainError::MissingNode(_))
    ));
    let mut child = node(2, Some(1), NodeKind::Task);
    child.workspace_id = Uuid::from_u128(200);
    assert!(matches!(
        Hierarchy::new([node(1, None, NodeKind::Project), child]),
        Err(DomainError::CrossWorkspace(_))
    ));
    assert!(matches!(
        Hierarchy::new([
            node(1, None, NodeKind::Project),
            node(2, Some(1), NodeKind::Task),
            node(3, Some(2), NodeKind::Project)
        ]),
        Err(DomainError::InvalidParent(_))
    ));
}

#[test]
fn reparenting_cannot_leave_descendant_reporting_target_outside_ancestry() {
    let mut child = node(3, Some(2), NodeKind::Task);
    child.reporting = ReportingPolicy::Ancestor {
        target_id: Uuid::from_u128(1),
    };
    let root = node(1, None, NodeKind::Project);
    assert!(Hierarchy::new([
        root.clone(),
        node(2, Some(1), NodeKind::Task),
        child.clone()
    ])
    .is_ok());
    assert_eq!(
        Hierarchy::new([root, node(2, None, NodeKind::Project), child]).unwrap_err(),
        DomainError::InvalidReportingTarget(Uuid::from_u128(3))
    );
}

#[test]
fn archive_propagates_effectively_and_root_defaults_are_explicit() {
    let mut root = node(1, None, NodeKind::Project);
    root.archived = true;
    let tree = Hierarchy::new([root, node(2, Some(1), NodeKind::Task)]).unwrap();
    let policy = tree.resolve(Uuid::from_u128(2)).unwrap();
    assert!(policy.effectively_archived);
    assert_eq!(policy.billing, BillingMode::NonBillable);
    assert_eq!(policy.reporting_target_id, Uuid::from_u128(1));
    assert_eq!(policy.billing_owner_id, None);
}

#[test]
fn monthly_mapping_uses_work_date_with_exclusive_boundary_and_unknown_gaps() {
    let tree = Hierarchy::new([
        node(1, None, NodeKind::Project),
        node(2, Some(1), NodeKind::Task),
    ])
    .unwrap();
    let mappings = [mapping(10, 1, 10), mapping(11, 1, 11)];
    let assigned = Uuid::from_u128(2);
    assert_eq!(
        tree.activity_id(assigned, date(10, 31), &mappings)
            .unwrap()
            .unwrap()
            .activity_id,
        "ACT-10"
    );
    assert_eq!(
        tree.activity_id(assigned, date(11, 1), &mappings)
            .unwrap()
            .unwrap()
            .activity_id,
        "ACT-11"
    );
    assert!(tree
        .activity_id(assigned, date(9, 30), &mappings)
        .unwrap()
        .is_none());
}

#[test]
fn mapping_overlap_is_per_target_and_open_ended_range_blocks_later_ranges() {
    let tree = Hierarchy::new([
        node(1, None, NodeKind::Project),
        node(2, None, NodeKind::Project),
    ])
    .unwrap();
    assert!(tree
        .validate_mappings(&[mapping(10, 1, 10), mapping(11, 2, 10)])
        .is_ok());
    let mut first = mapping(10, 1, 10);
    first.valid_until = None;
    assert!(matches!(
        tree.validate_mappings(&[first, mapping(11, 1, 11)]),
        Err(DomainError::OverlappingMappings(_, _))
    ));
    let mut invalid = mapping(12, 1, 10);
    invalid.valid_until = Some(invalid.valid_from);
    assert_eq!(
        tree.validate_mappings(&[invalid]),
        Err(DomainError::InvalidMapping(Uuid::from_u128(12)))
    );
}

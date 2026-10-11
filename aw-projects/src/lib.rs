//! Host-independent project tracking rules. Persistence and HTTP adapters live
//! in ActivityWatch's datastore and server; this crate has no executable.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Project,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    Inherited,
    Tm,
    Fixed,
    NonBillable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReportingPolicy {
    Inherited,
    #[serde(rename = "self")]
    SelfNode,
    Ancestor {
        target_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub kind: NodeKind,
    pub name: String,
    pub case_id: Option<String>,
    pub archived: bool,
    pub billing: BillingMode,
    pub reporting: ReportingPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityIdMapping {
    pub id: Uuid,
    pub owner_node_id: Uuid,
    pub valid_from: NaiveDate,
    /// Exclusive upper bound; None means no end date.
    pub valid_until: Option<NaiveDate>,
    pub activity_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    DuplicateId(Uuid),
    MissingNode(Uuid),
    Cycle(Uuid),
    DepthLimit,
    CrossWorkspace(Uuid),
    InvalidParent(Uuid),
    EmptyName(Uuid),
    InvalidReportingTarget(Uuid),
    InvalidMapping(Uuid),
    OverlappingMappings(Uuid, Uuid),
}

/// A validated snapshot. Reparenting requires validating the complete candidate
/// snapshot, including descendant policies, before committing a transaction.
#[derive(Debug)]
pub struct Hierarchy {
    nodes: HashMap<Uuid, Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPolicy {
    pub billing: BillingMode,
    pub billing_owner_id: Option<Uuid>,
    pub reporting_target_id: Uuid,
    pub reporting_owner_id: Option<Uuid>,
    pub effectively_archived: bool,
}

impl Hierarchy {
    pub fn new(nodes: impl IntoIterator<Item = Node>) -> Result<Self, DomainError> {
        let mut map = HashMap::new();
        for node in nodes {
            let id = node.id;
            if map.insert(id, node).is_some() {
                return Err(DomainError::DuplicateId(id));
            }
        }
        let tree = Self { nodes: map };
        for node in tree.nodes.values() {
            if node.name.trim().is_empty() {
                return Err(DomainError::EmptyName(node.id));
            }
            let path = tree.path(node.id)?;
            if node.parent_id.is_none() && node.kind != NodeKind::Project {
                return Err(DomainError::InvalidParent(node.id));
            }
            if let Some(parent_id) = node.parent_id {
                let parent = &tree.nodes[&parent_id];
                if parent.workspace_id != node.workspace_id {
                    return Err(DomainError::CrossWorkspace(node.id));
                }
                if parent.kind == NodeKind::Task && node.kind == NodeKind::Project {
                    return Err(DomainError::InvalidParent(node.id));
                }
            }
            if let ReportingPolicy::Ancestor { target_id } = node.reporting {
                if target_id == node.id || !path.iter().any(|n| n.id == target_id) {
                    return Err(DomainError::InvalidReportingTarget(node.id));
                }
            }
        }
        Ok(tree)
    }

    /// Assignment first, root last. No assumptions about names or CaseId.
    pub fn path(&self, id: Uuid) -> Result<Vec<&Node>, DomainError> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut current = Some(id);
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(DomainError::Cycle(id));
            }
            if result.len() >= MAX_DEPTH {
                return Err(DomainError::DepthLimit);
            }
            let node = self.nodes.get(&id).ok_or(DomainError::MissingNode(id))?;
            result.push(node);
            current = node.parent_id;
        }
        Ok(result)
    }

    pub fn resolve(&self, id: Uuid) -> Result<ResolvedPolicy, DomainError> {
        let path = self.path(id)?;
        let billing_owner = path.iter().find(|n| n.billing != BillingMode::Inherited);
        let reporting_owner = path
            .iter()
            .find(|n| n.reporting != ReportingPolicy::Inherited);
        let root = path.last().ok_or(DomainError::MissingNode(id))?;
        let target = match reporting_owner.map(|n| n.reporting) {
            Some(ReportingPolicy::SelfNode) => reporting_owner.unwrap().id,
            Some(ReportingPolicy::Ancestor { target_id }) => target_id,
            _ => root.id,
        };
        Ok(ResolvedPolicy {
            billing: billing_owner.map_or(BillingMode::NonBillable, |n| n.billing),
            billing_owner_id: billing_owner.map(|n| n.id),
            reporting_target_id: target,
            reporting_owner_id: reporting_owner.map(|n| n.id),
            effectively_archived: path.iter().any(|n| n.archived),
        })
    }

    pub fn validate_mappings(&self, mappings: &[ActivityIdMapping]) -> Result<(), DomainError> {
        let mut ids = HashSet::new();
        let mut by_owner: HashMap<Uuid, Vec<&ActivityIdMapping>> = HashMap::new();
        for mapping in mappings {
            if !ids.insert(mapping.id) {
                return Err(DomainError::DuplicateId(mapping.id));
            }
            if !self.nodes.contains_key(&mapping.owner_node_id) {
                return Err(DomainError::MissingNode(mapping.owner_node_id));
            }
            if mapping.activity_id.trim().is_empty()
                || mapping
                    .valid_until
                    .is_some_and(|end| end <= mapping.valid_from)
            {
                return Err(DomainError::InvalidMapping(mapping.id));
            }
            by_owner
                .entry(mapping.owner_node_id)
                .or_default()
                .push(mapping);
        }
        for group in by_owner.values_mut() {
            group.sort_by_key(|m| (m.valid_from, m.id));
            for pair in group.windows(2) {
                if pair[0]
                    .valid_until
                    .is_none_or(|end| end > pair[1].valid_from)
                {
                    return Err(DomainError::OverlappingMappings(pair[0].id, pair[1].id));
                }
            }
        }
        Ok(())
    }

    /// Caller supplies the work date after converting/slicing the UTC interval
    /// in the reporting timezone. Export date must not be passed here.
    pub fn activity_id<'a>(
        &self,
        assigned_id: Uuid,
        work_date: NaiveDate,
        mappings: &'a [ActivityIdMapping],
    ) -> Result<Option<&'a ActivityIdMapping>, DomainError> {
        self.validate_mappings(mappings)?;
        let target = self.resolve(assigned_id)?.reporting_target_id;
        Ok(mappings.iter().find(|m| {
            m.owner_node_id == target
                && m.valid_from <= work_date
                && m.valid_until.is_none_or(|end| work_date < end)
        }))
    }
}

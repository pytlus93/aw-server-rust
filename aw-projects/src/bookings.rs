//! Manual ledger intervals are independent of captured AW events.
use crate::Hierarchy;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Union overlapping/touching observations without filling disjoint gaps.
pub fn union(mut spans: Vec<Span>) -> Result<Vec<Span>, String> {
    if spans.is_empty() || spans.len() > 1000 {
        return Err("select 1 through 1000 intervals".into());
    }
    if spans
        .iter()
        .any(|s| s.end <= s.start || (s.end - s.start).num_nanoseconds().is_none())
    {
        return Err("intervals must have positive, representable duration".into());
    }
    spans.sort_by_key(|s| (s.start, s.end));
    let mut result: Vec<Span> = Vec::new();
    for span in spans {
        if let Some(last) = result.last_mut() {
            if span.start <= last.end {
                last.end = last.end.max(span.end);
                continue;
            }
        }
        result.push(span);
    }
    if result
        .iter()
        .any(|s| (s.end - s.start).num_nanoseconds().is_none())
    {
        return Err("merged interval duration is too large".into());
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub bucket_id: String,
    pub event_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub node_id: Uuid,
    pub description: String,
    pub intervals: Vec<Span>,
    pub sources: Vec<SourceRef>,
    pub deleted: bool,
    /// Server-derived exact nanoseconds as decimal string, never JS float.
    #[serde(default)]
    pub duration_ns: String,
}

impl Entry {
    pub fn normalize(&mut self) -> Result<(), String> {
        if self.description.len() > 16384
            || self.sources.len() > 1000
            || self.sources.iter().any(|s| {
                s.bucket_id.is_empty()
                    || s.bucket_id.len() > 1024
                    || s.event_id.is_some_and(|id| id < 0)
            })
        {
            return Err("invalid description or source references".into());
        }
        self.intervals = union(self.intervals.clone())?;
        self.duration_ns = self
            .intervals
            .iter()
            .map(|s| (s.end - s.start).num_nanoseconds().unwrap() as i128)
            .sum::<i128>()
            .to_string();
        Ok(())
    }
}

pub fn validate(tree: &Hierarchy, entries: &[Entry]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut intervals = Vec::new();
    for entry in entries {
        if !ids.insert(entry.id) {
            return Err("duplicate entry ID".into());
        }
        tree.resolve(entry.node_id)
            .map_err(|_| "unknown assignment node")?;
        let mut normalized = entry.clone();
        normalized.normalize()?;
        if normalized != *entry {
            return Err("noncanonical entry intervals or duration".into());
        }
        if !entry.deleted {
            intervals.extend(entry.intervals.iter().map(|s| (s.start, s.end, entry.id)));
        }
    }
    intervals.sort_by_key(|s| (s.0, s.1));
    for pair in intervals.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(format!(
                "booked time overlaps entries {} and {}",
                pair[0].2, pair[1].2
            ));
        }
    }
    Ok(())
}

/// Apply a batch atomically, used for edits, deletes and split (delete + two
/// replacements). Archived branches accept metadata/time corrections to an
/// existing assignment, but cannot receive new or reassigned entries.
pub fn apply(
    tree: &Hierarchy,
    existing: &[Entry],
    mut upsert: Vec<Entry>,
    delete_ids: &[Uuid],
) -> Result<Vec<Entry>, String> {
    if upsert.len() > 100 || delete_ids.len() > 100 {
        return Err("batch limit is 100 entries".into());
    }
    let mut touched = HashSet::new();
    let mut result = existing.to_vec();
    for id in delete_ids {
        if !touched.insert(*id) {
            return Err("duplicate batch entry ID".into());
        }
        let old = result
            .iter_mut()
            .find(|e| e.id == *id && !e.deleted)
            .ok_or("entry to delete does not exist")?;
        old.deleted = true;
    }
    for entry in &mut upsert {
        if entry.deleted || !touched.insert(entry.id) {
            return Err("invalid or duplicate batch entry ID".into());
        }
        entry.normalize()?;
        let old = existing.iter().find(|e| e.id == entry.id);
        if old.is_some_and(|e| e.deleted) {
            return Err("deleted IDs cannot be reused".into());
        }
        let archived = tree
            .resolve(entry.node_id)
            .map_err(|_| "unknown assignment node")?
            .effectively_archived;
        if archived && old.is_none_or(|e| e.node_id != entry.node_id) {
            return Err("cannot assign new work to an archived branch".into());
        }
        match result.iter_mut().find(|e| e.id == entry.id) {
            Some(old) => *old = entry.clone(),
            None => result.push(entry.clone()),
        }
    }
    validate(tree, &result)?;
    Ok(result)
}

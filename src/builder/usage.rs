//! Which fields of each SDE table the parser actually reads.
//!
//! [`super::parser::Parser::parse_data`] records every field it looks up
//! while it builds `sde.db`, table by table, as dotted paths
//! (`destination.solarSystemID`, `divisions.[].leaderID`). That set,
//! [`FieldUsage`], is what makes delta updates cheap and exact:
//!
//! - [`super::mirror`] keeps only these paths of each record, so the local
//!   copy of the SDE is a fraction of CCP's export and the parser still reads
//!   it unchanged.
//! - A delta line that only touches paths outside the set can't change
//!   `sde.db`, so it's dropped without rebuilding anything.
//!
//! Nothing here is maintained by hand: the set comes from the parser's own
//! code running against real data with the real [`super::parser::ParserConfig`]
//! (e.g. `position2D` is only read when the config trusts CCP's 2D
//! positions).
//!
//! ## How the recording works
//!
//! Every field lookup in the parser goes through [`field`]. While a
//! recording is active on the current thread, [`field`] knows the path of
//! the value it's looking into (registered by address when the record or
//! a parent field was looked up), so it can record the full path of the
//! field being read, and register the child's path in turn. Array
//! elements are registered as `<path>.[]`, so a lookup inside any element
//! is recorded the same way.
//!
//! A path is recorded when it's *looked up*, whether or not the record
//! has it: an optional field that no record carries today is still part
//! of the set, so a delta that adds it later is not dropped.
//!
//! Looking up `position` and then `position.x` records both; [`FieldUsage`]
//! prunes a path when a longer one below it was recorded too, so it means
//! "only these sub-fields", while a path with nothing recorded below it
//! (`name`, read by language with a fallback; `memberRaces`, read whole)
//! means "the whole subtree".

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The fields the parser reads, per SDE table (the `jsonl` file stem, e.g.
/// `mapSolarSystems`). See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldUsage {
    tables: BTreeMap<String, BTreeSet<String>>,
}

impl FieldUsage {
    /// Adds `path` (already normalized, see [`normalize_path`]) to `table`.
    pub fn insert(&mut self, table: &str, path: &str) {
        if let Some(paths) = self.tables.get_mut(table) {
            if !paths.contains(path) {
                paths.insert(path.to_string());
            }
        } else {
            self.tables
                .insert(table.to_string(), BTreeSet::from([path.to_string()]));
        }
    }

    /// Marks `table` as read even if no field of it was looked up (an empty
    /// file still has to exist for the parser to open it).
    pub fn insert_table(&mut self, table: &str) {
        self.tables.entry(table.to_string()).or_default();
    }

    /// Tables the parser reads, in name order.
    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.tables.keys().map(String::as_str)
    }

    /// Whether the parser reads `table` at all.
    pub fn uses_table(&self, table: &str) -> bool {
        self.tables.contains_key(table)
    }

    /// The paths read from `table`, if it's read at all.
    pub fn paths(&self, table: &str) -> Option<&BTreeSet<String>> {
        self.tables.get(table)
    }

    /// Whether a change at `path` (with or without list indices) of `table`
    /// can affect what the parser reads: some recorded path is the same as
    /// it, above it (its subtree is read whole) or below it (the change
    /// replaces a parent of something that's read).
    pub fn touches(&self, table: &str, path: &str) -> bool {
        let Some(paths) = self.tables.get(table) else {
            return false;
        };
        let path = normalize_path(path);
        let changed: Vec<&str> = path.split('.').collect();
        paths.iter().any(|used| {
            let used: Vec<&str> = used.split('.').collect();
            let shared = used.len().min(changed.len());
            used[..shared] == changed[..shared]
        })
    }

    /// Drops every path that has a longer recorded path below it -- see
    /// the module docs on what a remaining path means.
    pub(crate) fn prune(&mut self) {
        for paths in self.tables.values_mut() {
            let parents: Vec<String> = paths
                .iter()
                .filter(|path| {
                    let prefix = format!("{path}.");
                    paths.iter().any(|other| other.starts_with(&prefix))
                })
                .cloned()
                .collect();
            for parent in parents {
                paths.remove(&parent);
            }
        }
    }
}

/// Replaces list indices with `[]`: `messages.[5].en` -> `messages.[].en`,
/// the form [`FieldUsage`] stores and delta `schema` operations use.
pub fn normalize_path(path: &str) -> String {
    path.split('.')
        .map(|segment| {
            if segment.starts_with('[') && segment.ends_with(']') {
                "[]"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

struct Recorder {
    usage: FieldUsage,
    table: Option<String>,
    /// Path of every value of the current record registered so far, by
    /// address. Only valid while that record is alive: cleared by
    /// [`leave_record`].
    paths: HashMap<usize, String>,
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

fn address(value: &Value) -> usize {
    value as *const Value as usize
}

/// Starts recording on the current thread, discarding any recording in
/// progress.
pub(crate) fn start() {
    RECORDER.with(|recorder| {
        *recorder.borrow_mut() = Some(Recorder {
            usage: FieldUsage::default(),
            table: None,
            paths: HashMap::new(),
        });
    });
}

/// Stops recording and returns what was recorded (`None` if no recording
/// was active), pruned.
pub(crate) fn finish() -> Option<FieldUsage> {
    RECORDER.with(|recorder| {
        recorder.borrow_mut().take().map(|recorder| {
            let mut usage = recorder.usage;
            usage.prune();
            usage
        })
    })
}

/// Marks `table` as read, without a record (see [`FieldUsage::insert_table`]).
pub(crate) fn open_table(table: &str) {
    RECORDER.with(|recorder| {
        if let Some(recorder) = recorder.borrow_mut().as_mut() {
            recorder.usage.insert_table(table);
        }
    });
}

/// Makes `record` (a whole line of `table`) the root of later lookups.
pub(crate) fn enter_record(table: &str, record: &Value) {
    RECORDER.with(|recorder| {
        if let Some(recorder) = recorder.borrow_mut().as_mut() {
            if recorder.table.as_deref() != Some(table) {
                recorder.table = Some(table.to_string());
            }
            recorder.paths.clear();
            recorder.paths.insert(address(record), String::new());
        }
    });
}

/// Forgets the current record's addresses: they're about to be freed.
pub(crate) fn leave_record() {
    RECORDER.with(|recorder| {
        if let Some(recorder) = recorder.borrow_mut().as_mut() {
            recorder.paths.clear();
        }
    });
}

/// `value.get(name)`, recording the path of the field looked up when a
/// recording is active and `value` belongs to the current record. Every
/// field lookup of the parser goes through here.
pub(crate) fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    let child = value.get(name);
    RECORDER.with(|recorder| {
        let mut recorder = recorder.borrow_mut();
        let Some(recorder) = recorder.as_mut() else {
            return;
        };
        let Some(table) = recorder.table.clone() else {
            return;
        };
        let Some(prefix) = recorder.paths.get(&address(value)) else {
            return;
        };
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        };
        recorder.usage.insert(&table, &path);
        if let Some(child) = child {
            if let Value::Array(items) = child {
                let item_path = format!("{path}.[]");
                for item in items {
                    recorder.paths.insert(address(item), item_path.clone());
                }
            }
            recorder.paths.insert(address(child), path);
        }
    });
    child
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(table: &str, value: &Value, read: impl FnOnce(&Value)) {
        enter_record(table, value);
        read(value);
        leave_record();
    }

    #[test]
    fn records_top_level_nested_and_array_paths() {
        start();
        let value = json!({
            "_key": 1,
            "position": {"x": 1.0, "y": 2.0},
            "divisions": [{"_key": 3, "size": 1}, {"_key": 4, "size": 2}],
            "name": {"en": "A", "de": "B"}
        });
        record("t", &value, |value| {
            field(value, "_key");
            let position = field(value, "position").unwrap();
            field(position, "x");
            if let Some(Value::Array(items)) = field(value, "divisions") {
                for item in items {
                    field(item, "size");
                }
            }
            field(value, "name");
            field(value, "missing");
        });
        let usage = finish().unwrap();
        let paths: Vec<&str> = usage
            .paths("t")
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect();
        // `position` and `divisions` are pruned: their sub-fields say
        // exactly what's read.
        assert_eq!(
            paths,
            ["_key", "divisions.[].size", "missing", "name", "position.x"]
        );
    }

    #[test]
    fn lookups_outside_a_recording_record_nothing() {
        let value = json!({"a": 1});
        assert_eq!(field(&value, "a"), Some(&json!(1)));
        assert!(finish().is_none());
    }

    #[test]
    fn values_not_reached_from_the_record_are_not_recorded() {
        start();
        let value = json!({"a": 1});
        let other = json!({"b": 2});
        record("t", &value, |_| {
            field(&other, "b");
        });
        let usage = finish().unwrap();
        assert!(usage.paths("t").is_none());
    }

    #[test]
    fn open_table_marks_a_table_without_fields() {
        start();
        open_table("empty");
        let usage = finish().unwrap();
        assert!(usage.uses_table("empty"));
        assert!(usage.paths("empty").unwrap().is_empty());
    }

    #[test]
    fn touches_matches_the_same_path_its_parents_and_its_children() {
        let mut usage = FieldUsage::default();
        usage.insert("t", "statistics.locked");
        usage.insert("t", "name");
        usage.insert("t", "divisions.[].size");
        assert!(usage.touches("t", "statistics.locked"));
        assert!(usage.touches("t", "statistics"));
        assert!(!usage.touches("t", "statistics.pressure"));
        assert!(usage.touches("t", "name.de"));
        assert!(usage.touches("t", "divisions.[3].size"));
        assert!(!usage.touches("t", "divisions.[3].leaderID"));
        assert!(!usage.touches("other", "name"));
    }

    #[test]
    fn normalize_path_replaces_list_indices() {
        assert_eq!(normalize_path("messages.[5].en"), "messages.[].en");
        assert_eq!(normalize_path("a.b"), "a.b");
        assert_eq!(normalize_path("list.[]"), "list.[]");
    }
}

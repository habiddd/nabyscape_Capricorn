//! Task list model and rules (C# `TaskItem`, `AddTask`, `RemoveTask`, `Normalize`).

use super::notes::bound;
use super::{new_id, null_default};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_TASKS: usize = 100;
pub const MAX_TASK_TEXT: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TaskItem {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    pub done: bool,
}

impl Default for TaskItem {
    fn default() -> Self {
        Self {
            id: new_id(),
            text: String::new(),
            done: false,
        }
    }
}

/// Adds a trimmed task; false for blank text or when the list is full.
pub fn add_task(tasks: &mut Vec<TaskItem>, text: &str) -> bool {
    if text.trim().is_empty() || tasks.len() >= MAX_TASKS {
        return false;
    }
    tasks.push(TaskItem {
        text: bound(text.trim(), MAX_TASK_TEXT),
        ..Default::default()
    });
    true
}

/// True when something was removed.
pub fn remove_task(tasks: &mut Vec<TaskItem>, id: &str) -> bool {
    let before = tasks.len();
    tasks.retain(|t| t.id != id);
    tasks.len() != before
}

/// Drops blank tasks, caps at 100, bounds text and makes ids unique and non-empty.
pub fn normalize(tasks: &mut Vec<TaskItem>) {
    tasks.retain(|t| !t.text.trim().is_empty());
    tasks.truncate(MAX_TASKS);
    let mut seen = HashSet::new();
    for task in tasks.iter_mut() {
        if task.id.trim().is_empty() || !seen.insert(task.id.clone()) {
            task.id = new_id();
            seen.insert(task.id.clone());
        }
        task.text = bound(&task.text, MAX_TASK_TEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_are_capped_and_bounded() {
        let mut tasks = Vec::new();
        assert!(!add_task(&mut tasks, " "));
        assert!(add_task(&mut tasks, &"t".repeat(21_000)));
        assert_eq!(tasks[0].text.len(), 20_000);
        for i in 1..100 {
            assert!(add_task(&mut tasks, &format!("Task {i}")));
        }
        assert!(!add_task(&mut tasks, "overflow"));
        assert_eq!(tasks.len(), 100);
    }

    #[test]
    fn add_trims_and_remove_by_id() {
        let mut tasks = Vec::new();
        assert!(add_task(&mut tasks, "  tugas  "));
        assert_eq!(tasks[0].text, "tugas");
        assert_eq!(tasks[0].id.len(), 32);
        let id = tasks[0].id.clone();
        assert!(!remove_task(&mut tasks, "nope"));
        assert!(remove_task(&mut tasks, &id));
        assert!(tasks.is_empty());
    }

    #[test]
    fn normalize_dedupes_ids_and_drops_blank() {
        let mut tasks = vec![
            TaskItem {
                id: "same".into(),
                text: "first".into(),
                done: false,
            },
            TaskItem {
                id: "same".into(),
                text: "second".into(),
                done: false,
            },
            TaskItem {
                id: "x".into(),
                text: "".into(),
                done: false,
            },
            TaskItem {
                id: "".into(),
                text: "third".into(),
                done: true,
            },
        ];
        normalize(&mut tasks);
        assert_eq!(tasks.len(), 3);
        let ids: HashSet<_> = tasks.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(tasks[0].id, "same");
    }
}

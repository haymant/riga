//! Durable task tree for a run.
//!
//! A run is an orchestrator task; `task` tool dispatches append child tasks.
//! The tree is the durable record of what was delegated and how it ended, and
//! the source for the SubagentList / TaskCard UI. It is transport-free: the
//! server journals it and the UI renders it, but the shape lives here so both
//! share one definition and one set of invariants.

use serde::{Deserialize, Serialize};

/// Lifecycle of a single task. `WaitingForApproval` is a first-class state so
/// the UI can show work blocked on the user without treating it as running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    /// A task in a terminal state will not change again.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// One task in the tree. `agent` names the profile (`explore`, `plan`, `build`,
/// `review`, or the root orchestrator), `model` records which model ran it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent: String,
    pub description: String,
    pub model: String,
    pub state: TaskState,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Limits that keep a runaway tree from exhausting the process or budget.
pub const MAX_TASK_DEPTH: usize = 2;
pub const MAX_TASK_FANOUT: usize = 4;

/// The task tree for one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskTree {
    tasks: Vec<TaskRecord>,
    next_id: u64,
}

impl TaskTree {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    /// Start the root (orchestrator) task and return its id.
    pub fn root(&mut self, agent: &str, description: &str, model: &str) -> String {
        let id = self.mint_id();
        self.tasks.push(TaskRecord {
            id: id.clone(),
            parent_id: None,
            agent: agent.to_owned(),
            description: description.to_owned(),
            model: model.to_owned(),
            state: TaskState::Running,
            started_at: "now".into(),
            result: None,
        });
        id
    }

    /// Dispatch a child task, enforcing the depth and fan-out limits.
    ///
    /// Returns the new task id, or `Err` with an actionable message when the
    /// parent is unknown, too deep, or already has `MAX_TASK_FANOUT` children.
    pub fn spawn(
        &mut self,
        parent_id: &str,
        agent: &str,
        description: &str,
        model: &str,
    ) -> Result<String, String> {
        let depth = self
            .depth(parent_id)
            .ok_or_else(|| format!("unknown parent task `{parent_id}`"))?;
        if depth >= MAX_TASK_DEPTH {
            return Err(format!(
                "task nesting is limited to {MAX_TASK_DEPTH} levels; refusing to spawn `{agent}`"
            ));
        }
        let children = self
            .tasks
            .iter()
            .filter(|task| task.parent_id.as_deref() == Some(parent_id))
            .count();
        if children >= MAX_TASK_FANOUT {
            return Err(format!(
                "a task may dispatch at most {MAX_TASK_FANOUT} subagents at once; `{parent_id}` already has {children}"
            ));
        }
        let id = self.mint_id();
        self.tasks.push(TaskRecord {
            id: id.clone(),
            parent_id: Some(parent_id.to_owned()),
            agent: agent.to_owned(),
            description: description.to_owned(),
            model: model.to_owned(),
            state: TaskState::Pending,
            started_at: "now".into(),
            result: None,
        });
        Ok(id)
    }

    /// Move a task to a new state, attaching a result when it settles.
    ///
    /// A terminal task is immutable: a late completion cannot overwrite a
    /// cancellation, which is what makes replay safe.
    pub fn set_state(&mut self, id: &str, state: TaskState) -> Result<(), String> {
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| format!("unknown task `{id}`"))?;
        if task.state.is_terminal() {
            return Err(format!("task `{id}` already settled as {:?}", task.state));
        }
        task.state = state;
        Ok(())
    }

    /// Settle a task with its result. Cancellation wins over completion: a task
    /// that was cancelled before its result arrived stays cancelled.
    pub fn settle(&mut self, id: &str, ok: bool, result: &str) -> Result<(), String> {
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| format!("unknown task `{id}`"))?;
        if task.state == TaskState::Cancelled {
            return Ok(());
        }
        task.state = if ok {
            TaskState::Completed
        } else {
            TaskState::Failed
        };
        task.result = Some(result.to_owned());
        Ok(())
    }

    pub fn record(&self, id: &str) -> Option<&TaskRecord> {
        self.tasks.iter().find(|task| task.id == id)
    }

    pub fn children(&self, id: &str) -> Vec<&TaskRecord> {
        self.tasks
            .iter()
            .filter(|task| task.parent_id.as_deref() == Some(id))
            .collect()
    }

    pub fn all(&self) -> &[TaskRecord] {
        &self.tasks
    }

    /// Depth of `id`, where the root is depth 0. `None` when unknown.
    pub fn depth(&self, id: &str) -> Option<usize> {
        let mut cursor = self.record(id)?;
        let mut depth = 0;
        while let Some(parent) = cursor.parent_id.as_deref() {
            depth += 1;
            cursor = self.record(parent)?;
        }
        Some(depth)
    }

    fn mint_id(&mut self) -> String {
        let id = format!("task-{}", self.next_id);
        self.next_id += 1;
        id
    }
}

/// A step in an agent plan, mirroring the assistant-ui AgentPlan element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The plan the agent is working through. Rewritten as progress is made, so the
/// latest value is what the UI shows and the journal keeps the trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    pub steps: Vec<PlanStep>,
    pub active_index: usize,
}

/// Status of one todo row, mirroring the assistant-ui TodoList statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    Active,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The agent's live working list, rewritten mid-run as work is discovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub revision: u64,
    pub items: Vec<TodoItem>,
}

impl TodoList {
    /// Done items over items that have not left the plan. A failed item counts
    /// toward the denominator but not the numerator; a cancelled item counts
    /// toward neither, matching the element's ratio.
    pub fn progress(&self) -> (usize, usize) {
        let done = self
            .items
            .iter()
            .filter(|item| item.status == TodoStatus::Done)
            .count();
        let total = self
            .items
            .iter()
            .filter(|item| item.status != TodoStatus::Cancelled)
            .count();
        (done, total)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TASK_DEPTH, MAX_TASK_FANOUT, TaskState, TaskTree, TodoItem, TodoList, TodoStatus,
    };

    #[test]
    fn spawn_builds_a_tree_with_depth_and_parent_links() {
        let mut tree = TaskTree::new();
        let root = tree.root("orchestrator", "build a service", "gpt-5-nano");
        let child = tree
            .spawn(&root, "explore", "find the entry point", "haiku")
            .unwrap();
        let grandchild = tree
            .spawn(&child, "explore", "read one file", "haiku")
            .unwrap();

        assert_eq!(tree.depth(&root), Some(0));
        assert_eq!(tree.depth(&child), Some(1));
        assert_eq!(tree.depth(&grandchild), Some(2));
        assert_eq!(tree.children(&root).len(), 1);
        assert!(tree.children(&root)[0].parent_id.as_deref() == Some(root.as_str()));
    }

    #[test]
    fn spawn_refuses_deeper_than_the_limit() {
        let mut tree = TaskTree::new();
        let mut id = tree.root("orchestrator", "root", "m");
        for _ in 0..MAX_TASK_DEPTH {
            id = tree.spawn(&id, "explore", "child", "m").unwrap();
        }
        let error = tree.spawn(&id, "explore", "too deep", "m").unwrap_err();
        assert!(error.contains("nesting is limited"), "{error}");
    }

    #[test]
    fn spawn_refuses_more_than_fanout_children() {
        let mut tree = TaskTree::new();
        let root = tree.root("orchestrator", "root", "m");
        for index in 0..MAX_TASK_FANOUT {
            tree.spawn(&root, "explore", &format!("child {index}"), "m")
                .unwrap();
        }
        let error = tree
            .spawn(&root, "explore", "one too many", "m")
            .unwrap_err();
        assert!(error.contains("at most"), "{error}");
    }

    #[test]
    fn settling_is_idempotent_and_cancellation_wins() {
        let mut tree = TaskTree::new();
        let root = tree.root("orchestrator", "root", "m");
        let child = tree.spawn(&root, "build", "edit a file", "m").unwrap();

        tree.set_state(&child, TaskState::Cancelled).unwrap();
        // A late result must not resurrect a cancelled task.
        tree.settle(&child, true, "wrote file").unwrap();
        assert_eq!(tree.record(&child).unwrap().state, TaskState::Cancelled);

        tree.settle(&root, true, "done").unwrap();
        assert_eq!(tree.record(&root).unwrap().state, TaskState::Completed);
        assert_eq!(tree.record(&root).unwrap().result.as_deref(), Some("done"));
    }

    #[test]
    fn unknown_tasks_are_rejected() {
        let mut tree = TaskTree::new();
        assert!(tree.spawn("nope", "explore", "x", "m").is_err());
        assert!(tree.set_state("nope", TaskState::Running).is_err());
    }

    #[test]
    fn todo_progress_counts_failed_but_not_cancelled() {
        let list = TodoList {
            title: None,
            revision: 1,
            items: vec![
                TodoItem {
                    id: "1".into(),
                    text: "done".into(),
                    description: None,
                    status: TodoStatus::Done,
                    reason: None,
                },
                TodoItem {
                    id: "2".into(),
                    text: "failed".into(),
                    description: None,
                    status: TodoStatus::Failed,
                    reason: Some("timed out".into()),
                },
                TodoItem {
                    id: "3".into(),
                    text: "cancelled".into(),
                    description: None,
                    status: TodoStatus::Cancelled,
                    reason: None,
                },
            ],
        };
        // Two settled rows, one done: `1/2`, matching the element's ratio.
        assert_eq!(list.progress(), (1, 2));
    }
}

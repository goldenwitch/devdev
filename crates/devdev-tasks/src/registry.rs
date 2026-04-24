//! Task registry: stores active tasks, tracks state, serialization.

use std::collections::HashMap;

use crate::task::{Task, TaskError, TaskStatus};

/// Factory function for deserializing tasks from checkpoint.
pub type TaskFactory =
    Box<dyn Fn(serde_json::Value) -> Result<Box<dyn Task>, TaskError> + Send + Sync>;

/// Registry of task factories, keyed by task type string.
pub struct TaskFactories {
    factories: HashMap<String, TaskFactory>,
}

impl TaskFactories {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register(&mut self, task_type: &str, factory: TaskFactory) {
        self.factories.insert(task_type.to_string(), factory);
    }

    pub fn get(&self, task_type: &str) -> Option<&TaskFactory> {
        self.factories.get(task_type)
    }
}

impl Default for TaskFactories {
    fn default() -> Self {
        Self::new()
    }
}

/// Stores active tasks and tracks their state.
pub struct TaskRegistry {
    tasks: HashMap<String, Box<dyn Task>>,
    next_id: u64,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            next_id: 1,
        }
    }

    /// Add a task, return its ID.
    pub fn add(&mut self, task: Box<dyn Task>) -> String {
        let id = task.id().to_string();
        self.tasks.insert(id.clone(), task);
        // Bump next_id past any numeric suffix.
        if let Some(num) = id.strip_prefix("t-").and_then(|s| s.parse::<u64>().ok())
            && num >= self.next_id
        {
            self.next_id = num + 1;
        }
        id
    }

    /// Generate a fresh task ID.
    pub fn next_id(&mut self) -> String {
        let id = format!("t-{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Cancel a task by ID.
    pub fn cancel(&mut self, id: &str) -> Result<(), TaskError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;

        if task.status().is_terminal() {
            return Err(TaskError::AlreadyCancelled(id.to_string()));
        }

        task.set_status(TaskStatus::Cancelled);
        Ok(())
    }

    /// Get task by ID.
    pub fn get(&self, id: &str) -> Option<&dyn Task> {
        self.tasks.get(id).map(|t| t.as_ref())
    }

    /// Get mutable task by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Box<dyn Task>> {
        self.tasks.get_mut(id)
    }

    /// List all tasks.
    pub fn list(&self) -> Vec<&dyn Task> {
        self.tasks.values().map(|t| t.as_ref()).collect()
    }

    /// Number of tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Serialize all tasks for checkpoint.
    pub fn serialize(&self) -> Result<serde_json::Value, TaskError> {
        let mut entries = Vec::new();
        for task in self.tasks.values() {
            let state = task.serialize()?;
            entries.push(serde_json::json!({
                "type": task.task_type(),
                "id": task.id(),
                "state": state,
            }));
        }
        Ok(serde_json::json!({
            "next_id": self.next_id,
            "tasks": entries,
        }))
    }

    /// Deserialize tasks from checkpoint.
    pub fn deserialize(
        data: &serde_json::Value,
        factories: &TaskFactories,
    ) -> Result<Self, TaskError> {
        let next_id = data["next_id"].as_u64().unwrap_or(1);

        let mut tasks = HashMap::new();

        if let Some(arr) = data["tasks"].as_array() {
            for entry in arr {
                let task_type = entry["type"]
                    .as_str()
                    .ok_or_else(|| TaskError::Serialization("missing task type".into()))?;
                let state = &entry["state"];

                let factory = factories.get(task_type).ok_or_else(|| {
                    TaskError::Serialization(format!("unknown task type: {task_type}"))
                })?;

                let task = factory(state.clone())?;
                tasks.insert(task.id().to_string(), task);
            }
        }

        Ok(Self { tasks, next_id })
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

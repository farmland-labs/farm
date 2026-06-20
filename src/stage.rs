//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashMap;
use std::sync::Arc;
use crate::task::Task;
use crate::vendor::parse::EnvValue;

/// The `Stage` is a single coherent unit of work that can be executed independently or as part of a larger plan.
/// It is thread-safe, allowing it to be shared across multiple threads for concurrent execution.
/// It contains metadata about the stage, its dependencies, and the tasks to be executed.
/// Each stage can have multiple tasks, which are executed in the order they are added.
/// Stages can also have dependencies on other stages, which must be resolved before execution.
#[derive(Debug, Clone)]
pub struct Stage {
    pub label: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub declared_env: HashMap<String, EnvValue>,
    pub depends: Vec<String>,        // Dependencies
    pub tasks: Vec<Arc<Box<dyn Task>>>,
}

impl Stage {

    /// `get_depends` returns the dependencies of this stage.
    pub fn get_depends(&self) -> Vec<String> {
        self.depends.clone()
    }

    /// `get_tasks` returns the tasks of this stage.
    pub fn get_tasks(&self) -> Vec<Arc<Box<dyn Task>>> {
        self.tasks.clone()
    }

    /// `add_task` adds a new task to this stage.
    pub fn add_task<T>(&mut self, task: T)
    where 
        T: Task + 'static
    {
        self.tasks.push(Arc::new(Box::new(task)));
    }

    /// Create a new stage with the given label.
    pub fn new(label: String) -> Self {
        Self {
            label,
            inputs: Vec::new(),
            outputs: Vec::new(),
            declared_env: HashMap::new(),
            depends: Vec::new(),
            tasks: Vec::new(),
        }
    }

}

impl PartialEq for Stage {
    fn eq(&self, other: &Self) -> bool {
        // Compare all fields except tasks
        self.label == other.label &&
        self.inputs == other.inputs &&
        self.outputs == other.outputs &&
        self.depends == other.depends &&
        // Compare tasks by their count and labels
        self.tasks.len() == other.tasks.len() &&
        self.tasks.iter().zip(other.tasks.iter())
            .all(|(a, b)| a.label() == b.label())
    }
}


#[cfg(test)]
mod tests {
    use crate::task::{Callable, Context, TaskExecutionResult};

    use super::*;

    // Mock task for testing
    #[derive(Debug)]
    struct MockTask {
        name: String,
    }

    impl MockTask {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    impl Callable for MockTask {

        fn run(&self, _ctx: &Context) -> TaskExecutionResult {
            // In a real implementation, we'd use Arc<Mutex<bool>> for ran
            // but for test purposes this is sufficient
            println!("Running mock task: {}", self.name);
            TaskExecutionResult {
                success: true,
                stdout: format!("Mock task {} executed", self.name),
                stderr: String::new(),
                exit_code: Some(0),
            }
        }

        fn label(&self) -> String {
            self.name.clone()
        }
    }

    // MockTask automatically implements Task through the blanket implementation

    #[test]
    fn test_stage_new() {
        let stage = Stage::new("test_stage".to_string());
        assert_eq!(stage.label, "test_stage");
        assert!(stage.inputs.is_empty());
        assert!(stage.outputs.is_empty());
        assert!(stage.depends.is_empty());
        assert!(stage.tasks.is_empty());
    }

    #[test]
    fn test_stage_add_task() {
        let mut stage = Stage::new("test_stage".to_string());
        let task = MockTask::new("mock_task");
        stage.add_task(task);
        
        assert_eq!(stage.tasks.len(), 1);
        assert_eq!(stage.tasks[0].label(), "mock_task");
    }

    #[test]
    fn test_stage_clone() {
        let mut stage = Stage::new("test_stage".to_string());
        stage.inputs.push("input.txt".to_string());
        stage.outputs.push("output.txt".to_string());
        stage.depends.push("other_stage".to_string());
        stage.add_task(MockTask::new("mock_task"));

        let cloned = stage.clone();
        
        assert_eq!(cloned.label, stage.label);
        assert_eq!(cloned.inputs, stage.inputs);
        assert_eq!(cloned.outputs, stage.outputs);
        assert_eq!(cloned.depends, stage.depends);
        assert_eq!(cloned.tasks.len(), stage.tasks.len());
        assert_eq!(cloned.tasks[0].label(), stage.tasks[0].label());
    }

    #[test]
    fn test_stage_partial_eq() {
        let mut stage1 = Stage::new("test_stage".to_string());
        stage1.add_task(MockTask::new("task1"));
        stage1.add_task(MockTask::new("task2"));

        let mut stage2 = Stage::new("test_stage".to_string());
        stage2.add_task(MockTask::new("task1"));
        stage2.add_task(MockTask::new("task2"));

        // Stages with same structure and task labels should be equal
        assert_eq!(stage1, stage2);

        // Test inequality cases
        let mut stage3 = stage1.clone();
        stage3.inputs.push("different_input.txt".to_string());
        assert_ne!(stage1, stage3, "Stages with different inputs should not be equal");

        let mut stage4 = stage1.clone();
        stage4.add_task(MockTask::new("task3"));
        assert_ne!(stage1, stage4, "Stages with different number of tasks should not be equal");

        let mut stage5 = stage1.clone();
        stage5.tasks.clear();
        stage5.add_task(MockTask::new("different_task"));
        stage5.add_task(MockTask::new("task2"));
        assert_ne!(stage1, stage5, "Stages with different task labels should not be equal");
    }
}

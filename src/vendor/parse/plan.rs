//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Plan definitions for Farmland parser
//! 
//! Plans represent the complete build configuration with operations and dependencies.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::{toposort, is_cyclic_directed};
use std::fmt;
use colored::*;

use crate::vendor::parse::task::Task;
pub use crate::vendor::parse::parser::EnvValue;

/// Global cache configuration from `[cache]` section
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CacheConfig {
    /// Maximum local cache size (e.g., "10GB")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<String>,
    
    /// TTL for cache entries (e.g., "30d")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
    
    /// Environment variable names that influence cache key
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_key: Vec<String>,
    
    /// Commands whose output is hashed into cache key
    /// Key = env var name, Value = command to run
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env_command: HashMap<String, String>,
}

/// Operation represents a single build operation with its configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Operation {
    /// The label/name of this operation
    pub label: String,
    
    /// The tasks to execute for this operation
    pub tasks: Vec<Task>,
    
    /// Dependencies on other operations (by label)
    pub depends: Vec<String>,
    
    /// Input file patterns for cache key computation (e.g., "src/**", "Makefile")
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    
    /// Output file patterns (e.g., ".build/**")
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    
    /// Declared environment for cache key computation.
    /// - `EnvValue::Explicit(value)` = use this exact value for cache key
    /// - `EnvValue::Capture` = capture from current env at build time (logged)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub declared_env: HashMap<String, EnvValue>,
    
    /// Plugin metadata from //! plugin: key=value comments
    pub plugin_metadata: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

impl Operation {
    /// Create a new operation with the given label
    pub fn new<S: Into<String>>(label: S) -> Self {
        Self {
            label: label.into(),
            tasks: Vec::new(),
            depends: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            declared_env: HashMap::new(),
            plugin_metadata: std::collections::HashMap::new(),
        }
    }

    /// Helper function to format operation name with color support
    fn format_name(&self) -> String {
        if atty::is(atty::Stream::Stdout) {
            self.label.bold().to_string()
        } else {
            self.label.clone()
        }
    }
    
    /// Add a task to this operation
    pub fn with_task(mut self, task: Task) -> Self {
        self.tasks.push(task);
        self
    }
    
    /// Add a dependency to this operation
    pub fn with_dependency<S: Into<String>>(mut self, dep: S) -> Self {
        self.depends.push(dep.into());
        self
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tasks_str = if self.tasks.is_empty() {
            String::new()
        } else {
            format!(" ({} tasks)", self.tasks.len())
        };
        
        let deps_str = if self.depends.is_empty() {
            String::new()
        } else {
            format!(" -> {}", self.depends.join(", "))
        };
        
        write!(f, "{}{}{}", self.format_name(), tasks_str, deps_str)
    }
}

/// Plan represents the complete build configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Global environment variables
    pub globals: HashMap<String, String>,
    
    /// Available variants for this plan
    pub variants: Vec<String>,
    
    /// All operations in this plan
    pub operations: Vec<Operation>,
    
    /// User-facing goal names mapped to operation labels.
    /// Goals are the explicit entry points exposed to the UI/CLI.
    /// Example: { "build": "build_website", "test": "run_tests" }
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub goals: HashMap<String, String>,
    
    /// Global cache configuration from `[cache]` section
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheConfig>,
    
    /// Internal dependency graph (not serialized)
    #[serde(skip)]
    graph: DiGraph<String, ()>,
    
    /// Mapping from operation labels to graph node indices (not serialized)
    #[serde(skip)]
    node_map: HashMap<String, NodeIndex>,
}

impl Plan {
    /// Create a new empty plan
    pub fn new() -> Self {
        Self {
            globals: HashMap::new(),
            variants: Vec::new(),
            operations: Vec::new(),
            goals: HashMap::new(),
            cache: None,
            graph: DiGraph::new(),
            node_map: HashMap::new(),
        }
    }
    
    /// Add variants to this plan
    pub fn add_variants(&mut self, variants: Vec<String>) -> Result<(), String> {
        let new_variants: Vec<_> = variants.into_iter()
            .filter(|v| !self.variants.contains(v))
            .collect();
        self.variants.extend(new_variants);
        Ok(())
    }
    
    /// Add an operation to this plan
    pub fn add_operation(&mut self, operation: Operation) -> Result<(), String> {
        // Validate that the operation has at least one task
        if operation.tasks.is_empty() {
            return Err(format!("Operation '{}' must have at least one 'work:' field", operation.label));
        }
        
        // Check if operation with same label already exists
        for existing_operation in &self.operations {
            if existing_operation.label == operation.label {
                return Err(format!("Duplicate operation label: {}", operation.label));
            }
        }
        
        self.operations.push(operation);
        Ok(())
    }
    
    /// Build the dependency graph from the operations
    pub fn build_graph(&mut self) -> Result<(), String> {
        self.graph.clear();
        self.node_map.clear();
        
        // Add all operations as nodes
        for operation in &self.operations {
            let node = self.graph.add_node(operation.label.clone());
            self.node_map.insert(operation.label.clone(), node);
        }
        
        // Add edges for dependencies
        for operation in &self.operations {
            let operation_node = self.node_map[&operation.label];
            
            for dep in &operation.depends {
                if let Some(&dep_node) = self.node_map.get(dep) {
                    self.graph.add_edge(dep_node, operation_node, ());
                } else {
                    return Err(format!("Operation '{}' depends on unknown operation '{}'", operation.label, dep));
                }
            }
        }
        
        // Check for cycles
        if is_cyclic_directed(&self.graph) {
            return Err("Circular dependency detected in operation dependencies".to_string());
        }
        
        Ok(())
    }
    
    /// Get the dependency graph
    pub fn get_graph(&self) -> &DiGraph<String, ()> {
        &self.graph
    }
    
    /// Get operations in topological order (dependencies first)
    pub fn get_execution_order(&self) -> Result<Vec<&Operation>, String> {
        let topo_order = toposort(&self.graph, None)
            .map_err(|_| "Failed to compute topological order (circular dependency?)")?;
        
        let mut ordered_operations = Vec::new();
        for node in topo_order {
            let operation_label = &self.graph[node];
            
            // Find the operation that matches this node
            if let Some(operation) = self.operations.iter().find(|s| &s.label == operation_label) {
                ordered_operations.push(operation);
            }
        }
        
        Ok(ordered_operations)
    }
    
    /// Find an operation by label
    pub fn find_operation(&self, label: &str) -> Option<&Operation> {
        self.operations.iter().find(|s| s.label == label)
    }
    
    /// Get operations that have no dependencies (root operations)
    pub fn get_root_operations(&self) -> Vec<&Operation> {
        self.operations.iter()
            .filter(|operation| operation.depends.is_empty())
            .collect()
    }
    
    /// Get operations that depend on a given operation
    pub fn get_dependents(&self, operation_label: &str) -> Vec<&Operation> {
        self.operations.iter()
            .filter(|operation| operation.depends.contains(&operation_label.to_string()))
            .collect()
    }
    
    /// Pretty print the plan as a tree
    pub fn pretty_print_graph(&self) -> String {
        let mut output = String::new();
        output.push_str("📋 Farmland Plan:\n");
        
        if self.variants.is_empty() {
            output.push_str("   variants: none\n");
        } else {
            output.push_str(&format!("   variants: {}\n", self.variants.join(", ")));
        }
        
        output.push_str(&format!("   Operations: {}\n\n", self.operations.len()));
        
        // Add legend to explain the format
        output.push_str("📖 Legend: operation_name [variant] (task_count) → dependencies\n\n");
        
        // If dependency graph is available, show operations in dependency order
        if self.graph.node_count() > 0 {
            if let Ok(ordered_operations) = self.get_execution_order() {
                output.push_str("🔄 Execution Order:\n");
                for (i, operation) in ordered_operations.iter().enumerate() {
                    let prefix = if i == ordered_operations.len() - 1 { "└── " } else { "├── " };
                    output.push_str(&format!("{}{}\n", prefix, operation));
                }
                output.push('\n');
            }
        } else {
            // Fallback to simple list
            output.push_str("📋 Operations:\n");
            for operation in &self.operations {
                output.push_str(&format!("  • {}\n", operation));
            }
            output.push('\n');
        }
        
        output
    }
    
    /// Generate a DOT graph representation for visualization
    pub fn to_dot(&self) -> String {
        let mut dot = String::new();
        dot.push_str("digraph BuildPlan {\n");
        dot.push_str("  rankdir=TB;\n");
        dot.push_str("  node [shape=box, style=rounded];\n\n");
        
        // Add nodes for each operation
        for operation in &self.operations {
            dot.push_str(&format!("  \"{}\";\n", operation.label));
        }
        
        dot.push('\n');
        
        // Add edges for dependencies
        for operation in &self.operations {
            for dep in &operation.depends {
                dot.push_str(&format!("  \"{}\" -> \"{}\";\n", dep, operation.label));
            }
        }
        
        dot.push_str("}\n");
        dot
    }
    
    /// Pretty print with verbose details
    pub fn pretty_print_graph_verbose(&self) -> String {
        let mut output = self.pretty_print_graph();
        
        output.push_str("\n📝 Operation Details:\n");
        for operation in &self.operations {
            output.push_str(&format!("\n🎯 {}\n", operation.label));
            
            if !operation.depends.is_empty() {
                output.push_str(&format!("   Depends: {}\n", operation.depends.join(", ")));
            }
            
            if operation.tasks.is_empty() {
                output.push_str("   Tasks: none\n");
            } else {
                output.push_str(&format!("   Tasks ({}):\n", operation.tasks.len()));
                for (i, task) in operation.tasks.iter().enumerate() {
                    output.push_str(&format!("     {}. {}\n", i + 1, task.label()));
                }
            }
        }
        
        output
    }
}

impl Default for Plan {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::parse::task::Task;
    
    #[test]
    fn test_plan_creation() {
        let plan = Plan::new();
        assert!(plan.variants.is_empty());
        assert!(plan.operations.is_empty());
        assert_eq!(plan.graph.node_count(), 0);
    }
    
    #[test]
    fn test_add_variants() {
        let mut plan = Plan::new();
        plan.add_variants(vec!["debug".to_string(), "release".to_string()]).unwrap();
        
        assert_eq!(plan.variants, vec!["debug", "release"]);
        
        // Adding same variants should not duplicate
        plan.add_variants(vec!["debug".to_string(), "test".to_string()]).unwrap();
        assert_eq!(plan.variants, vec!["debug", "release", "test"]);
    }
    
    #[test]
    fn test_add_operations() {
        let mut plan = Plan::new();
        
        let mut prepare = Operation::new("prepare");
        prepare.tasks.push(Task::shell("echo preparing"));
        
        let mut build = Operation::new("build");
        build.depends.push("prepare".to_string());
        build.tasks.push(Task::shell("make build"));
        
        plan.add_operation(prepare).unwrap();
        plan.add_operation(build).unwrap();
        
        assert_eq!(plan.operations.len(), 2);
    }
    
    #[test]
    fn test_basic_plan_creation() {
        use crate::vendor::parse::task::Task;
        
        let mut plan = Plan::new();
        let mut prepare = Operation::new("prepare");
        prepare.tasks.push(Task::shell("echo preparing".to_string()));
        
        let mut build = Operation::new("build");
        build.depends.push("prepare".to_string());
        build.tasks.push(Task::shell("echo building".to_string()));
        
        plan.add_operation(prepare).unwrap();
        plan.add_operation(build).unwrap();
        plan.build_graph().unwrap();
        
        assert_eq!(plan.operations.len(), 2);
    }
    
    #[test]
    fn test_duplicate_operation_labels() {
        use crate::vendor::parse::task::Task;
        
        let mut plan = Plan::new();
        let mut operation1 = Operation::new("build");
        operation1.tasks.push(Task::shell("echo debug".to_string()));
        
        let mut operation2 = Operation::new("build");
        operation2.tasks.push(Task::shell("echo release".to_string()));
        
        plan.add_operation(operation1).unwrap();
        let result = plan.add_operation(operation2);
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Duplicate operation label"));
    }
    
    #[test]
    fn test_build_graph() {
        use crate::vendor::parse::task::Task;
        
        let mut plan = Plan::new();
        
        let mut prepare = Operation::new("prepare");
        prepare.tasks.push(Task::shell("echo prepare".to_string()));
        
        let mut build = Operation::new("build");
        build.depends.push("prepare".to_string());
        build.tasks.push(Task::shell("echo build".to_string()));
        
        let mut test = Operation::new("test");
        test.depends.push("build".to_string());
        test.tasks.push(Task::shell("echo test".to_string()));
        
        plan.add_operation(prepare).unwrap();
        plan.add_operation(build).unwrap();
        plan.add_operation(test).unwrap();
        
        plan.build_graph().unwrap();
        
        assert_eq!(plan.graph.node_count(), 3);
        assert_eq!(plan.graph.edge_count(), 2);
    }
    
    #[test]
    fn test_circular_dependency_detection() {
        use crate::vendor::parse::task::Task;
        
        let mut plan = Plan::new();
        
        let mut operation_a = Operation::new("a");
        operation_a.depends.push("b".to_string());
        operation_a.tasks.push(Task::shell("echo a".to_string()));
        
        let mut operation_b = Operation::new("b");
        operation_b.depends.push("a".to_string());
        operation_b.tasks.push(Task::shell("echo b".to_string()));
        
        plan.add_operation(operation_a).unwrap();
        plan.add_operation(operation_b).unwrap();
        
        let result = plan.build_graph();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Circular dependency"));
    }
    
    #[test]
    fn test_execution_order() {
        use crate::vendor::parse::task::Task;
        
        let mut plan = Plan::new();
        
        let mut prepare = Operation::new("prepare");
        prepare.tasks.push(Task::shell("echo prepare".to_string()));
        
        let mut build = Operation::new("build");
        build.depends.push("prepare".to_string());
        build.tasks.push(Task::shell("echo build".to_string()));
        
        let mut test = Operation::new("test");
        test.depends.push("build".to_string());
        test.tasks.push(Task::shell("echo test".to_string()));
        
        plan.add_operation(prepare).unwrap();
        plan.add_operation(build).unwrap();
        plan.add_operation(test).unwrap();
        plan.build_graph().unwrap();
        
        let order = plan.get_execution_order().unwrap();
        let labels: Vec<_> = order.iter().map(|s| &s.label).collect();
        
        assert_eq!(labels, vec!["prepare", "build", "test"]);
    }
}

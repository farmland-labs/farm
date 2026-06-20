//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Build Graph/DAG
//! Represents the build process as a directed acyclic graph (DAG).
//! Would need to represent the dependency tree (prepare -> build -> test)
//! Could use something like petgraph crate for dependency resolution
//! Validates that there are no circular dependencies

use std::collections::HashMap;
use petgraph::graph::{DiGraph, NodeIndex};
use crate::vendor::parse::CacheConfig;
pub use crate::{env::CapturedEnv, stage::Stage, task::{Task as RuntimeTask, Exec}};

pub type Variant = String;
pub type Label = String;
pub type Tag = String;
pub type Shell = String;


#[derive(Debug)]
pub struct Plan {
    pub env: CapturedEnv,
    pub globals: HashMap<String, String>,
    pub variants: Vec<Variant>,
    pub stages: Vec<Stage>,
    /// Goal name -> operation label mapping (for resolving goals to operations)
    goals: HashMap<String, String>,
    /// Operation label -> goal name mapping (for display)
    goals_by_operation: HashMap<String, String>,
    /// Global cache configuration from `[cache]` section
    pub cache: Option<CacheConfig>,
    graph: DiGraph<String, ()>,
}

impl Default for Plan {
    fn default() -> Self {
        Self::new()
    }
}

impl Plan {

    pub fn new() -> Self {
        Self {
            env: CapturedEnv::new(),
            globals: HashMap::new(),
            variants: Vec::new(),
            stages: Vec::new(),
            goals: HashMap::new(),
            goals_by_operation: HashMap::new(),
            cache: None,
            graph: DiGraph::new(),
        }
    }

    /// Convert from a parsed plan (from Farmfile) to a runtime execution plan
    pub fn from_parsed(parsed: crate::parser::ParsedPlan) -> Self {
        let mut plan = Self::new();
        
        plan.globals = parsed.globals;
        plan.variants = parsed.variants;
        plan.cache = parsed.cache;
        
        // Store both mappings for goal resolution and display
        plan.goals = parsed.goals.clone();
        for (goal_name, operation_label) in &parsed.goals {
            plan.goals_by_operation.insert(operation_label.clone(), goal_name.clone());
        }
        
        // Convert parsed operations to runtime stages
        for parsed_operation in parsed.operations {
            let mut stage = Stage::new(parsed_operation.label.clone());
            stage.depends = parsed_operation.depends;
            stage.inputs = parsed_operation.inputs;
            stage.outputs = parsed_operation.outputs;
            stage.declared_env = parsed_operation.declared_env;
            
            // Convert tasks
            for parsed_task in parsed_operation.tasks {
                match parsed_task {
                    crate::parser::ParsedTask::Shell(shell_task) => {
                        // Run shell commands through a shell to support variable expansion
                        // On Unix: /bin/sh -c "command"
                        // On Windows: cmd /C "command"
                        #[cfg(unix)]
                        let args = vec!["/bin/sh", "-c", &shell_task.command];
                        #[cfg(windows)]
                        let args = vec!["cmd", "/C", &shell_task.command];
                        
                        let exec = Exec::from(args);
                        stage.add_task(exec);
                    }
                }
            }
            
            // Add stage to plan (this will handle graph building)
            let _ = plan.add_stage(stage);
        }
        
        plan
    }

    pub fn get_graph(&self) -> &DiGraph<String, ()> {
        &self.graph
    }

    /// Adds new variants to the global variants list in the plan.
    pub fn add_variants(&mut self, variants: Vec<Variant>) -> Result<(), String> {
        let new_variants: Vec<_> = variants.into_iter()
            .filter(|v| !self.variants.contains(v))
            .collect();
        self.variants.extend(new_variants);
        Ok(())
    }

    /// Resolve a target string to an operation label.
    /// If target is a goal name, returns the corresponding operation label.
    /// Otherwise returns the target unchanged (assumed to be an operation label).
    pub fn resolve_target(&self, target: &str) -> String {
        if let Some(operation) = self.goals.get(target) {
            operation.clone()
        } else {
            target.to_string()
        }
    }

    /// Returns the number of goals defined in the plan
    pub fn goals_count(&self) -> usize {
        self.goals.len()
    }

    pub fn add_stage(&mut self, stage: Stage) -> Result<(), String> {
        // Check if stage with same label already exists
        for existing_stage in &self.stages {
            if existing_stage.label == stage.label {
                return Err(format!(
                    "Stage '{}' already exists", 
                    stage.label
                ));
            }
        }

        // Add node to graph if it doesn't exist yet
        let node_exists = self.graph.node_indices()
            .any(|i| self.graph[i] == stage.label);
        
        if !node_exists {
            self.graph.add_node(stage.label.clone());
        }

        // Add dependencies to graph
        let mut dependencies = stage.get_depends();
        
        // Add implicit root dependency if no explicit dependencies and not root stage itself
        if dependencies.is_empty() && stage.label != "root" {
            dependencies.push("root".to_string());
        }
        
        // Update the stage's depends field to include implicit dependencies
        let mut updated_stage = stage;
        if !dependencies.is_empty() && updated_stage.depends.is_empty() && updated_stage.label != "root" {
            updated_stage.depends = vec!["root".to_string()];
        }
        
        for dep in &dependencies {
            // Ensure dependency node exists
            let dep_node_exists = self.graph.node_indices()
                .any(|i| self.graph[i] == *dep);
            if !dep_node_exists {
                self.graph.add_node(dep.clone());
            }
            
            let from_node = self.graph.node_indices().find(|i| self.graph[*i] == *dep);
            let to_node = self.graph.node_indices().find(|i| self.graph[*i] == updated_stage.label);

            if let (Some(from_node), Some(to_node)) = (from_node, to_node) {
                // Only add edge if it doesn't already exist
                if self.graph.find_edge(from_node, to_node).is_none() {
                    self.graph.add_edge(from_node, to_node, ());
                }
            }
        }

        self.stages.push(updated_stage);
        Ok(())
    }

    /// Debug function to dump the plan graph with dependencies in a readable format
    pub fn debug_dump_graph(&self) {
        use crate::vendor::log::debug;
        
        debug!("=== Plan Graph Debug Dump ===");
        debug!("Stages: {}", self.stages.len());
        debug!("Graph nodes: {}", self.graph.node_count());
        debug!("Graph edges: {}", self.graph.edge_count());
        
        // Print all stages
        debug!("--- Stages ---");
        for (i, stage) in self.stages.iter().enumerate() {
            debug!("  {}: {} (depends: {:?})", 
                   i, stage.label, stage.depends);
        }
        
        // Print graph structure
        debug!("--- Dependencies Graph ---");
        for node_idx in self.graph.node_indices() {
            let node_name = &self.graph[node_idx];
            let dependencies: Vec<String> = self.graph
                .neighbors_directed(node_idx, petgraph::Direction::Incoming)
                .map(|dep_idx| self.graph[dep_idx].clone())
                .collect();
            
            let dependents: Vec<String> = self.graph
                .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
                .map(|dep_idx| self.graph[dep_idx].clone())
                .collect();
            
            if dependencies.is_empty() && dependents.is_empty() {
                debug!("  {} (no dependencies, no dependents)", node_name);
            } else {
                debug!("  {} <- {:?} -> {:?}", node_name, dependencies, dependents);
            }
        }
        
        // Print DOT format for visualization tools
        debug!("--- DOT Format (for visualization) ---");
        debug!("\n{:?}", petgraph::dot::Dot::with_config(&self.graph, &[petgraph::dot::Config::EdgeNoLabel]));
        
        debug!("=== End Graph Debug Dump ===");
    }

    /// Pretty print the plan graph in a tree-like format
    pub fn pretty_print_graph(&self) -> String {
        self.pretty_print_graph_colored(false)
    }
    
    /// Pretty print the plan graph in a tree-like format with optional colors
    pub fn pretty_print_graph_colored(&self, use_colors: bool) -> String {
        use std::collections::HashSet;
        
        let mut output = String::new();
        output.push_str("📋 Plan Dependency Graph:\n");
        
        // Find root nodes (nodes with no incoming edges)
        let mut roots = Vec::new();
        for node_idx in self.graph.node_indices() {
            let has_incoming = self.graph
                .neighbors_directed(node_idx, petgraph::Direction::Incoming)
                .next()
                .is_some();
            
            if !has_incoming {
                roots.push(node_idx);
            }
        }
        
        if roots.is_empty() {
            output.push_str("  (empty graph)\n");
            return output;
        }
        
        // Print tree starting from roots
        let mut visited = HashSet::new();
        for (i, root) in roots.iter().enumerate() {
            let is_last_root = i == roots.len() - 1;
            let root_connector = if is_last_root { 
                "└─ " 
            } else { 
                "├─ " 
            };
            let root_child_prefix = if is_last_root { 
                "   " 
            } else { 
                "│  " 
            };
            
            self.print_node_tree_with_prefix_colored(&mut output, *root, root_connector, root_child_prefix, &mut visited, use_colors);
        }
        
        output
    }
    
    fn print_node_tree_with_prefix_colored(&self, output: &mut String, node_idx: NodeIndex, prefix: &str, child_prefix: &str, visited: &mut std::collections::HashSet<NodeIndex>, use_colors: bool) {
        use colored::*;
        if visited.contains(&node_idx) {
            return;
        }
        visited.insert(node_idx);
        
        let node_name = &self.graph[node_idx];
        
        // Show goal name if this operation has a mapped goal (bold)
        let goal_suffix = self.goals_by_operation.get(node_name)
            .map(|goal| {
                if use_colors {
                    format!(" → {}", goal.yellow().bold())
                } else {
                    format!(" → {}", goal)
                }
            })
            .unwrap_or_default();
        
        output.push_str(&format!("{}{}{}\n", prefix, node_name, goal_suffix));
        
        // Print children (nodes that depend on this one)
        let children: Vec<_> = self.graph
            .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
            .collect();
        
        for (i, child) in children.iter().enumerate() {
            let is_last = i == children.len() - 1;
            let child_connector = if is_last { "└─ " } else { "├─ " };
            let child_child_prefix = if is_last { 
                format!("{}   ", child_prefix)
            } else { 
                format!("{}│  ", child_prefix)
            };
            
            self.print_node_tree_with_prefix_colored(
                output, 
                *child, 
                &format!("{}{}", child_prefix, child_connector),
                &child_child_prefix,
                visited,
                use_colors
            );
        }
    }

    pub fn pretty_print_graph_verbose(&self) -> String {
        self.pretty_print_graph_verbose_colored(true) // verbose always uses colors when available
    }
    
    pub fn pretty_print_graph_verbose_colored(&self, use_colors: bool) -> String {
        use std::collections::HashSet;
        
        let mut output = String::new();
        output.push_str("📋 Plan Dependency Graph (verbose):\n");
        
        // Find root nodes (nodes with no incoming edges)
        let mut roots = Vec::new();
        for node_idx in self.graph.node_indices() {
            let has_incoming = self.graph
                .neighbors_directed(node_idx, petgraph::Direction::Incoming)
                .next()
                .is_some();
            
            if !has_incoming {
                roots.push(node_idx);
            }
        }
        
        if roots.is_empty() {
            output.push_str("  (empty graph)\n");
            return output;
        }
        
        // Print tree starting from roots
        let mut visited = HashSet::new();
        for (i, root) in roots.iter().enumerate() {
            let is_last_root = i == roots.len() - 1;
            let root_connector = if is_last_root { 
                "└─ " 
            } else { 
                "├─ " 
            };
            let root_child_prefix = if is_last_root { 
                "   " 
            } else { 
                "│  " 
            };
            
            self.print_node_tree_verbose_with_prefix_colored(&mut output, *root, root_connector, root_child_prefix, &mut visited, use_colors);
        }
        
        output
    }
    
    fn print_node_tree_verbose_with_prefix_colored(&self, output: &mut String, node_idx: NodeIndex, prefix: &str, child_prefix: &str, visited: &mut std::collections::HashSet<NodeIndex>, use_colors: bool) {
        use colored::*;
        
        if visited.contains(&node_idx) {
            return;
        }
        visited.insert(node_idx);
        
        let node_name = &self.graph[node_idx];
        
        // Find stage info for this node
        let stage = self.stages.iter()
            .find(|s| s.label == *node_name);
        
        // Show goal name if this operation has a mapped goal (bold)
        let goal_suffix = self.goals_by_operation.get(node_name)
            .map(|goal| {
                if use_colors {
                    format!(" → {}", goal.bold())
                } else {
                    format!(" → {}", goal)
                }
            })
            .unwrap_or_default();
        
        let task_count = stage
            .map(|s| s.tasks.len())
            .unwrap_or(0);
        
        // Print stage header with task count
        output.push_str(&format!("{}{}{}  [{} tasks]\n", prefix, node_name, goal_suffix, task_count));
        
        // Print tasks for this stage (with proper tree prefix)
        if let Some(stage) = stage {
            for task in stage.tasks.iter() {
                let task_prefix = format!("{}│    ", child_prefix);
                let task_cmd = task.label();
                output.push_str(&format!("{}{}\n", task_prefix, task_cmd));
            }
        }
        
        // Print children (nodes that depend on this one)
        let children: Vec<_> = self.graph
            .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
            .collect();
        
        // Add connecting line to maintain tree structure if there were tasks above AND there are children
        if let Some(stage) = stage {
            if !stage.tasks.is_empty() && !children.is_empty() {
                // Show proper vertical line continuation before children
                output.push_str(&format!("{}│\n", child_prefix));
            }
        }
        
        for (i, child) in children.iter().enumerate() {
            let is_last = i == children.len() - 1;
            let child_connector = if is_last { "└─ " } else { "├─ " };
            let child_child_prefix = if is_last { 
                format!("{}   ", child_prefix)
            } else { 
                format!("{}│  ", child_prefix)
            };
            
            self.print_node_tree_verbose_with_prefix_colored(
                output, 
                *child, 
                &format!("{}{}", child_prefix, child_connector),
                &child_child_prefix,
                visited,
                use_colors
            );
        }
    }

}

impl std::fmt::Display for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plan with {} stages", self.stages.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_duplicate_stage_labels() {
        let mut plan = Plan::new();
        
        // Create two stages with same label
        let stage1 = Stage::new("build".to_string());
        let stage2 = Stage::new("build".to_string());
        
        // First should be added successfully, second should fail
        assert!(plan.add_stage(stage1).is_ok());
        let result = plan.add_stage(stage2);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
        
        assert_eq!(plan.stages.len(), 1);
    }

    #[test]
    fn test_add_stage_with_same_label() {
        let mut plan = Plan::new();
        
        // Create two stages with same label
        let stage1 = Stage::new("build".to_string());
        let stage2 = Stage::new("build".to_string());
        
        // First should be added successfully
        assert!(plan.add_stage(stage1).is_ok());
        
        // Second should fail due to duplicate label
        let result = plan.add_stage(stage2);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
        
        assert_eq!(plan.stages.len(), 1);
    }

    #[test]
    fn test_add_stages_with_different_labels() {
        let mut plan = Plan::new();
        
        // Create stages with different labels
        let stage1 = Stage::new("build".to_string());
        let stage2 = Stage::new("test".to_string());
        
        // Both should be added successfully (different labels)
        assert!(plan.add_stage(stage1).is_ok());
        assert!(plan.add_stage(stage2).is_ok());
        
        assert_eq!(plan.stages.len(), 2);
    }

    #[test]
    fn test_debug_graph_functions() {
        let mut plan = Plan::new();
        
        // Create a simple dependency chain: prepare -> build -> test
        let prepare = Stage::new("prepare".to_string());

        
        let mut build = Stage::new("build".to_string());

        build.depends = vec!["prepare".to_string()];
        
        let mut test = Stage::new("test".to_string());

        test.depends = vec!["build".to_string()];
        
        plan.add_stage(prepare).unwrap();
        plan.add_stage(build).unwrap();
        plan.add_stage(test).unwrap();
        
        // Test debug dump (this will only output if logging is enabled)
        plan.debug_dump_graph();
        
        // Test pretty print
        let graph_str = plan.pretty_print_graph();
        println!("Graph output:\n{}", graph_str);
        
        // Verify the graph contains our stages
        assert!(graph_str.contains("prepare"));
        assert!(graph_str.contains("build"));
        assert!(graph_str.contains("test"));
        // Variants are no longer displayed in graphs
    }

    #[test]
    fn test_debug_graph_with_logger() {
        // Initialize logger for this test
        let _ = env_logger::builder().is_test(true).try_init();
        
        let mut plan = Plan::new();
        
        // Create a more complex dependency scenario
        let prepare = Stage::new("prepare".to_string());

        
        let mut build = Stage::new("build".to_string());

        build.depends = vec!["prepare".to_string()];
        
        let mut test = Stage::new("test".to_string());

        test.depends = vec!["prepare".to_string(), "build".to_string()];
        
        plan.add_stage(prepare).unwrap();
        plan.add_stage(build).unwrap();
        plan.add_stage(test).unwrap();
        
        // This should output debug logs
        plan.debug_dump_graph();
        
        // Also test pretty print
        let graph_str = plan.pretty_print_graph();
        println!("Complete dependency graph:\n{}", graph_str);
        
        // Verify structure
        assert_eq!(plan.stages.len(), 3);
        assert!(graph_str.contains("prepare"));
    }

    #[test]
    fn test_pretty_print_complex_graph() {
        let mut plan = Plan::new();
        
        // Create a more complex dependency scenario with multiple branches
        let prepare = Stage::new("prepare".to_string());

        
        let mut build = Stage::new("build".to_string());

        build.depends = vec!["prepare".to_string()];
        
        let mut test = Stage::new("test".to_string());

        test.depends = vec!["build".to_string()];
        
        let mut package = Stage::new("package".to_string());

        package.depends = vec!["build".to_string()];
        
        let mut deploy = Stage::new("deploy".to_string());

        deploy.depends = vec!["package".to_string(), "test".to_string()];
        
        plan.add_stage(prepare).unwrap();
        plan.add_stage(build).unwrap();
        plan.add_stage(test).unwrap();
        plan.add_stage(package).unwrap();
        plan.add_stage(deploy).unwrap();
        
        let graph_str = plan.pretty_print_graph();
        println!("Complex dependency graph:\n{}", graph_str);
        
        // Verify the graph contains all our stages
        assert!(graph_str.contains("prepare"));
        assert!(graph_str.contains("build"));
        assert!(graph_str.contains("test"));
        assert!(graph_str.contains("package"));
        assert!(graph_str.contains("deploy"));
        // Variants are no longer displayed in graphs
    }

    #[test]
    fn test_pretty_print_multiple_roots() {
        let mut plan = Plan::new();
        
        // Create multiple root nodes to test tree formatting
        let prepare_a = Stage::new("prepare-a".to_string());

        
        let prepare_b = Stage::new("prepare-b".to_string());

        
        let mut build_a = Stage::new("build-a".to_string());

        build_a.depends = vec!["prepare-a".to_string()];
        
        let mut build_b = Stage::new("build-b".to_string());

        build_b.depends = vec!["prepare-b".to_string()];
        
        let mut final_stage = Stage::new("final".to_string());

        final_stage.depends = vec!["build-a".to_string(), "build-b".to_string()];
        
        plan.add_stage(prepare_a).unwrap();
        plan.add_stage(prepare_b).unwrap();
        plan.add_stage(build_a).unwrap();
        plan.add_stage(build_b).unwrap();
        plan.add_stage(final_stage).unwrap();
        
        let graph_str = plan.pretty_print_graph();
        println!("Multiple roots dependency graph:\n{}", graph_str);
        
        // Verify the graph contains all our stages
        assert!(graph_str.contains("prepare-a"));
        assert!(graph_str.contains("prepare-b"));
        assert!(graph_str.contains("build-a"));
        assert!(graph_str.contains("build-b"));
        assert!(graph_str.contains("final"));
    }
}

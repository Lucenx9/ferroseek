// SPDX-License-Identifier: GPL-3.0-or-later
//! Resolve an ordered workflow to a snapshot of registered executables.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Deserialize;

use super::Script;
use crate::matcher;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<String>,
}

impl Workflow {
    pub(super) fn validate(&self, titles: &mut HashSet<String>) -> Result<(), &'static str> {
        if self.title.is_empty() || self.title.chars().any(char::is_control) {
            return Err("Enter a single-line title.");
        }
        if !titles.insert(matcher::fold(&self.title)) {
            return Err("Use a different title for each workflow and command.");
        }
        if !(2..=8).contains(&self.steps.len()) {
            return Err("List two to eight commands.");
        }
        if self.steps.iter().any(|name| name.trim().is_empty() || name.trim().chars().any(char::is_control)) {
            return Err("Enter a command title for every step.");
        }
        Ok(())
    }
}

pub struct Step {
    pub script: Arc<Script>,
    arguments: Vec<usize>,
}

impl Step {
    pub(super) fn inputs(&self, values: &[String]) -> Vec<String> {
        self.arguments.iter().map(|&index| values[index].clone()).collect()
    }
}

pub struct Plan {
    pub title: String,
    pub parameters: Vec<String>,
    pub confirm: bool,
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn single(script: Arc<Script>) -> Self {
        let parameters = script.parameters.clone();
        let step = Step { arguments: (0..parameters.len()).collect(), script };
        Self { title: step.script.title.clone(), parameters, confirm: step.script.confirm, steps: vec![step] }
    }

    pub(super) fn workflow(workflow: &Workflow, commands: &[Arc<Script>]) -> Result<Self, String> {
        let commands: HashMap<_, _> = commands.iter().map(|script| (matcher::fold(&script.title), script)).collect();
        let mut positions = HashMap::new();
        let mut parameters = Vec::new();
        let mut steps = Vec::with_capacity(workflow.steps.len());
        for (index, title) in workflow.steps.iter().enumerate() {
            let script = commands
                .get(&matcher::fold(title.trim()))
                .ok_or_else(|| format!("Step {} needs the registered command “{}”.", index + 1, title.trim()))?;
            let arguments = script
                .parameters
                .iter()
                .map(|label| {
                    *positions.entry(matcher::fold(label.trim())).or_insert_with(|| {
                        parameters.push(label.trim().to_owned());
                        parameters.len() - 1
                    })
                })
                .collect();
            steps.push(Step { script: Arc::clone(script), arguments });
        }
        Ok(Self { title: workflow.title.clone(), parameters, confirm: true, steps })
    }

    pub fn is_workflow(&self) -> bool {
        self.steps.len() > 1
    }
}

#[cfg(test)]
mod tests {
    use crate::script_commands::{Catalog, parse_manifest};

    #[test]
    fn resolves_every_step_and_shares_matching_parameter_names() {
        let manifest = parse_manifest(
            r#"
[[command]]
title = "Build"
executable = "/synthetic/build"
parameters = ["Project name"]

[[command]]
title = "Check"
executable = "/synthetic/check"
parameters = [" project NAME ", "Branch"]

[[workflow]]
title = "Prepare"
steps = ["BUILD", "Check"]

[[workflow]]
title = "Missing"
steps = ["Build", "Absent"]
"#,
        )
        .unwrap();
        let catalog = Catalog::from_manifest(manifest);
        let workflows = catalog.matching_workflows("", false);
        let plan = catalog.plan(workflows[0].0).unwrap();
        assert_eq!(plan.parameters, ["Project name", "Branch"]);
        let values = ["A project".into(), "feature/name".into()];
        assert_eq!(plan.steps[0].inputs(&values), ["A project"]);
        assert_eq!(plan.steps[1].inputs(&values), values);
        assert!(catalog.plan(workflows[1].0).err().unwrap().contains("Step 2"));
    }
}

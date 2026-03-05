use super::{generation_prompt, CaseGenerationRequest, CaseGenerator};
use crate::command_match::argv_matches_command;
use crate::error::{llm_err, CapTraceError};
use crate::time_utils::now_ms;
use async_trait::async_trait;
use sieve_llm::{OpenAiPlannerModel, PlannerModel};
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use sieve_tool_contracts::{validate_at_index, TypedCall};
use sieve_types::{CommandKnowledge, PlannerTurnInput, RunId};
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct PlannerCaseGenerator {
    planner: Arc<dyn PlannerModel>,
    shell: BasicShellAnalyzer,
}

impl PlannerCaseGenerator {
    pub fn from_env() -> Result<Self, CapTraceError> {
        let planner = OpenAiPlannerModel::from_env().map_err(llm_err)?;
        Ok(Self {
            planner: Arc::new(planner),
            shell: BasicShellAnalyzer,
        })
    }

    #[cfg(test)]
    pub fn new(planner: Arc<dyn PlannerModel>) -> Self {
        Self {
            planner,
            shell: BasicShellAnalyzer,
        }
    }
}

#[async_trait]
impl CaseGenerator for PlannerCaseGenerator {
    async fn generate_cases(
        &self,
        request: CaseGenerationRequest,
    ) -> Result<Vec<Vec<String>>, CapTraceError> {
        let user_message = generation_prompt(&request.command, request.max_cases);
        let output = self
            .planner
            .plan_turn(PlannerTurnInput {
                run_id: RunId(format!("captrace-llm-{}", now_ms())),
                user_message,
                allowed_tools: vec!["bash".to_string()],
                allowed_net_connect_scopes: Vec::new(),
                previous_events: Vec::new(),
                guidance: None,
            })
            .await
            .map_err(llm_err)?;

        let mut unique = BTreeSet::new();
        let mut cases = Vec::new();
        for (idx, tool_call) in output.tool_calls.iter().enumerate() {
            let args_json = serde_json::to_value(&tool_call.args)
                .map_err(|err| CapTraceError::Llm(err.to_string()))?;
            let typed = validate_at_index(idx, &tool_call.tool_name, &args_json)
                .map_err(|err| CapTraceError::Llm(err.to_string()))?;
            let TypedCall::Bash(args) = typed else {
                continue;
            };
            let analysis = self
                .shell
                .analyze_shell_lc_script(&args.cmd)
                .map_err(|err| CapTraceError::Shell(err.to_string()))?;
            if analysis.knowledge != CommandKnowledge::Known || analysis.segments.len() != 1 {
                continue;
            }
            let argv = analysis.segments[0].argv.clone();
            if !argv_matches_command(&argv, &request.command) {
                continue;
            }
            let key = argv.join("\u{1f}");
            if unique.insert(key) {
                cases.push(argv);
            }
            if cases.len() >= request.max_cases {
                break;
            }
        }

        if cases.is_empty() {
            return Err(CapTraceError::Llm(
                "planner returned no valid command cases".to_string(),
            ));
        }
        Ok(cases)
    }
}

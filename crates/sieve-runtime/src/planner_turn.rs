use super::{RuntimeDisposition, RuntimeError, RuntimeOrchestrator, ShellRunRequest};
use sieve_tool_contracts::{validate_at_index, TypedCall, TOOL_CONTRACTS_VERSION};
use sieve_types::{
    ApprovalRequestId, DeclassifyRequest, DeclassifyStateTransition, EndorseRequest,
    EndorseStateTransition, PlannerBrowserSession, PlannerCodexSession, PlannerConversationMessage,
    PlannerGuidanceFrame, PlannerToolCall, PlannerTurnInput, RunId, RuntimeEvent,
    ToolContractValidationReport, TrustedToolEffect, UncertainMode, UnknownMode, ValueRef,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct PlannerRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub user_message: String,
    pub conversation: Vec<PlannerConversationMessage>,
    pub allowed_tools: Vec<String>,
    pub current_time_utc: Option<String>,
    pub current_timezone: Option<String>,
    pub allowed_net_connect_scopes: Vec<String>,
    pub browser_sessions: Vec<PlannerBrowserSession>,
    pub codex_sessions: Vec<PlannerCodexSession>,
    pub previous_events: Vec<RuntimeEvent>,
    pub guidance: Option<PlannerGuidanceFrame>,
    pub control_value_refs: BTreeSet<ValueRef>,
    pub control_endorsed_by: Option<ApprovalRequestId>,
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannerToolResult {
    Automation {
        request: sieve_types::AutomationRequest,
        message: Option<String>,
        effect: Option<TrustedToolEffect>,
        failure_reason: Option<String>,
    },
    Bash {
        command: String,
        disposition: RuntimeDisposition,
    },
    CodexExec {
        request: sieve_types::CodexExecRequest,
        result: Option<sieve_types::CodexExecResult>,
        failure_reason: Option<String>,
    },
    CodexSession {
        request: sieve_types::CodexSessionRequest,
        result: Option<sieve_types::CodexTurnResult>,
        failure_reason: Option<String>,
    },
    Endorse {
        request: EndorseRequest,
        transition: Option<EndorseStateTransition>,
    },
    Declassify {
        request: DeclassifyRequest,
        transition: Option<DeclassifyStateTransition>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerRunResult {
    pub thoughts: Option<String>,
    pub tool_results: Vec<PlannerToolResult>,
}

impl RuntimeOrchestrator {
    /// Runs a full planner turn and dispatches each validated tool call through runtime gates.
    pub async fn orchestrate_planner_turn(
        &self,
        request: PlannerRunRequest,
    ) -> Result<PlannerRunResult, RuntimeError> {
        let planner_output = self
            .planner
            .plan_turn(PlannerTurnInput {
                run_id: request.run_id.clone(),
                user_message: request.user_message.clone(),
                conversation: request.conversation.clone(),
                allowed_tools: request.allowed_tools.clone(),
                current_time_utc: request.current_time_utc.clone(),
                current_timezone: request.current_timezone.clone(),
                allowed_net_connect_scopes: request.allowed_net_connect_scopes.clone(),
                browser_sessions: request.browser_sessions.clone(),
                codex_sessions: request.codex_sessions.clone(),
                previous_events: request.previous_events.clone(),
                guidance: request.guidance.clone(),
            })
            .await?;

        let mut tool_results = Vec::with_capacity(planner_output.tool_calls.len());
        for (idx, tool_call) in planner_output.tool_calls.into_iter().enumerate() {
            Self::ensure_tool_allowed(idx, &tool_call.tool_name, &request.allowed_tools)?;
            let typed_call = self.validate_planner_tool_call(idx, &tool_call)?;
            match typed_call {
                TypedCall::Automation(automation_request) => {
                    let automation = self.automation.as_ref().ok_or_else(|| {
                        RuntimeError::Automation(
                            "automation tool is not configured for this runtime".to_string(),
                        )
                    })?;
                    match automation.handle_request(automation_request.clone()).await {
                        Ok(result) => tool_results.push(PlannerToolResult::Automation {
                            request: automation_request,
                            message: Some(result.message),
                            effect: result.effect,
                            failure_reason: None,
                        }),
                        Err(err) => tool_results.push(PlannerToolResult::Automation {
                            request: automation_request,
                            message: None,
                            effect: None,
                            failure_reason: Some(err),
                        }),
                    }
                }
                TypedCall::Bash(args) => {
                    let expanded_command =
                        match self.expand_bash_placeholders(&request.run_id, &args.cmd) {
                            Ok(command) if command.contains("[[handle:") => {
                                tool_results.push(PlannerToolResult::Bash {
                                    command: args.cmd,
                                    disposition: RuntimeDisposition::Denied {
                                        reason: "unknown opaque handle placeholder".to_string(),
                                    },
                                });
                                continue;
                            }
                            Ok(command) => command,
                            Err(err) => {
                                tool_results.push(PlannerToolResult::Bash {
                                    command: args.cmd,
                                    disposition: RuntimeDisposition::Denied {
                                        reason: err.to_string(),
                                    },
                                });
                                continue;
                            }
                        };
                    let disposition = self
                        .orchestrate_shell(ShellRunRequest {
                            run_id: request.run_id.clone(),
                            cwd: request.cwd.clone(),
                            script: expanded_command,
                            control_value_refs: request.control_value_refs.clone(),
                            control_endorsed_by: request.control_endorsed_by.clone(),
                            unknown_mode: request.unknown_mode,
                            uncertain_mode: request.uncertain_mode,
                        })
                        .await?;
                    tool_results.push(PlannerToolResult::Bash {
                        command: args.cmd,
                        disposition,
                    });
                }
                TypedCall::CodexExec(codex_request) => {
                    let codex = self.codex.as_ref().ok_or_else(|| {
                        RuntimeError::Automation(
                            "codex tool is not configured for this runtime".to_string(),
                        )
                    })?;
                    match codex.exec(codex_request.clone()).await {
                        Ok(result) => tool_results.push(PlannerToolResult::CodexExec {
                            request: codex_request,
                            result: Some(result.result),
                            failure_reason: None,
                        }),
                        Err(err) => tool_results.push(PlannerToolResult::CodexExec {
                            request: codex_request,
                            result: None,
                            failure_reason: Some(err),
                        }),
                    }
                }
                TypedCall::CodexSession(codex_request) => {
                    let codex = self.codex.as_ref().ok_or_else(|| {
                        RuntimeError::Automation(
                            "codex tool is not configured for this runtime".to_string(),
                        )
                    })?;
                    match codex.run_session(codex_request.clone()).await {
                        Ok(result) => tool_results.push(PlannerToolResult::CodexSession {
                            request: codex_request,
                            result: Some(result.result),
                            failure_reason: None,
                        }),
                        Err(err) => tool_results.push(PlannerToolResult::CodexSession {
                            request: codex_request,
                            result: None,
                            failure_reason: Some(err),
                        }),
                    }
                }
                TypedCall::Endorse(endorse_request) => {
                    let transition = self
                        .endorse_value_once(request.run_id.clone(), endorse_request.clone())
                        .await?;
                    tool_results.push(PlannerToolResult::Endorse {
                        request: endorse_request,
                        transition,
                    });
                }
                TypedCall::Declassify(declassify_request) => {
                    let transition = self
                        .declassify_value_once(request.run_id.clone(), declassify_request.clone())
                        .await?;
                    tool_results.push(PlannerToolResult::Declassify {
                        request: declassify_request,
                        transition,
                    });
                }
            }
        }

        Ok(PlannerRunResult {
            thoughts: planner_output.thoughts,
            tool_results,
        })
    }

    fn validate_planner_tool_call(
        &self,
        tool_call_index: usize,
        tool_call: &PlannerToolCall,
    ) -> Result<TypedCall, RuntimeError> {
        let args_json = serde_json::Value::Object(
            tool_call
                .args
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        match validate_at_index(tool_call_index, &tool_call.tool_name, &args_json) {
            Ok(typed) => Ok(typed),
            Err(error) => {
                let report = ToolContractValidationReport {
                    contract_version: TOOL_CONTRACTS_VERSION,
                    errors: vec![error.as_validation_error()],
                };
                Self::log_tool_contract_failure(&report);
                Err(RuntimeError::ToolContract { report })
            }
        }
    }

    fn ensure_tool_allowed(
        tool_call_index: usize,
        tool_name: &str,
        allowed_tools: &[String],
    ) -> Result<(), RuntimeError> {
        if allowed_tools.iter().any(|allowed| allowed == tool_name) {
            return Ok(());
        }

        Err(RuntimeError::DisallowedTool {
            tool_call_index,
            tool_name: tool_name.to_string(),
            allowed_tools: allowed_tools.to_vec(),
        })
    }

    fn log_tool_contract_failure(report: &ToolContractValidationReport) {
        if let Ok(encoded) = serde_json::to_string(report) {
            eprintln!("sieve-runtime contract validation failure: {encoded}");
        } else {
            eprintln!("sieve-runtime contract validation failure");
        }
    }
}

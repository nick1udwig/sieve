use serde::{Deserialize, Serialize};

/// Typed control signal emitted by Q-LLM and consumed by planner/runtime loop logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u16)]
pub enum PlannerGuidanceSignal {
    ContinueNeedEvidence = 100,
    ContinueFetchPrimarySource = 101,
    ContinueFetchAdditionalSource = 102,
    ContinueRefineApproach = 103,
    ContinueNeedRequiredParameter = 104,
    ContinueNeedFreshOrTimeBoundEvidence = 105,
    ContinueNeedPreferenceOrConstraint = 106,
    ContinueToolDeniedTryAlternativeAllowedTool = 107,
    ContinueNeedHigherQualitySource = 108,
    ContinueResolveSourceConflict = 109,
    ContinueNeedPrimaryContentFetch = 110,
    ContinueNeedUrlExtraction = 111,
    ContinueNeedCanonicalNonAssetUrl = 112,
    ContinueNoProgressTryDifferentAction = 113,
    ContinueNeedCurrentPageInspection = 114,
    ContinueEncounteredAccessInterstitial = 115,
    ContinueNeedCommandReformulation = 116,
    FinalAnswerReady = 200,
    FinalAnswerPartial = 201,
    FinalInsufficientEvidence = 202,
    FinalSingleFactReady = 203,
    FinalConflictingFactsWithRange = 204,
    FinalNoToolActionNeeded = 205,
    StopPolicyBlocked = 300,
    StopBudgetExhausted = 301,
    StopNoAllowedToolCanSatisfyTask = 302,
    ErrorContractViolation = 900,
}

impl PlannerGuidanceSignal {
    pub const fn code(self) -> u16 {
        self as u16
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::ContinueNeedEvidence => "continue_need_evidence",
            Self::ContinueFetchPrimarySource => "continue_fetch_primary_source",
            Self::ContinueFetchAdditionalSource => "continue_fetch_additional_source",
            Self::ContinueRefineApproach => "continue_refine_approach",
            Self::ContinueNeedRequiredParameter => "continue_need_required_parameter",
            Self::ContinueNeedFreshOrTimeBoundEvidence => {
                "continue_need_fresh_or_time_bound_evidence"
            }
            Self::ContinueNeedPreferenceOrConstraint => "continue_need_preference_or_constraint",
            Self::ContinueToolDeniedTryAlternativeAllowedTool => {
                "continue_tool_denied_try_alternative_allowed_tool"
            }
            Self::ContinueNeedHigherQualitySource => "continue_need_higher_quality_source",
            Self::ContinueResolveSourceConflict => "continue_resolve_source_conflict",
            Self::ContinueNeedPrimaryContentFetch => "continue_need_primary_content_fetch",
            Self::ContinueNeedUrlExtraction => "continue_need_url_extraction",
            Self::ContinueNeedCanonicalNonAssetUrl => "continue_need_canonical_non_asset_url",
            Self::ContinueNoProgressTryDifferentAction => {
                "continue_no_progress_try_different_action"
            }
            Self::ContinueNeedCurrentPageInspection => "continue_need_current_page_inspection",
            Self::ContinueEncounteredAccessInterstitial => {
                "continue_encountered_access_interstitial"
            }
            Self::ContinueNeedCommandReformulation => "continue_need_command_reformulation",
            Self::FinalAnswerReady => "final_answer_ready",
            Self::FinalAnswerPartial => "final_answer_partial",
            Self::FinalInsufficientEvidence => "final_insufficient_evidence",
            Self::FinalSingleFactReady => "final_single_fact_ready",
            Self::FinalConflictingFactsWithRange => "final_conflicting_facts_with_range",
            Self::FinalNoToolActionNeeded => "final_no_tool_action_needed",
            Self::StopPolicyBlocked => "stop_policy_blocked",
            Self::StopBudgetExhausted => "stop_budget_exhausted",
            Self::StopNoAllowedToolCanSatisfyTask => "stop_no_allowed_tool_can_satisfy_task",
            Self::ErrorContractViolation => "error_contract_violation",
        }
    }
}

impl TryFrom<u16> for PlannerGuidanceSignal {
    type Error = String;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            100 => Ok(Self::ContinueNeedEvidence),
            101 => Ok(Self::ContinueFetchPrimarySource),
            102 => Ok(Self::ContinueFetchAdditionalSource),
            103 => Ok(Self::ContinueRefineApproach),
            104 => Ok(Self::ContinueNeedRequiredParameter),
            105 => Ok(Self::ContinueNeedFreshOrTimeBoundEvidence),
            106 => Ok(Self::ContinueNeedPreferenceOrConstraint),
            107 => Ok(Self::ContinueToolDeniedTryAlternativeAllowedTool),
            108 => Ok(Self::ContinueNeedHigherQualitySource),
            109 => Ok(Self::ContinueResolveSourceConflict),
            110 => Ok(Self::ContinueNeedPrimaryContentFetch),
            111 => Ok(Self::ContinueNeedUrlExtraction),
            112 => Ok(Self::ContinueNeedCanonicalNonAssetUrl),
            113 => Ok(Self::ContinueNoProgressTryDifferentAction),
            114 => Ok(Self::ContinueNeedCurrentPageInspection),
            115 => Ok(Self::ContinueEncounteredAccessInterstitial),
            116 => Ok(Self::ContinueNeedCommandReformulation),
            200 => Ok(Self::FinalAnswerReady),
            201 => Ok(Self::FinalAnswerPartial),
            202 => Ok(Self::FinalInsufficientEvidence),
            203 => Ok(Self::FinalSingleFactReady),
            204 => Ok(Self::FinalConflictingFactsWithRange),
            205 => Ok(Self::FinalNoToolActionNeeded),
            300 => Ok(Self::StopPolicyBlocked),
            301 => Ok(Self::StopBudgetExhausted),
            302 => Ok(Self::StopNoAllowedToolCanSatisfyTask),
            900 => Ok(Self::ErrorContractViolation),
            _ => Err(format!("unknown planner guidance signal code `{value}`")),
        }
    }
}

/// Numeric wire-safe envelope for Q-LLM -> planner guidance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerGuidanceFrame {
    pub code: u16,
    #[serde(default)]
    pub confidence_bps: u16,
    #[serde(default)]
    pub source_hit_index: Option<u16>,
    #[serde(default)]
    pub evidence_ref_index: Option<u16>,
}

impl PlannerGuidanceFrame {
    pub fn signal(&self) -> Result<PlannerGuidanceSignal, String> {
        PlannerGuidanceSignal::try_from(self.code)
    }
}

/// Input payload for Q-LLM planner-guidance classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerGuidanceInput {
    pub run_id: crate::RunId,
    pub prompt: String,
}

/// Output payload for Q-LLM planner-guidance classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerGuidanceOutput {
    pub guidance: PlannerGuidanceFrame,
}

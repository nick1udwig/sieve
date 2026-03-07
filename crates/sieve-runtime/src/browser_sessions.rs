use crate::browser_session_summary::{
    contextual_summary, parse_agent_browser, session_transition, ParsedAgentBrowser,
    SessionTransition,
};
use sieve_command_summaries::SummaryOutcome;
use sieve_types::CommandKnowledge;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserSessionState {
    pub current_origin: String,
    pub current_url: String,
    pub tabs: Vec<String>,
    pub frames: Vec<String>,
    pub allowed_origins: BTreeSet<String>,
}

pub(crate) type BrowserSessionMutations = BTreeMap<String, Option<BrowserSessionState>>;

#[derive(Debug, Clone)]
struct ImplicitSessionState {
    name: Option<String>,
    state: BrowserSessionState,
}

#[derive(Debug)]
pub(crate) struct BrowserSessionTracker {
    named: BTreeMap<String, BrowserSessionState>,
    implicit: Option<ImplicitSessionState>,
    mutations: BrowserSessionMutations,
}

impl BrowserSessionTracker {
    pub(crate) fn new(named: BTreeMap<String, BrowserSessionState>) -> Self {
        Self {
            named,
            implicit: None,
            mutations: BTreeMap::new(),
        }
    }

    pub(crate) fn summarize_segment(
        &mut self,
        argv: &[String],
        stateless: SummaryOutcome,
    ) -> SummaryOutcome {
        let Some(parsed) = parse_agent_browser(argv) else {
            return stateless;
        };

        let outcome = if stateless.knowledge == CommandKnowledge::Unknown
            && stateless
                .summary
                .as_ref()
                .is_none_or(|summary| summary.unsupported_flags.is_empty())
        {
            self.session_state_for(parsed.session_name.as_deref())
                .and_then(|state| contextual_summary(&parsed, &state.current_origin))
                .unwrap_or(stateless)
        } else {
            stateless
        };

        if outcome.knowledge == CommandKnowledge::Known {
            self.apply_successful_segment(&parsed);
        }

        outcome
    }

    pub(crate) fn mutations(self) -> BrowserSessionMutations {
        self.mutations
    }

    fn session_state_for(&self, session_name: Option<&str>) -> Option<&BrowserSessionState> {
        if let Some(name) = session_name {
            return self.named.get(name);
        }
        self.implicit.as_ref().map(|session| &session.state)
    }

    fn apply_successful_segment(&mut self, parsed: &ParsedAgentBrowser) {
        match session_transition(parsed) {
            SessionTransition::SetCurrentPage { origin, url } => {
                let mut allowed_origins = BTreeSet::new();
                allowed_origins.insert(origin.clone());
                let state = BrowserSessionState {
                    current_origin: origin,
                    current_url: url.clone(),
                    tabs: vec![url],
                    frames: vec!["main".to_string()],
                    allowed_origins,
                };
                if let Some(name) = parsed.session_name.clone() {
                    self.named.insert(name.clone(), state.clone());
                    self.mutations.insert(name.clone(), Some(state.clone()));
                    self.implicit = Some(ImplicitSessionState {
                        name: Some(name),
                        state,
                    });
                } else {
                    self.implicit = Some(ImplicitSessionState { name: None, state });
                }
            }
            SessionTransition::Clear => {
                if let Some(name) = parsed.session_name.clone().or_else(|| {
                    self.implicit
                        .as_ref()
                        .and_then(|session| session.name.clone())
                }) {
                    self.named.remove(&name);
                    self.mutations.insert(name, None);
                }
                self.implicit = None;
            }
            SessionTransition::None => {
                if let Some(name) = parsed.session_name.as_ref() {
                    if let Some(state) = self.named.get(name).cloned() {
                        self.implicit = Some(ImplicitSessionState {
                            name: Some(name.clone()),
                            state,
                        });
                    }
                }
            }
        }
    }
}

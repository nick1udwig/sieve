use sieve_types::{CommandKnowledge, CommandSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryOutcome {
    pub knowledge: CommandKnowledge,
    pub summary: Option<CommandSummary>,
    pub reason: Option<String>,
}

pub trait CommandSummarizer: Send + Sync {
    fn summarize(&self, argv: &[String]) -> SummaryOutcome;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultCommandSummarizer;

impl CommandSummarizer for DefaultCommandSummarizer {
    fn summarize(&self, argv: &[String]) -> SummaryOutcome {
        crate::summarize_argv(argv)
    }
}

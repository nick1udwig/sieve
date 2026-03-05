mod fs;
mod lcm;

use crate::SummaryOutcome;

pub(crate) fn summarize_builtin(argv: &[String]) -> Option<SummaryOutcome> {
    fs::summarize_fs_builtin(argv).or_else(|| lcm::summarize_sieve_lcm_cli(argv))
}

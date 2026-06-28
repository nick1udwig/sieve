use crate::SummaryOutcome;

mod cp;
mod metadata;
mod mkdir;
mod mv;
mod tee;
mod touch;
mod trash;

use cp::summarize_cp;
use metadata::{summarize_chmod, summarize_chown};
use mkdir::summarize_mkdir;
use mv::summarize_mv;
use tee::summarize_tee;
use touch::summarize_touch;
use trash::summarize_trash;

pub(super) fn summarize_fs_builtin(argv: &[String]) -> Option<SummaryOutcome> {
    summarize_trash(argv)
        .or_else(|| summarize_cp(argv))
        .or_else(|| summarize_mv(argv))
        .or_else(|| summarize_mkdir(argv))
        .or_else(|| summarize_touch(argv))
        .or_else(|| summarize_chmod(argv))
        .or_else(|| summarize_chown(argv))
        .or_else(|| summarize_tee(argv))
}

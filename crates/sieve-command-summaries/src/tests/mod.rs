mod agent_browser;
mod command_classes;
mod curl;
mod fs;
mod gws;
mod planner_catalog;
mod sieve_lcm;

pub(super) fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

mod collect;
mod parse;

use crate::report::io_err;
use crate::QuarantineRunError;
use sieve_types::{Action, Capability, Resource};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

pub(crate) use collect::collect_trace_files;
pub(crate) use parse::parse_trace_line;

pub(crate) fn parse_trace_capabilities(
    trace_files: &[PathBuf],
) -> Result<Vec<Capability>, QuarantineRunError> {
    let mut set = BTreeSet::new();

    for path in trace_files {
        let content = fs::read_to_string(path).map_err(io_err)?;
        for line in content.lines() {
            if let Some(capability) = parse_trace_line(line) {
                set.insert((
                    resource_order(capability.resource),
                    action_order(capability.action),
                    capability.scope,
                ));
            }
        }
    }

    Ok(set
        .into_iter()
        .map(|(resource_key, action_key, scope)| Capability {
            resource: resource_from_order(resource_key),
            action: action_from_order(action_key),
            scope,
        })
        .collect())
}

fn resource_order(resource: Resource) -> u8 {
    match resource {
        Resource::Fs => 0,
        Resource::Net => 1,
        Resource::Proc => 2,
        Resource::Env => 3,
        Resource::Ipc => 4,
    }
}

fn action_order(action: Action) -> u8 {
    match action {
        Action::Read => 0,
        Action::Write => 1,
        Action::Append => 2,
        Action::Exec => 3,
        Action::Connect => 4,
    }
}

fn resource_from_order(key: u8) -> Resource {
    match key {
        0 => Resource::Fs,
        1 => Resource::Net,
        2 => Resource::Proc,
        3 => Resource::Env,
        4 => Resource::Ipc,
        _ => Resource::Proc,
    }
}

fn action_from_order(key: u8) -> Action {
    match key {
        0 => Action::Read,
        1 => Action::Write,
        2 => Action::Append,
        3 => Action::Exec,
        4 => Action::Connect,
        _ => Action::Exec,
    }
}

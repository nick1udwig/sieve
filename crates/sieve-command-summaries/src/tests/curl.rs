use super::argv;
use crate::summarize_argv;
use sieve_types::{Action, Capability, CommandKnowledge, Resource, SinkCheck, SinkKey, ValueRef};

include!("curl/payloads.rs");
include!("curl/request_values.rs");
include!("curl/network.rs");

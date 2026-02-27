use crate::{TOOL_BASH, TOOL_BRAVE_SEARCH, TOOL_DECLASSIFY, TOOL_ENDORSE};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn tool_args_schema(tool_name: &str) -> Option<Value> {
    match tool_name {
        TOOL_BASH => Some(bash_args_schema()),
        TOOL_ENDORSE => Some(endorse_args_schema()),
        TOOL_DECLASSIFY => Some(declassify_args_schema()),
        TOOL_BRAVE_SEARCH => Some(brave_search_args_schema()),
        _ => None,
    }
}

pub fn all_tool_args_schemas() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    out.insert(TOOL_BASH.to_string(), bash_args_schema());
    out.insert(TOOL_BRAVE_SEARCH.to_string(), brave_search_args_schema());
    out.insert(TOOL_DECLASSIFY.to_string(), declassify_args_schema());
    out.insert(TOOL_ENDORSE.to_string(), endorse_args_schema());
    out
}

pub fn planner_tool_call_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "tool_name": {"const": TOOL_BASH},
                    "args": bash_args_schema()
                },
                "required": ["tool_name", "args"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "tool_name": {"const": TOOL_ENDORSE},
                    "args": endorse_args_schema()
                },
                "required": ["tool_name", "args"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "tool_name": {"const": TOOL_DECLASSIFY},
                    "args": declassify_args_schema()
                },
                "required": ["tool_name", "args"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "tool_name": {"const": TOOL_BRAVE_SEARCH},
                    "args": brave_search_args_schema()
                },
                "required": ["tool_name", "args"]
            }
        ]
    })
}

pub fn planner_turn_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "thoughts": { "type": ["string", "null"] },
            "tool_calls": {
                "type": "array",
                "items": planner_tool_call_schema()
            }
        },
        "required": ["thoughts", "tool_calls"]
    })
}

pub fn emitted_schema_documents() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    out.insert("bash-args.schema.json".to_string(), bash_args_schema());
    out.insert(
        "brave-search-args.schema.json".to_string(),
        brave_search_args_schema(),
    );
    out.insert(
        "declassify-args.schema.json".to_string(),
        declassify_args_schema(),
    );
    out.insert(
        "endorse-args.schema.json".to_string(),
        endorse_args_schema(),
    );
    out.insert(
        "planner-tool-call.schema.json".to_string(),
        planner_tool_call_schema(),
    );
    out.insert(
        "planner-turn-output.schema.json".to_string(),
        planner_turn_output_schema(),
    );
    out
}

fn bash_args_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "cmd": {"type": "string"}
        },
        "required": ["cmd"]
    })
}

fn endorse_args_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "value_ref": {"type": "string"},
            "target_integrity": {
                "type": "string",
                "enum": ["trusted", "untrusted"]
            },
            "reason": {"type": ["string", "null"]}
        },
        "required": ["value_ref", "target_integrity"]
    })
}

fn declassify_args_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "value_ref": {"type": "string"},
            "sink": {
                "type": "string",
                "description": "absolute URL sink key"
            },
            "reason": {"type": ["string", "null"]}
        },
        "required": ["value_ref", "sink"]
    })
}

fn brave_search_args_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": {"type": "string"},
            "count": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10
            }
        },
        "required": ["query"]
    })
}

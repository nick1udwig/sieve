use crate::{TOOL_AUTOMATION, TOOL_BASH, TOOL_DECLASSIFY, TOOL_ENDORSE};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn tool_args_schema(tool_name: &str) -> Option<Value> {
    match tool_name {
        TOOL_AUTOMATION => Some(automation_args_schema()),
        TOOL_BASH => Some(bash_args_schema()),
        TOOL_ENDORSE => Some(endorse_args_schema()),
        TOOL_DECLASSIFY => Some(declassify_args_schema()),
        _ => None,
    }
}

pub fn all_tool_args_schemas() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    out.insert(TOOL_AUTOMATION.to_string(), automation_args_schema());
    out.insert(TOOL_BASH.to_string(), bash_args_schema());
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
                    "tool_name": {"const": TOOL_AUTOMATION},
                    "args": automation_args_schema()
                },
                "required": ["tool_name", "args"]
            },
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
    out.insert(
        "automation-args.schema.json".to_string(),
        automation_args_schema(),
    );
    out.insert("bash-args.schema.json".to_string(), bash_args_schema());
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

fn automation_args_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {
                "type": "string",
                "enum": ["cron_list", "cron_add", "cron_remove", "cron_pause", "cron_resume"]
            },
            "target": {
                "type": ["string", "null"],
                "enum": ["main", "isolated", null]
            },
            "schedule": {
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": {"const": "after"},
                            "delay": {"type": "string"}
                        },
                        "required": ["kind", "delay"]
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": {"const": "at"},
                            "timestamp": {"type": "string"}
                        },
                        "required": ["kind", "timestamp"]
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": {"const": "every"},
                            "interval": {"type": "string"}
                        },
                        "required": ["kind", "interval"]
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": {"const": "cron"},
                            "expr": {"type": "string"}
                        },
                        "required": ["kind", "expr"]
                    },
                    {"type": "null"}
                ]
            },
            "prompt": {"type": ["string", "null"]},
            "job_id": {"type": ["string", "null"]}
        },
        "required": ["action"]
    })
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
            "channel": {
                "type": "string",
                "enum": ["body", "header", "query", "path", "cookie"]
            },
            "reason": {"type": ["string", "null"]}
        },
        "required": ["value_ref", "sink", "channel"]
    })
}

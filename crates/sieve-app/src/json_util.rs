use serde::Serialize;

pub(crate) fn to_json_value<T: Serialize>(value: T, context: &str) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|err| panic!("failed to serialize {context}: {err}"))
}

pub(crate) fn to_json_string<T: Serialize>(value: &T, context: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|err| panic!("failed to serialize {context}: {err}"))
}

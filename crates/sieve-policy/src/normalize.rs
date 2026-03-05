use crate::canonicalize_sink_key;
use crate::PolicyConfig;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(crate) fn normalize_config(config: &mut PolicyConfig) {
    for cap in &mut config.allow_capabilities {
        if cap.resource == sieve_types::Resource::Net {
            cap.scope = canonicalize_sink_key(&cap.scope).unwrap_or_else(|_| cap.scope.clone());
        }
    }

    for sinks in config.value_sinks.values_mut() {
        let normalized: BTreeSet<String> = sinks
            .iter()
            .map(|sink| canonicalize_sink_key(sink).unwrap_or_else(|_| sink.clone()))
            .collect();
        *sinks = normalized;
    }
}

pub(crate) fn normalize_capability_scope(
    resource: sieve_types::Resource,
    scope: &str,
    cwd: &str,
) -> String {
    match resource {
        sieve_types::Resource::Net => {
            canonicalize_sink_key(scope).unwrap_or_else(|_| scope.to_string())
        }
        sieve_types::Resource::Fs => canonicalize_fs_scope(scope, cwd),
        _ => scope.to_string(),
    }
}

pub(crate) fn canonicalize_fs_scope(scope: &str, cwd: &str) -> String {
    let path = Path::new(scope);
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(cwd).join(path)
    };
    normalize_path_lexically(base)
}

pub(crate) fn normalize_path_lexically(path: PathBuf) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut absolute = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                out.clear();
                out.push(prefix.as_os_str().to_string_lossy().into_owned());
            }
            Component::RootDir => {
                absolute = true;
                out.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = out.last() {
                    if last != ".." {
                        out.pop();
                    } else if !absolute {
                        out.push("..".to_string());
                    }
                } else if !absolute {
                    out.push("..".to_string());
                }
            }
            Component::Normal(seg) => out.push(seg.to_string_lossy().into_owned()),
        }
    }

    if absolute {
        if out.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", out.join("/"))
        }
    } else if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    }
}

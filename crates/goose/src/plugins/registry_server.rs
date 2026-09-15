use crate::agents::extension::{Envs, ExtensionConfig};
use crate::config::{ExtensionEntry, DEFAULT_EXTENSION_TIMEOUT};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerDocument {
    name: String,
    description: String,
    #[serde(default)]
    packages: Vec<Package>,
    #[serde(default)]
    remotes: Vec<Transport>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Package {
    registry_type: String,
    identifier: String,
    version: Option<String>,
    runtime_hint: Option<String>,
    transport: Transport,
    #[serde(default)]
    runtime_arguments: Vec<Argument>,
    #[serde(default)]
    package_arguments: Vec<Argument>,
    #[serde(default)]
    environment_variables: Vec<Input>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transport {
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    #[serde(default)]
    headers: Vec<Input>,
    #[serde(default)]
    variables: HashMap<String, Input>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    name: Option<String>,
    value: Option<String>,
    default: Option<String>,
    #[serde(default)]
    is_required: bool,
    #[serde(default)]
    is_secret: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Argument {
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    value: Option<String>,
    default: Option<String>,
    value_hint: Option<String>,
    #[serde(default)]
    is_required: bool,
    #[serde(default)]
    variables: HashMap<String, Input>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerSelection {
    Package(usize),
    Remote(usize),
}

pub fn server_json_choices(json: &str) -> Result<Vec<(ServerSelection, String)>> {
    let document: ServerDocument =
        serde_json::from_str(json).context("invalid MCP registry server.json")?;
    let packages = document
        .packages
        .iter()
        .enumerate()
        .map(|(index, package)| {
            (
                ServerSelection::Package(index),
                format!(
                    "Package {index}: {} {} ({})",
                    package.identifier,
                    package.version.as_deref().unwrap_or(""),
                    package.registry_type
                ),
            )
        });
    let remotes = document.remotes.iter().enumerate().map(|(index, remote)| {
        (
            ServerSelection::Remote(index),
            format!(
                "Remote {index}: {} ({})",
                remote.url.as_deref().unwrap_or("missing URL"),
                remote.kind
            ),
        )
    });
    Ok(packages.chain(remotes).collect())
}

pub fn import_server_json(
    json: &str,
    selections: &[ServerSelection],
    values: &HashMap<String, String>,
) -> Result<(Vec<ExtensionEntry>, HashMap<String, String>)> {
    let document: ServerDocument =
        serde_json::from_str(json).context("invalid MCP registry server.json")?;
    if selections.is_empty() {
        bail!("no package or remote selected")
    }
    let base_name = document.name.rsplit('/').next().unwrap_or(&document.name);
    let multiple = selections.len() > 1;
    let secret_names = selections
        .iter()
        .flat_map(|selection| -> Box<dyn Iterator<Item = &Input>> {
            match *selection {
                ServerSelection::Package(index) => Box::new(
                    document
                        .packages
                        .get(index)
                        .into_iter()
                        .flat_map(|package| package.environment_variables.iter()),
                ),
                ServerSelection::Remote(index) => Box::new(
                    document
                        .remotes
                        .get(index)
                        .into_iter()
                        .flat_map(|remote| remote.headers.iter()),
                ),
            }
        })
        .filter(|input| input.is_secret)
        .filter_map(|input| input.name.as_deref())
        .collect::<Vec<_>>();
    let secrets = secret_names
        .iter()
        .filter_map(|name| {
            values
                .get(*name)
                .map(|value| ((*name).to_string(), value.clone()))
        })
        .collect();
    let public_values = values
        .iter()
        .filter(|(name, _)| !secret_names.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let entries = selections
        .iter()
        .map(|selection| {
            let (name, config) = match *selection {
                ServerSelection::Package(index) => {
                    let package = document
                        .packages
                        .get(index)
                        .with_context(|| format!("package index {index} is out of range"))?;
                    let name = if multiple {
                        format!("{base_name}-package-{index}")
                    } else {
                        base_name.to_string()
                    };
                    let config = package_config(
                        name.clone(),
                        document.description.clone(),
                        package,
                        &public_values,
                    )?;
                    (name, config)
                }
                ServerSelection::Remote(index) => {
                    let remote = document
                        .remotes
                        .get(index)
                        .with_context(|| format!("remote index {index} is out of range"))?;
                    let name = if multiple {
                        format!("{base_name}-remote-{index}")
                    } else {
                        base_name.to_string()
                    };
                    let config = remote_config(
                        name.clone(),
                        document.description.clone(),
                        remote,
                        &public_values,
                    )?;
                    (name, config)
                }
            };
            let _ = name;
            Ok(ExtensionEntry {
                enabled: true,
                config,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((entries, secrets))
}

fn package_config(
    name: String,
    description: String,
    package: &Package,
    values: &HashMap<String, String>,
) -> Result<ExtensionConfig> {
    if package.transport.kind != "stdio" {
        return remote_config(name, description, &package.transport, values);
    }
    let (cmd, mut args) = runtime(package)?;
    args.extend(resolve_arguments(&package.runtime_arguments, values)?);
    let (envs, env_keys) = resolve_inputs(&package.environment_variables, values)?;
    match package.registry_type.as_str() {
        "npm" => {
            args.push("-y".into());
            args.push(versioned(
                &package.identifier,
                package.version.as_deref(),
                "@",
            ));
        }
        "pypi" => args.push(versioned(
            &package.identifier,
            package.version.as_deref(),
            "==",
        )),
        "oci" => {
            args.extend(["--rm".into(), "-i".into()]);
            for input in &package.environment_variables {
                let name = input
                    .name
                    .as_deref()
                    .context("environment variable is missing name")?;
                args.extend(["-e".into(), name.into()]);
            }
            args.push(package.identifier.clone());
        }
        "nuget" => args.push(versioned(
            &package.identifier,
            package.version.as_deref(),
            "@",
        )),
        "cargo" => {
            anyhow::ensure!(
                package.runtime_hint.is_none(),
                "cargo packages do not support runtimeHint"
            );
        }
        "mcpb" => bail!(
            "MCPB packages require bundle download, SHA-256 verification, extraction, and manifest processing; Goose does not yet support importing them"
        ),
        _other if package.runtime_hint.is_some() => args.push(package.identifier.clone()),
        other => bail!("unsupported registryType '{other}'; a runtimeHint is required"),
    }
    if package.registry_type == "nuget" && !package.package_arguments.is_empty() {
        args.push("--".into());
    }
    args.extend(resolve_arguments(&package.package_arguments, values)?);
    Ok(ExtensionConfig::Stdio {
        name,
        description,
        cmd,
        args,
        envs: Envs::new(envs),
        env_keys,
        timeout: Some(DEFAULT_EXTENSION_TIMEOUT),
        cwd: None,
        bundled: Some(false),
        available_tools: vec![],
    })
}

fn runtime(package: &Package) -> Result<(String, Vec<String>)> {
    if let Some(runtime) = &package.runtime_hint {
        return Ok((runtime.clone(), vec![]));
    }
    Ok(match package.registry_type.as_str() {
        "npm" => ("npx".into(), vec![]),
        "pypi" => ("uvx".into(), vec![]),
        "oci" => ("docker".into(), vec!["run".into()]),
        "nuget" => ("dnx".into(), vec![]),
        "cargo" => (package.identifier.clone(), vec![]),
        "mcpb" => bail!(
            "MCPB packages require bundle download, SHA-256 verification, extraction, and manifest processing; Goose does not yet support importing them"
        ),
        other => bail!("unsupported registryType '{other}'"),
    })
}

fn remote_config(
    name: String,
    description: String,
    transport: &Transport,
    values: &HashMap<String, String>,
) -> Result<ExtensionConfig> {
    if transport.kind != "streamable-http" {
        bail!(
            "unsupported transport '{}'; Goose supports stdio and streamable-http",
            transport.kind
        );
    }
    let vars = resolve_variable_map(&transport.variables, values)?;
    let uri = substitute(
        transport
            .url
            .as_deref()
            .context("streamable-http transport is missing url")?,
        &vars,
    );
    let (headers, env_keys) = resolve_named_inputs(&transport.headers, values)?;
    Ok(ExtensionConfig::StreamableHttp {
        name,
        description,
        uri,
        envs: Envs::default(),
        env_keys,
        headers,
        timeout: Some(DEFAULT_EXTENSION_TIMEOUT),
        socket: None,
        client_id: None,
        client_secret_key: None,
        scopes: vec![],
        bundled: Some(false),
        available_tools: vec![],
    })
}

fn resolve_arguments(items: &[Argument], values: &HashMap<String, String>) -> Result<Vec<String>> {
    let mut out = vec![];
    for item in items {
        let vars = resolve_variable_map(&item.variables, values)?;
        let key = item.value_hint.as_deref().or(item.name.as_deref());
        let value = item
            .value
            .clone()
            .or_else(|| key.and_then(|k| values.get(k).cloned()))
            .or_else(|| item.default.clone());
        if item.is_required && value.is_none() {
            bail!(
                "missing required value '{}'; pass --value {}=VALUE",
                key.unwrap_or("argument"),
                key.unwrap_or("argument")
            );
        }
        if let Some(value) = value {
            if item.kind == "named" {
                out.push(
                    item.name
                        .clone()
                        .context("named argument is missing name")?,
                );
            }
            out.push(substitute(&value, &vars));
        }
    }
    Ok(out)
}

fn resolve_inputs(
    items: &[Input],
    values: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, Vec<String>)> {
    let mut fixed = HashMap::new();
    let mut keys = vec![];
    for input in items {
        let name = input
            .name
            .as_deref()
            .context("environment variable is missing name")?;
        if let Some(value) = input
            .value
            .clone()
            .or_else(|| values.get(name).cloned())
            .or_else(|| input.default.clone())
        {
            fixed.insert(name.into(), value);
        } else if input.is_required || input.is_secret {
            keys.push(name.into());
        }
    }
    Ok((fixed, keys))
}
fn resolve_named_inputs(
    items: &[Input],
    values: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, Vec<String>)> {
    let mut fixed = HashMap::new();
    let mut keys = vec![];
    for input in items {
        let name = input.name.as_deref().context("header is missing name")?;
        if let Some(v) = input
            .value
            .clone()
            .or_else(|| values.get(name).cloned())
            .or_else(|| input.default.clone())
        {
            fixed.insert(name.into(), v);
        } else if input.is_required || input.is_secret {
            keys.push(name.into());
            fixed.insert(name.into(), format!("${{{name}}}"));
        }
    }
    Ok((fixed, keys))
}
fn resolve_variable_map(
    inputs: &HashMap<String, Input>,
    values: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    inputs
        .iter()
        .map(|(k, v)| {
            let value = v
                .value
                .clone()
                .or_else(|| values.get(k).cloned())
                .or_else(|| v.default.clone());
            if v.is_required && value.is_none() {
                bail!("missing required value '{k}'; pass --value {k}=VALUE");
            }
            Ok((k.clone(), value.unwrap_or_default()))
        })
        .collect()
}
fn substitute(template: &str, values: &HashMap<String, String>) -> String {
    values.iter().fold(template.to_string(), |s, (k, v)| {
        s.replace(&format!("{{{k}}}"), v)
    })
}
fn versioned(id: &str, version: Option<&str>, separator: &str) -> String {
    version.map_or_else(|| id.into(), |v| format!("{id}{separator}{v}"))
}

//! Explicit session CLI projection. No legacy store opens or mutation replay.
use anyhow::Result;
use serde_json::Value;

#[cfg(not(feature = "recalc-formualizer"))]
pub async fn run(_: crate::cli::SessionArgs) -> Result<Value> {
    anyhow::bail!(
        "native durable sessions require the recalc-formualizer feature; legacy mutation fallback is disabled"
    )
}

#[cfg(feature = "recalc-formualizer")]
pub async fn run(args: crate::cli::SessionArgs) -> Result<Value> {
    native::run(args).await
}

#[cfg(feature = "recalc-formualizer")]
mod native {
    use super::*;
    use crate::{
        cli::{SessionArgs, SessionCommands},
        native_host::{HostRequest, NativeHostClient},
        operations::{CanonicalErrorCode, CanonicalResponse},
    };
    use anyhow::{Context, anyhow, bail, ensure};
    use serde_json::json;
    use std::path::PathBuf;

    struct Client {
        host: NativeHostClient,
        resource: String,
        identity: String,
        expected: Option<String>,
    }
    impl Client {
        async fn call(&self, operation: &str, mut payload: Value) -> Result<CanonicalResponse> {
            let object = payload
                .as_object_mut()
                .ok_or_else(|| anyhow!("operation input must be an object"))?;
            if let Some(resource) = object.get("resource_id") {
                ensure!(
                    resource.as_str() == Some(&self.resource),
                    "payload resource differs from --session"
                );
            }
            object.insert("resource_id".into(), json!(self.resource));
            self.host
                .execute(
                    self.resource.clone(),
                    self.identity.clone(),
                    operation.into(),
                    payload,
                )
                .await
                .map_err(|error| anyhow!("outcome unknown for request {}: {error}", self.identity))?
                .map_err(|error| {
                    anyhow!(
                        "request {}: {}",
                        self.identity,
                        serde_json::to_string(&error).unwrap_or_default()
                    )
                })
        }
        async fn original(&self) -> Result<Option<CanonicalResponse>> {
            let result = self.host.execute(self.resource.clone(), self.identity.clone(), "session_history".into(),
                json!({"resource_id":self.resource,"action":"outcome","request_id":self.identity})).await?;
            match result {
                Err(error) if error.error.code == CanonicalErrorCode::ResourceNotFound => Ok(None),
                Err(error) => bail!("{}", serde_json::to_string(&error)?),
                Ok(response) => match response.data["state"].as_str() {
                    Some("not_found") => Ok(None),
                    Some("committed") => Ok(Some(serde_json::from_value(
                        response.data["response"].clone(),
                    )?)),
                    _ => bail!(
                        "outcome unknown for request {}; reconcile before retry",
                        self.identity
                    ),
                },
            }
        }
        async fn revision(&self) -> Result<String> {
            if let Some(expected) = &self.expected {
                return Ok(expected.clone());
            }
            if let Some(original) = self.original().await? {
                if let Some(before) = original.data["revision_before"].as_str() {
                    return Ok(before.into());
                }
            }
            let status = self
                .call("session_history", json!({"action":"status"}))
                .await?;
            status.data["revision_id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("session has no usable current revision"))
        }
        async fn operation(
            &self,
            operation: &str,
            mut payload: Value,
        ) -> Result<CanonicalResponse> {
            let descriptor = crate::operations::operation_descriptor(operation)
                .ok_or_else(|| anyhow!("unknown canonical operation {operation}"))?;
            if needs_revision(&(descriptor.input_schema)(), &payload)
                && payload.get("expected_revision").is_none()
            {
                payload
                    .as_object_mut()
                    .ok_or_else(|| anyhow!("input must be an object"))?
                    .insert("expected_revision".into(), json!(self.revision().await?));
            }
            self.call(operation, payload).await
        }
        fn project(&self, response: CanonicalResponse) -> Value {
            let mut value = serde_json::to_value(response).expect("canonical response serializes");
            value["session_id"] = json!(self.resource);
            value["request_id"] = json!(self.identity);
            value
        }
        async fn history(&self, payload: Value) -> Result<Value> {
            let response = self.operation("session_history", payload).await?;
            let mut value = response.data.clone();
            value["session_id"] = json!(self.resource);
            value["request_id"] = json!(self.identity);
            value["revision_id"] = json!(response.revision_id);
            Ok(value)
        }
    }

    // Derive CAS requirements from the canonical schema, including action unions,
    // instead of maintaining another operation list in this adapter.
    fn needs_revision(schema: &Value, input: &Value) -> bool {
        if let Some(action) = schema.pointer("/properties/action/const") {
            if input.get("action") != Some(action) {
                return false;
            }
        }
        schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields.iter().any(|field| field == "expected_revision"))
            || ["oneOf", "anyOf", "allOf"].iter().any(|key| {
                schema
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|variants| {
                        variants
                            .iter()
                            .any(|variant| needs_revision(variant, input))
                    })
            })
    }
    fn workspace(path: Option<PathBuf>) -> Result<PathBuf> {
        Ok(path.unwrap_or(std::env::current_dir()?).canonicalize()?)
    }
    fn resource(id: &str) -> Result<String> {
        let resource = if id.starts_with("fork:") || id.starts_with("session:") {
            id.to_owned()
        } else if id.starts_with("created_") {
            format!("fork:{id}")
        } else {
            format!("session:{id}")
        };
        let _: crate::operations::ResourceId = serde_json::from_value(json!(resource))?;
        Ok(resource)
    }
    async fn client(
        workspace: Option<PathBuf>,
        id: &str,
        identity: String,
        expected: Option<String>,
    ) -> Result<Client> {
        let workspace = self::workspace(workspace)?;
        let root =
            crate::native_host::workspace_root(&workspace).context("resolve native host root")?;
        let host = NativeHostClient::connect(&root)
            .await
            .context("connect native host (not submitted)")?;
        Ok(Client {
            host,
            resource: resource(id)?,
            identity,
            expected,
        })
    }

    pub async fn run(args: SessionArgs) -> Result<Value> {
        let identity = args
            .request_id
            .or_else(|| std::env::var("ASP_REQUEST_ID").ok())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        ensure!(
            !identity.is_empty() && identity.len() <= 256,
            "request identity must contain 1-256 bytes"
        );
        let expected = args.expected_revision;
        match args.command {
            SessionCommands::Start {
                base,
                label,
                workspace,
            } => {
                let workspace = self::workspace(workspace)?;
                let source = if base.is_absolute() {
                    base
                } else {
                    std::env::current_dir()?.join(base)
                };
                // Resolve a missing leaf without reopening the workbook, allowing
                // an original creation receipt to reconcile after source deletion.
                let source = if source.exists() {
                    source.canonicalize()?
                } else {
                    source
                        .parent()
                        .ok_or_else(|| anyhow!("base has no parent"))?
                        .canonicalize()
                        .context("base file not found: source parent is unavailable")?
                        .join(source.file_name().ok_or_else(|| anyhow!("invalid base"))?)
                };
                ensure!(
                    source.starts_with(&workspace),
                    "session base must be within --workspace"
                );
                let base_id: crate::operations::ResourceId = serde_json::from_value(json!(
                    format!("wb:{}", crate::utils::hash_path_identity(&source))
                ))?;
                let created_resource =
                    crate::native_resident::NativeBindings::creation_resource(&identity, &base_id)?;
                let cli = client(
                    Some(workspace.clone()),
                    &created_resource,
                    identity,
                    expected,
                )
                .await?;
                let config = crate::config::ServerConfig::from_args(crate::config::CliArgs {
                    workspace_root: Some(workspace.clone()),
                    recalc_enabled: true,
                    ..Default::default()
                })?;
                cli.host
                    .request(&HostRequest::Configure { config })
                    .await
                    .context("configure native host (not submitted)")?;
                let original = cli
                    .original()
                    .await
                    .context("lookup original session creation")?;
                let revision = match original.as_ref() {
                    Some(response) => {
                        ensure!(
                            response.operation == "create_fork"
                                && response.data["base_resource_id"] == json!(base_id),
                            "request identity belongs to another operation"
                        );
                        response.data["base_revision_id"]
                            .as_str()
                            .ok_or_else(|| anyhow!("creation receipt lacks base revision"))?
                            .to_owned()
                    }
                    None => crate::utils::hash_file_sha256_hex(&source).with_context(|| {
                        format!("base file not found or unreadable: {}", source.display())
                    })?,
                };
                let response = cli
                    .host
                    .execute(
                        base_id.as_str().into(),
                        cli.identity.clone(),
                        "create_fork".into(),
                        json!({"resource_id":base_id,"expected_revision":revision,"label":label}),
                    )
                    .await?
                    .map_err(|error| {
                        anyhow!(
                            "request {}: {}",
                            cli.identity,
                            serde_json::to_string(&error).unwrap_or_default()
                        )
                    })?;
                let mut result = cli.project(response);
                result["base_path"] = json!(source);
                result["workspace_root"] = json!(workspace);
                result["label"] = result["data"].get("label").cloned().unwrap_or(Value::Null);
                Ok(result)
            }
            SessionCommands::Exec {
                session,
                operation,
                json: payload,
                workspace,
            } => {
                let cli = client(workspace, &session, identity, expected).await?;
                Ok(cli.project(
                    cli.operation(&operation, serde_json::from_str(&payload)?)
                        .await?,
                ))
            }
            SessionCommands::Log {
                session,
                since,
                kind,
                workspace,
            } => {
                let cli = client(workspace, &session, identity, expected).await?;
                let mut result = cli.history(json!({"action":"list","limit":2000})).await?;
                let records = result["records"]
                    .as_array()
                    .ok_or_else(|| anyhow!("history has no records"))?;
                let start = since
                    .as_ref()
                    .and_then(|id| records.iter().position(|record| record["commit_id"] == *id))
                    .unwrap_or(0);
                let events = records
                    .iter()
                    .skip(start)
                    .filter(|record| {
                        matches!(
                            record["transition"].as_str(),
                            Some("mutation" | "stage_apply")
                        )
                    })
                    .filter(|record| {
                        kind.as_ref().is_none_or(|prefix| {
                            record["op_kinds"].as_array().is_some_and(|kinds| {
                                kinds
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .any(|kind| legacy_kind(kind).starts_with(prefix))
                            })
                        })
                    })
                    .map(|record| {
                        let mut record = record.clone();
                        record["kind"] = record["op_kinds"][0]
                            .as_str()
                            .map(legacy_kind)
                            .map(Value::String)
                            .unwrap_or(Value::Null);
                        record["op_id"] = record["commit_id"].clone();
                        record["parent_id"] = record["history_parent_commit_id"].clone();
                        record
                    })
                    .collect::<Vec<_>>();
                result["event_count"] = json!(events.len());
                result["events"] = json!(events);
                Ok(result)
            }
            SessionCommands::Branches { session, workspace } => {
                let cli = client(workspace, &session, identity, expected).await?;
                let mut result = cli.history(json!({"action":"list","limit":1})).await?;
                let branches = result["branches"].as_object().ok_or_else(|| anyhow!("history has no branches"))?.iter().map(|(name, tip)| json!({"name":name,"tip_op_id":tip,"label":result["branch_labels"][name],"current":result["branch"] == *name})).collect::<Vec<_>>();
                result["current_branch"] = result["branch"].clone();
                result["branches"] = json!(branches);
                Ok(result)
            }
            SessionCommands::Switch {
                session,
                branch,
                workspace,
            } => {
                client(workspace, &session, identity, expected)
                    .await?
                    .history(json!({"action":"switch_branch","name":branch}))
                    .await
            }
            SessionCommands::Checkout {
                session,
                op_id,
                workspace,
            } => {
                client(workspace, &session, identity, expected)
                    .await?
                    .history(json!({"action":"checkout","target_commit_id":op_id}))
                    .await
            }
            SessionCommands::Undo { session, workspace } => {
                let mut value = client(workspace, &session, identity, expected)
                    .await?
                    .history(json!({"action":"undo"}))
                    .await?;
                value["undone"] = json!(true);
                Ok(value)
            }
            SessionCommands::Redo { session, workspace } => {
                let mut value = client(workspace, &session, identity, expected)
                    .await?
                    .history(json!({"action":"redo"}))
                    .await?;
                value["redone"] = json!(true);
                Ok(value)
            }
            SessionCommands::Fork {
                session,
                from,
                label,
                branch_name,
                workspace,
            } => {
                let mut value = client(workspace, &session, identity, expected).await?.history(json!({"action":"create_branch","name":branch_name,"target_commit_id":from,"label":label})).await?;
                value["branch"] = json!(branch_name);
                value["label"] = json!(label);
                value["fork_point"] = json!(from);
                Ok(value)
            }
            SessionCommands::Op {
                session,
                ops,
                workspace,
            } => {
                let payload: Value =
                    serde_json::from_slice(&std::fs::read(ops.strip_prefix('@').unwrap_or(&ops))?)?;
                let ops = translate_payload(payload)?;
                let cli = client(workspace, &session, identity, expected).await?;
                let response = cli
                    .operation("write", json!({"mode":"stage","atomic":true,"ops":ops}))
                    .await?;
                let mut value = cli.project(response);
                value["staged_id"] = json!(format!(
                    "stg_{}",
                    value["data"]["change_id"]
                        .as_str()
                        .context("stage response lacks identifier")?
                ));
                value["dry_run_impact"] = value["data"]["impact"].clone();
                value["dry_run_impact"]["cells_changed"] =
                    json!(value["data"]["diff"]["changes"].as_array().map(|changes| {
                        changes
                            .iter()
                            .filter(|change| change.get("address").is_some())
                            .count()
                    }));
                Ok(value)
            }
            SessionCommands::Apply {
                session,
                staged_id,
                workspace,
            } => {
                let cli = client(workspace, &session, identity, expected).await?;
                let response = cli
                    .operation(
                        "staged_change",
                        json!({"action":"apply","change_id":staged_id.strip_prefix("stg_").unwrap_or(&staged_id)}),
                    )
                    .await?;
                let mut value = cli.project(response);
                value["staged_id"] = json!(staged_id);
                value["applied"] = json!(true);
                value["op_id"] = value["data"]["head"].clone();
                value["head"] = value["data"]["head"].clone();
                Ok(value)
            }
            SessionCommands::Materialize {
                session,
                output,
                source,
                force,
                workspace,
            } => {
                ensure!(
                    source != output.is_some(),
                    "choose exactly one of --output or --source"
                );
                let cli = client(workspace, &session, identity, expected).await?;
                let destination = output
                    .map(|path| -> Result<String> {
                        let path = if path.is_absolute() {
                            path
                        } else {
                            std::env::current_dir()?.join(path)
                        };
                        let path = path
                            .parent()
                            .context("output lacks parent")?
                            .canonicalize()?
                            .join(path.file_name().context("output lacks filename")?);
                        Ok(path.to_str().context("output is not UTF-8")?.into())
                    })
                    .transpose()?;
                let intent = crate::resident_export::FileExportIntent {
                    resource_id: cli.resource.clone(),
                    request_id: cli.identity.clone(),
                    expected_revision: cli.expected.clone(),
                    destination,
                    force,
                };
                let mut result = cli
                    .host
                    .request(&HostRequest::Materialize { intent })
                    .await
                    .map_err(|error| anyhow!("materialize request {}: {error:#}", cli.identity))?;
                result["session_id"] = json!(cli.resource);
                result["output_size_bytes"] = result["artifact"]["bytes"].clone();
                Ok(result)
            }
        }
    }

    fn legacy_kind(kind: &str) -> String {
        let family = match kind {
            "write_matrix" | "clear_range" | "fill_range" | "replace_in_range" => "transform",
            "insert_rows" | "delete_rows" | "insert_cols" | "delete_cols" | "rename_sheet"
            | "create_sheet" | "delete_sheet" | "copy_range" | "move_range" | "merge_cells"
            | "unmerge_cells" | "clone_row" | "clone_row_band" => "structure",
            _ => return kind.into(),
        };
        format!("{family}.{kind}")
    }

    // Input normalization only: every resulting operation is decoded by the
    // canonical WriteOp schema and executed by the shared stage/apply executor.
    fn translate_payload(mut payload: Value) -> Result<Vec<crate::canonical_write::WriteOp>> {
        let kind = payload["kind"]
            .as_str()
            .ok_or_else(|| anyhow!("session ops payload must include a top-level string 'kind', e.g. transform.write_matrix; see `asp example session op transform.write_matrix`"))?
            .to_owned();
        let mut operations = match kind.as_str() {
            "transform.write_matrix" => {
                payload["kind"] = json!("write_matrix");
                if payload.get("anchor").is_none() {
                    payload["anchor"] = json!("A1");
                }
                if payload.get("overwrite_formulas").is_none() {
                    payload["overwrite_formulas"] = json!(false);
                }
                for row in payload["rows"]
                    .as_array_mut()
                    .ok_or_else(|| anyhow!("rows must be an array"))?
                {
                    for cell in row
                        .as_array_mut()
                        .ok_or_else(|| anyhow!("row must be an array"))?
                    {
                        if !cell.is_null() && !cell.is_object() {
                            *cell = json!({"v":cell.clone()});
                        }
                    }
                }
                vec![payload]
            }
            "name.define" | "name.update" | "name.delete" | "formula.replace_in_formulas" => {
                payload["kind"] = json!(match kind.as_str() {
                    "name.define" => "define_name",
                    "name.update" => "update_name",
                    "name.delete" => "delete_name",
                    _ => "replace_in_formulas",
                });
                if kind == "name.define" && payload["scope"].is_null() {
                    payload["scope"] = json!("workbook");
                }
                vec![payload]
            }
            _ => {
                let mut operations = payload["ops"]
                    .as_array()
                    .ok_or_else(|| anyhow!("{kind} requires an ops array"))?
                    .clone();
                for operation in &mut operations {
                    match kind.as_str() {
                        "style.apply" => operation["kind"] = json!("style"),
                        "formula.apply_pattern" => operation["kind"] = json!("formula_pattern"),
                        "column.size" => {
                            operation["kind"] = json!("column_size");
                            operation["sheet_name"] = payload["sheet_name"].clone();
                        }
                        k if k.starts_with("structure.")
                            || k.starts_with("transform.")
                            || k == "layout.apply"
                            || k == "rules.apply" => {}
                        _ => bail!("unsupported session payload kind {kind}"),
                    }
                }
                operations
            }
        };
        ensure!(
            !operations.is_empty() && operations.len() <= 128,
            "session operations must contain 1-128 entries"
        );
        operations
            .drain(..)
            .map(|operation| Ok(serde_json::from_value(operation)?))
            .collect()
    }
}

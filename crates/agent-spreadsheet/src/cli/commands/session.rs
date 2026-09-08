//! Compatibility payload discovery for the native explicit session CLI.
//! Execution lives in `native_session` and never opens the legacy SessionStore.
pub use super::native_session::run;
use anyhow::{Result, bail};
use schemars::{JsonSchema, schema_for};
use serde_json::{Value, json};

#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct SessionOpsPayload<T> {
    ops: Vec<T>,
}
#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct SessionWriteMatrixPayloadSchema {
    sheet_name: String,
    anchor: String,
    rows: Vec<Vec<Option<crate::core::session::SessionMatrixCell>>>,
    overwrite_formulas: bool,
}
#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct SessionColumnSizePayloadSchema {
    sheet_name: String,
    ops: Vec<crate::tools::fork::ColumnSizeOp>,
}
#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct SessionNameDefinePayloadSchema {
    name: String,
    refers_to: String,
    scope: Option<String>,
    scope_sheet_name: Option<String>,
}
#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct SessionNameUpdatePayloadSchema {
    name: String,
    refers_to: Option<String>,
    scope: Option<String>,
    scope_sheet_name: Option<String>,
}
#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct SessionNameDeletePayloadSchema {
    name: String,
    scope: Option<String>,
    scope_sheet_name: Option<String>,
}

pub fn session_payload_schema(kind: String) -> Result<Value> {
    let kind = normalize_session_payload_kind(&kind)?;
    Ok(
        json!({"schema_kind":"session_ops_payload","op_kind":kind,"schema":session_payload_schema_json(&kind)?,"notes":session_payload_notes(&kind)}),
    )
}
pub fn session_payload_example(kind: String) -> Result<Value> {
    let kind = normalize_session_payload_kind(&kind)?;
    Ok(
        json!({"example_kind":"session_ops_payload","op_kind":kind,"example":session_payload_example_json(&kind)?,"notes":session_payload_notes(&kind)}),
    )
}
fn normalize_session_payload_kind(kind: &str) -> Result<String> {
    match kind {
        k if k.starts_with("structure.")
            || matches!(
                k,
                "transform.write_matrix"
                    | "transform.clear_range"
                    | "transform.fill_range"
                    | "transform.replace_in_range"
                    | "style.apply"
                    | "formula.apply_pattern"
                    | "formula.replace_in_formulas"
                    | "column.size"
                    | "layout.apply"
                    | "rules.apply"
                    | "name.define"
                    | "name.update"
                    | "name.delete"
            ) =>
        {
            Ok(kind.to_string())
        }
        _ => bail!(
            "invalid argument: unsupported session payload kind '{kind}'. Try `asp example session op transform.write_matrix` or `asp schema session op structure.insert_rows`"
        ),
    }
}
fn session_payload_schema_json(kind: &str) -> Result<Value> {
    let schema = match kind {
        "transform.write_matrix" => {
            serde_json::to_value(schema_for!(SessionWriteMatrixPayloadSchema))?
        }
        k if k.starts_with("structure.") => serde_json::to_value(schema_for!(
            SessionOpsPayload<crate::tools::fork::StructureOp>
        ))?,
        "transform.clear_range" | "transform.fill_range" | "transform.replace_in_range" => {
            serde_json::to_value(schema_for!(
                SessionOpsPayload<crate::tools::fork::TransformOp>
            ))?
        }
        "style.apply" => {
            serde_json::to_value(schema_for!(SessionOpsPayload<crate::tools::fork::StyleOp>))?
        }
        "formula.apply_pattern" => serde_json::to_value(schema_for!(
            SessionOpsPayload<crate::tools::fork::ApplyFormulaPatternOpInput>
        ))?,
        "formula.replace_in_formulas" => {
            serde_json::to_value(schema_for!(crate::tools::fork::ReplaceInFormulasOp))?
        }
        "column.size" => serde_json::to_value(schema_for!(SessionColumnSizePayloadSchema))?,
        "layout.apply" => serde_json::to_value(schema_for!(
            SessionOpsPayload<crate::tools::sheet_layout::SheetLayoutOp>
        ))?,
        "rules.apply" => serde_json::to_value(schema_for!(
            SessionOpsPayload<crate::tools::rules_batch::RulesOp>
        ))?,
        "name.define" => serde_json::to_value(schema_for!(SessionNameDefinePayloadSchema))?,
        "name.update" => serde_json::to_value(schema_for!(SessionNameUpdatePayloadSchema))?,
        "name.delete" => serde_json::to_value(schema_for!(SessionNameDeletePayloadSchema))?,
        _ => bail!("invalid argument: unsupported session payload kind '{kind}'"),
    };
    Ok(inject_kind_property(schema, kind))
}
fn inject_kind_property(mut schema: Value, kind: &str) -> Value {
    let Some(obj) = schema.as_object_mut() else {
        return schema;
    };
    obj.entry("properties").or_insert_with(|| json!({})).as_object_mut().expect("properties object")
        .insert("kind".into(), json!({"type":"string","const":kind,"description":"Top-level session op discriminator"}));
    let required = obj
        .entry("required")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("required array");
    if !required.iter().any(|entry| entry.as_str() == Some("kind")) {
        required.push(json!("kind"));
    }
    schema
}
fn session_payload_example_json(kind: &str) -> Result<Value> {
    Ok(match kind {
        "transform.write_matrix" => {
            json!({"kind":kind,"sheet_name":"Sheet1","anchor":"B7","rows":[[{"v":"Revenue"},{"v":100}]],"overwrite_formulas":false})
        }
        "structure.insert_rows" => {
            json!({"kind":kind,"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":12,"count":2}]})
        }
        "structure.clone_row" => {
            json!({"kind":kind,"ops":[{"kind":"clone_row","sheet_name":"Sheet1","source_row":12,"insert_at":13,"count":1,"expand_adjacent_sums":true}]})
        }
        "structure.copy_range" => {
            json!({"kind":kind,"ops":[{"kind":"copy_range","sheet_name":"Sheet1","src_range":"A1:C3","dest_anchor":"E1","include_styles":true,"include_formulas":true}]})
        }
        "structure.move_range" => {
            json!({"kind":kind,"ops":[{"kind":"move_range","sheet_name":"Sheet1","src_range":"A10:B12","dest_anchor":"D10"}]})
        }
        k if k.starts_with("structure.") => {
            json!({"kind":kind,"ops":[{"kind":k.trim_start_matches("structure."),"sheet_name":"Sheet1"}]})
        }
        "transform.clear_range" => {
            json!({"kind":kind,"ops":[{"kind":"clear_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A2:C10"},"clear_values":true,"clear_formulas":false}]})
        }
        "transform.fill_range" => {
            json!({"kind":kind,"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"range","range":"B2:B10"},"value":"Filled"}]})
        }
        "transform.replace_in_range" => {
            json!({"kind":kind,"ops":[{"kind":"replace_in_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A2:A10"},"find":"Old","replace":"New","match_mode":"exact"}]})
        }
        "style.apply" => {
            json!({"kind":kind,"ops":[{"sheet_name":"Sheet1","target":{"kind":"range","range":"A1:C1"},"patch":{"font":{"bold":true}}}]})
        }
        "formula.apply_pattern" => {
            json!({"kind":kind,"ops":[{"sheet_name":"Sheet1","target_range":"C2:C10","anchor_cell":"C2","base_formula":"=A2+B2","fill_direction":"down","relative_mode":"excel"}]})
        }
        "formula.replace_in_formulas" => {
            json!({"kind":kind,"sheet_name":"Sheet1","find":"Sheet1!","replace":"Sheet2!","range":"A1:Z100","regex":false,"case_sensitive":true})
        }
        "column.size" => {
            json!({"kind":kind,"sheet_name":"Sheet1","ops":[{"target":{"kind":"columns","range":"A:C"},"size":{"kind":"width","width_chars":18.0}}]})
        }
        "layout.apply" => {
            json!({"kind":kind,"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]})
        }
        "rules.apply" => {
            json!({"kind":kind,"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B10","validation":{"kind":"list","formula1":"\"A,B,C\""}}]})
        }
        "name.define" => {
            json!({"kind":kind,"name":"SalesTotal","refers_to":"Sheet1!$C$100","scope":"workbook"})
        }
        "name.update" => {
            json!({"kind":kind,"name":"SalesTotal","refers_to":"Sheet1!$C$101","scope":"workbook"})
        }
        "name.delete" => json!({"kind":kind,"name":"SalesTotal","scope":"workbook"}),
        _ => bail!("invalid argument: unsupported session payload kind '{kind}'"),
    })
}
fn session_payload_notes(kind: &str) -> Vec<String> {
    match kind {
        "transform.write_matrix" => vec![
            "Flat payload: do not wrap transform.write_matrix in an ops array.".into(),
            "rows entries use {'v': ...} for values and {'f': ...} for formulas.".into(),
        ],
        k if k.starts_with("structure.")
            || matches!(
                k,
                "transform.clear_range"
                    | "transform.fill_range"
                    | "transform.replace_in_range"
                    | "style.apply"
                    | "formula.apply_pattern"
                    | "layout.apply"
                    | "rules.apply"
            ) =>
        {
            vec![
                "Batch envelope: use a top-level kind plus an ops array.".into(),
                "The inner ops array carries the operation-specific kind/value shape.".into(),
            ]
        }
        "column.size" => {
            vec!["column.size requires both a top-level sheet_name and an ops array.".into()]
        }
        "formula.replace_in_formulas" | "name.define" | "name.update" | "name.delete" => {
            vec!["Flat payload: do not wrap this kind in an ops array.".into()]
        }
        _ => Vec::new(),
    }
}

#![cfg(feature = "recalc")]

use anyhow::Result;
use serde_json::json;
use agent_spreadsheet_mcp::model::{FillPatch, FontPatch, PatternFillPatch, StylePatch};
use agent_spreadsheet_mcp::styles::StylePatchMode;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, StyleBatchParamsInput, StyleOp, StyleTarget,
    apply_staged_change, create_fork, normalize_style_batch, style_batch,
};
use agent_spreadsheet_mcp::tools::param_enums::BatchMode;
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, list_workbooks};
use umya_spreadsheet::{
    ConditionalFormatValues, ConditionalFormatting, ConditionalFormattingRule, Formula,
    PatternValues,
};

mod support;

fn recalc_state(
    workspace: &support::TestWorkspace,
) -> std::sync::Arc<agent_spreadsheet_mcp::state::AppState> {
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
    });
    support::app_state_with_config(config)
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_merge_set_clear_semantics() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("style.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        let style = sheet.style_mut("A1");
        style.font_mut().set_bold(true);
        style
            .fill_mut()
            .pattern_fill_mut()
            .set_pattern_type(umya_spreadsheet::PatternValues::Solid)
            .foreground_color_mut()
            .set_argb_str("FF0000FF");
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    // Merge: remove bold (explicit false) and clear fill.
    let patch_merge = StylePatch {
        font: Some(Some(FontPatch {
            bold: Some(Some(false)),
            ..Default::default()
        })),
        fill: Some(None),
        ..Default::default()
    };

    style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Range {
                        range: "A1:A1".to_string(),
                    },
                    patch: patch_merge,
                    op_mode: Some(StylePatchMode::Merge),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1 cell").style(),
        )
    })?;
    assert!(desc_a1.font.as_ref().and_then(|f| f.bold).is_none());
    assert!(desc_a1.fill.is_none());

    // Set: apply solid red fill only, wiping other direct formatting.
    let patch_set = StylePatch {
        fill: Some(Some(agent_spreadsheet_mcp::model::FillPatch::Pattern(
            PatternFillPatch {
                pattern_type: Some(Some("solid".to_string())),
                foreground_color: Some(Some("FFFF0000".to_string())),
                ..Default::default()
            },
        ))),
        ..Default::default()
    };

    style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Cells {
                        cells: vec!["A1".to_string()],
                    },
                    patch: patch_set,
                    op_mode: Some(StylePatchMode::Set),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1 cell").style(),
        )
    })?;
    assert!(desc_a1.font.as_ref().and_then(|f| f.bold).is_none());
    let fill = desc_a1.fill.as_ref().expect("fill");
    match fill {
        agent_spreadsheet_mcp::model::FillDescriptor::Pattern(p) => {
            assert_eq!(p.foreground_color.as_deref(), Some("FFFF0000"));
        }
        _ => panic!("expected pattern fill"),
    }

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_preview_stages_and_apply() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("preview.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let patch = StylePatch {
        font: Some(Some(FontPatch {
            bold: Some(Some(true)),
            ..Default::default()
        })),
        ..Default::default()
    };

    let preview = style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Cells {
                        cells: vec!["A1".to_string()],
                    },
                    patch,
                    op_mode: None,
                }
                .into(),
            ],
            mode: Some(BatchMode::Preview),
            label: Some("bold headers".to_string()),
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    // Preview should not mutate the fork.
    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1 cell").style(),
        )
    })?;
    assert!(desc_a1.font.is_none());

    apply_staged_change(
        state.clone(),
        ApplyStagedChangeParams {
            fork_id: fork.fork_id.clone(),
            change_id,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1 cell").style(),
        )
    })?;
    assert_eq!(desc_a1.font.as_ref().and_then(|f| f.bold), Some(true));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_overlap_ordering_last_wins() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("overlap.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for addr in ["A1", "B1", "C1", "A2"] {
            sheet.cell_mut(addr).set_value("x");
        }
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let base_fill = StylePatch {
        fill: Some(Some(FillPatch::Pattern(PatternFillPatch {
            pattern_type: Some(Some("solid".to_string())),
            foreground_color: Some(Some("FFCCE5FF".to_string())),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let header_bold = StylePatch {
        font: Some(Some(FontPatch {
            bold: Some(Some(true)),
            ..Default::default()
        })),
        ..Default::default()
    };

    style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Range {
                        range: "A1:C3".to_string(),
                    },
                    patch: base_fill,
                    op_mode: Some(StylePatchMode::Set),
                }
                .into(),
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Range {
                        range: "A1:C1".to_string(),
                    },
                    patch: header_bold,
                    op_mode: Some(StylePatchMode::Merge),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1").style(),
        )
    })?;
    let desc_a2 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A2").expect("A2").style(),
        )
    })?;

    assert_eq!(desc_a1.font.as_ref().and_then(|f| f.bold), Some(true));
    assert!(desc_a1.fill.is_some());
    assert!(desc_a2.fill.is_some());
    assert!(desc_a2.font.as_ref().and_then(|f| f.bold).is_none());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_nested_null_clear_only_subfield() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("null_clear.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        let style = sheet.style_mut("A1");
        style.font_mut().set_bold(true);
        style.font_mut().color_mut().set_argb_str("FFFF0000");
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let patch = StylePatch {
        font: Some(Some(FontPatch {
            color: Some(None),
            ..Default::default()
        })),
        ..Default::default()
    };

    style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Cells {
                        cells: vec!["A1".to_string()],
                    },
                    patch,
                    op_mode: Some(StylePatchMode::Merge),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1").style(),
        )
    })?;
    assert_eq!(desc_a1.font.as_ref().and_then(|f| f.bold), Some(true));
    assert!(
        desc_a1
            .font
            .as_ref()
            .and_then(|f| f.color.clone())
            .is_none()
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_region_target_resolves() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("region.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("H1");
        sheet.cell_mut("B1").set_value("H2");
        sheet.cell_mut("C1").set_value("H3");
        for r in 2..=5 {
            sheet
                .cell_mut(format!("A{r}").as_str())
                .set_value_number(r);
            sheet
                .cell_mut(format!("B{r}").as_str())
                .set_value_number(r);
            sheet
                .cell_mut(format!("C{r}").as_str())
                .set_value_number(r);
        }
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let metrics = fork_wb.get_sheet_metrics("Sheet1")?;
    let regions = metrics.detected_regions();
    let region_id = regions.first().expect("region detected").id;

    let patch = StylePatch {
        font: Some(Some(FontPatch {
            bold: Some(Some(true)),
            ..Default::default()
        })),
        ..Default::default()
    };

    style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Region { region_id },
                    patch,
                    op_mode: Some(StylePatchMode::Merge),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let desc_a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(
            sheet.cell("A1").expect("A1").style(),
        )
    })?;
    let desc_j1 = fork_wb.with_sheet("Sheet1", |sheet| {
        agent_spreadsheet_mcp::styles::descriptor_from_style(sheet.style("J1"))
    })?;
    assert_eq!(desc_a1.font.as_ref().and_then(|f| f.bold), Some(true));
    assert!(desc_j1.font.is_none());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_idempotent_noop_counts_and_no_diff() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("noop.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.style_mut("A1").font_mut().set_bold(true);
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let patch = StylePatch {
        font: Some(Some(FontPatch {
            bold: Some(Some(true)),
            ..Default::default()
        })),
        ..Default::default()
    };

    let resp = style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Cells {
                        cells: vec!["A1".to_string()],
                    },
                    patch,
                    op_mode: Some(StylePatchMode::Merge),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_style_changed").copied(),
        Some(0)
    );

    let changes = agent_spreadsheet_mcp::tools::fork::get_changeset(
        state.clone(),
        agent_spreadsheet_mcp::tools::fork::GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: None,
            ..Default::default()
        },
    )
    .await?;
    use agent_spreadsheet_mcp::diff::Change;
    use agent_spreadsheet_mcp::diff::merge::{CellDiff, ModificationType};
    let non_style_change = changes.changes.iter().any(|c| match c {
        Change::Cell(cell) => match &cell.diff {
            CellDiff::Modified { subtype, .. } => !matches!(subtype, ModificationType::StyleEdit),
            CellDiff::Added { .. } | CellDiff::Deleted { .. } => true,
        },
        Change::Table(_) | Change::Name(_) => true,
    });
    assert!(!non_style_change);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_preserves_conditional_formats() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cf.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(1);

        let mut cf = ConditionalFormatting::default();
        cf.sequence_of_references_mut().set_sqref("A1:A3");

        let mut rule = ConditionalFormattingRule::default();
        rule.set_type(ConditionalFormatValues::Expression);
        rule.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("A1>0");
        rule.set_formula(formula);

        let mut style = umya_spreadsheet::Style::default();
        style
            .fill_mut()
            .pattern_fill_mut()
            .set_pattern_type(PatternValues::Solid)
            .foreground_color_mut()
            .set_argb_str("FFFFFF00");
        rule.set_style(style);

        cf.add_conditional_collection(rule);
        sheet.add_conditional_formatting_collection(cf);
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let patch = StylePatch {
        font: Some(Some(FontPatch {
            italic: Some(Some(true)),
            ..Default::default()
        })),
        ..Default::default()
    };

    style_batch(
        state.clone(),
        StyleBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StyleOp {
                    sheet_name: "Sheet1".to_string(),
                    target: StyleTarget::Range {
                        range: "A1:A3".to_string(),
                    },
                    patch,
                    op_mode: Some(StylePatchMode::Merge),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(fork.fork_id.clone()))
        .await?;
    let cf_count = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.conditional_formatting_collection().len()
    })?;
    assert_eq!(cf_count, 1);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn style_batch_number_format_shorthand_applies_and_is_idempotent() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("numfmt.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(123.45);
        sheet.cell_mut("B1").set_value_number(0.25);
        sheet.cell_mut("C1").set_value_number(45123.0);
        sheet.cell_mut("D1").set_value_number(123.45);
        sheet.cell_mut("E1").set_value_number(42);
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let fork_id = fork.fork_id;
    let input = json!({
        "fork_id": fork_id,
        "mode": "apply",
        "ops": [
            { "sheet_name": "Sheet1", "range": "A1:A1", "style": { "number_format": { "kind": "currency" } } },
            { "sheet_name": "Sheet1", "range": "B1:B1", "style": { "number_format": { "kind": "percent" } } },
            { "sheet_name": "Sheet1", "range": "C1:C1", "style": { "number_format": { "kind": "date_iso" } } },
            { "sheet_name": "Sheet1", "range": "D1:D1", "style": { "number_format": { "kind": "accounting" } } },
            { "sheet_name": "Sheet1", "range": "E1:E1", "style": { "number_format": { "kind": "integer" } } }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input.clone()).unwrap();
    style_batch(state.clone(), params).await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(
            input["fork_id"].as_str().unwrap().to_string(),
        ))
        .await?;
    let expect = [
        ("A1", "$#,##0.00"),
        ("B1", "0.00%"),
        ("C1", "yyyy-mm-dd"),
        ("D1", "_($* #,##0.00_)"),
        ("E1", "0"),
    ];
    for (cell, expected_fmt) in expect {
        let desc = fork_wb.with_sheet("Sheet1", |sheet| {
            agent_spreadsheet_mcp::styles::descriptor_from_style(
                sheet.cell(cell).unwrap().style(),
            )
        })?;
        assert_eq!(desc.number_format.as_deref(), Some(expected_fmt));
    }

    // Apply the same shorthand again; should be a no-op.
    let params2: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let resp2 = style_batch(state.clone(), params2).await?;
    assert_eq!(
        resp2.summary.counts.get("cells_style_changed").copied(),
        Some(0)
    );

    Ok(())
}

#[test]
fn style_batch_accepts_range_and_style_shorthand() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "A2:F2",
                "style": {
                    "font": { "bold": true }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    assert_eq!(normalized.ops.len(), 1);
    let op = &normalized.ops[0];
    assert_eq!(op.sheet_name, "Accounts");
    match &op.target {
        StyleTarget::Range { range } => assert_eq!(range, "A2:F2"),
        _ => panic!("expected range target"),
    }
    let bold = op
        .patch
        .font
        .as_ref()
        .and_then(|font| font.as_ref())
        .and_then(|font| font.bold.as_ref())
        .and_then(|bold| *bold);
    assert_eq!(bold, Some(true));
    assert!(warnings.iter().any(|w| w.code == "WARN_STYLE_SHORTHAND"));
}

#[test]
fn style_batch_normalizes_number_format_shorthand_to_format_code() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "B3:B3",
                "style": {
                    "number_format": { "kind": "currency" }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, _warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    assert_eq!(
        op.patch
            .number_format
            .as_ref()
            .and_then(|nf| nf.as_ref().map(String::as_str)),
        Some("$#,##0.00")
    );
}

#[test]
fn style_batch_number_format_format_code_takes_precedence_over_kind() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "B3:B3",
                "style": {
                    "number_format": { "kind": "percent", "format_code": "0.000%" }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, _warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    assert_eq!(
        op.patch
            .number_format
            .as_ref()
            .and_then(|nf| nf.as_ref().map(String::as_str)),
        Some("0.000%")
    );
}

#[test]
fn style_batch_number_format_explicit_string_is_preserved() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "B3:B3",
                "style": {
                    "number_format": "0.0000"
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, _warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    assert_eq!(
        op.patch
            .number_format
            .as_ref()
            .and_then(|nf| nf.as_ref().map(String::as_str)),
        Some("0.0000")
    );
}

#[test]
fn style_batch_normalizes_fill_color_shorthand() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "A2:F2",
                "style": {
                    "fill": { "color": "#F2F2F2" }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    let fill = op
        .patch
        .fill
        .as_ref()
        .and_then(|fill| fill.as_ref())
        .expect("fill");
    match fill {
        FillPatch::Pattern(pattern) => {
            assert_eq!(
                pattern
                    .pattern_type
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("solid")
            );
            assert_eq!(
                pattern
                    .foreground_color
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("FFF2F2F2")
            );
        }
        _ => panic!("expected pattern fill"),
    }

    assert!(warnings.iter().any(|w| w.code == "WARN_FILL_COLOR"));
}

#[test]
fn style_batch_expands_rgb_hex_to_argb() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "A2:F2",
                "style": {
                    "fill": { "color": "#F2F2F2" }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    let fill = op
        .patch
        .fill
        .as_ref()
        .and_then(|fill| fill.as_ref())
        .expect("fill");
    match fill {
        FillPatch::Pattern(pattern) => {
            assert_eq!(
                pattern
                    .foreground_color
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("FFF2F2F2")
            );
        }
        _ => panic!("expected pattern fill"),
    }

    assert!(
        warnings
            .iter()
            .any(|w| w.code == "WARN_COLOR_ALPHA_DEFAULT")
    );
}

#[test]
fn style_batch_expands_short_rgb_hex_to_argb() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "range": "A2:F2",
                "style": {
                    "fill": { "color": "#F2F" }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    let fill = op
        .patch
        .fill
        .as_ref()
        .and_then(|fill| fill.as_ref())
        .expect("fill");
    match fill {
        FillPatch::Pattern(pattern) => {
            assert_eq!(
                pattern
                    .foreground_color
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("FFFF22FF")
            );
        }
        _ => panic!("expected pattern fill"),
    }

    assert!(
        warnings
            .iter()
            .any(|w| w.code == "WARN_COLOR_ALPHA_DEFAULT")
    );
}

#[test]
fn style_batch_preserves_argb() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "target": { "kind": "range", "range": "A2:F2" },
                "patch": {
                    "fill": {
                        "kind": "pattern",
                        "pattern_type": "solid",
                        "foreground_color": "80FF0000"
                    }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    let fill = op
        .patch
        .fill
        .as_ref()
        .and_then(|fill| fill.as_ref())
        .expect("fill");
    match fill {
        FillPatch::Pattern(pattern) => {
            assert_eq!(
                pattern
                    .foreground_color
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("80FF0000")
            );
        }
        _ => panic!("expected pattern fill"),
    }

    assert!(
        !warnings
            .iter()
            .any(|w| w.code == "WARN_COLOR_ALPHA_DEFAULT")
    );
}

#[test]
fn style_batch_warns_once_for_multiple_rgb_colors() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "target": { "kind": "range", "range": "A2:F2" },
                "patch": {
                    "font": { "color": "#112233" },
                    "fill": {
                        "kind": "pattern",
                        "pattern_type": "solid",
                        "foreground_color": "#445566",
                        "background_color": "#778899"
                    }
                }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    let op = &normalized.ops[0];
    let font_color = op
        .patch
        .font
        .as_ref()
        .and_then(|font| font.as_ref())
        .and_then(|font| font.color.as_ref())
        .and_then(|color| color.as_ref().map(String::as_str));
    assert_eq!(font_color, Some("FF112233"));

    let fill = op
        .patch
        .fill
        .as_ref()
        .and_then(|fill| fill.as_ref())
        .expect("fill");
    match fill {
        FillPatch::Pattern(pattern) => {
            assert_eq!(
                pattern
                    .foreground_color
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("FF445566")
            );
            assert_eq!(
                pattern
                    .background_color
                    .as_ref()
                    .and_then(|v| v.as_ref().map(String::as_str)),
                Some("FF778899")
            );
        }
        _ => panic!("expected pattern fill"),
    }

    let color_warnings = warnings
        .iter()
        .filter(|w| w.code == "WARN_COLOR_ALPHA_DEFAULT")
        .count();
    assert_eq!(color_warnings, 1);
}

#[test]
fn style_batch_handles_mixed_shorthand_and_canonical_ops() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "sheet_name": "Accounts",
                "target": { "kind": "range", "range": "A1:A1" },
                "patch": { "font": { "bold": true } }
            },
            {
                "sheet_name": "Accounts",
                "range": "B1:B1",
                "style": { "font": { "bold": false } }
            }
        ]
    });

    let params: StyleBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_style_batch(params).unwrap();

    assert_eq!(normalized.ops.len(), 2);
    let first = &normalized.ops[0];
    let second = &normalized.ops[1];

    match &first.target {
        StyleTarget::Range { range } => assert_eq!(range, "A1:A1"),
        _ => panic!("expected range target"),
    }
    let first_bold = first
        .patch
        .font
        .as_ref()
        .and_then(|font| font.as_ref())
        .and_then(|font| font.bold.as_ref())
        .and_then(|bold| *bold);
    assert_eq!(first_bold, Some(true));

    match &second.target {
        StyleTarget::Range { range } => assert_eq!(range, "B1:B1"),
        _ => panic!("expected range target"),
    }
    let second_bold = second
        .patch
        .font
        .as_ref()
        .and_then(|font| font.as_ref())
        .and_then(|font| font.bold.as_ref())
        .and_then(|bold| *bold);
    assert_eq!(second_bold, Some(false));

    let shorthand_warnings = warnings
        .iter()
        .filter(|w| w.code == "WARN_STYLE_SHORTHAND")
        .count();
    assert_eq!(shorthand_warnings, 1);
}

#![cfg(feature = "recalc-formualizer")]
#[path = "support/source_xlsx.rs"]
mod fixture;
use agent_spreadsheet::stateless_read::*;
use serde_json::{Value, json};
fn inspect(bytes: &[u8], request: Value) -> ReadReport {
    inspect_xlsx_bytes(bytes, serde_json::from_value(request).unwrap()).unwrap()
}
fn trace(bytes: &[u8], request: Value) -> ReadReport {
    trace_xlsx_bytes(bytes, serde_json::from_value(request).unwrap()).unwrap()
}
fn chain() -> Vec<u8> {
    fixture::pack(&fixture::parts(
        "<row r=\"1\"><c r=\"A1\"><v>21</v></c><c r=\"B1\"><f>A1*2</f><v>0</v></c><c r=\"C1\"><f>B1+1</f><v>0</v></c></row>",
    ))
}
#[test]
fn native_style_free_source_cache_is_separate_from_unevaluated_engine() {
    let bytes = chain();
    let copy = bytes.clone();
    let source = inspect(
        &bytes,
        json!({"selection":{"kind":"cells","addresses":["Sheet1!B1","Sheet1!A1"]}}),
    );
    assert_eq!(
        source.items[1]["stored_formula_cache"]["freshness"],
        "unverified"
    );
    assert_eq!(
        source.items[1]["stored_formula_cache"]["value"]["value"],
        0.0
    );
    assert!(source.items[1]["source_value"].is_null());
    let engine = trace(&bytes, json!({"roots":["Sheet1!B1"]}));
    assert!(engine.items[0]["cell"]["value"].is_null());
    assert_eq!(engine.items[0]["cell"]["staleness"], "NeverEvaluated");
    assert_eq!(engine.items[1]["cell"]["value"]["value"], 21.0);
    assert_eq!(engine.items[1]["cell"]["staleness"], "Current");
    assert!(!engine.evaluated);
    assert_eq!(bytes, copy);
}
#[test]
fn one_hop_reroot_and_literal_expansion_budget() {
    let bytes = chain();
    let first = trace(&bytes, json!({"roots":["Sheet1!C1"]}));
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items[1]["cell"]["address"], "Sheet1!B1");
    assert_eq!(
        first.items[0]["links"][0]["targets"][0]["disposition"],
        "Elided"
    );
    let reroot = trace(&bytes, first.items[1]["next_trace_request"].clone());
    assert_eq!(reroot.items[1]["cell"]["address"], "Sheet1!A1");
    let deep = trace(
        &bytes,
        json!({"roots":["Sheet1!C1"],"limits":{"max_depth":1}}),
    );
    assert_eq!(deep.items.len(), 3);
    assert_eq!(deep.coverage["engine_limits"]["max_depth"], 1);
}
#[test]
fn finite_coordinate_pages_deduplicate_and_bind_file_scope_and_settings() {
    let bytes = chain();
    let mut request =
        json!({"selection":{"kind":"range","address":"Sheet1!A1:B3"},"page":{"limit":2}});
    let mut seen = Vec::new();
    loop {
        let report = inspect(&bytes, request.clone());
        seen.extend(
            report
                .items
                .iter()
                .map(|v| v["address"].as_str().unwrap().to_string()),
        );
        if let Some(next) = report.next_request {
            request = next;
        } else {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![
            "Sheet1!A1",
            "Sheet1!B1",
            "Sheet1!A2",
            "Sheet1!B2",
            "Sheet1!A3",
            "Sheet1!B3"
        ]
    );
    let first = inspect(
        &bytes,
        json!({"selection":{"kind":"cells","addresses":["Sheet1!B1","Sheet1!A1","Sheet1!A1"]},"page":{"limit":1}}),
    );
    let next = first.next_request.unwrap();
    assert_eq!(
        inspect(&bytes, next.clone()).items[0]["address"],
        "Sheet1!B1"
    );
    let mut changed = bytes.clone();
    changed.push(0);
    assert!(matches!(
        inspect_xlsx_bytes(&changed, serde_json::from_value(next.clone()).unwrap()),
        Err(ReadError::Stale)
    ));
    let mut different = next;
    different["page"]["limit"] = json!(2);
    assert!(matches!(
        inspect_xlsx_bytes(&bytes, serde_json::from_value(different).unwrap()),
        Err(ReadError::Stale)
    ));
    let large = inspect(
        &bytes,
        json!({"selection":{"kind":"range","address":"Sheet1!A1:XFD1048576"},"page":{"limit":2}}),
    );
    assert_eq!(large.items.len(), 2);
    assert_eq!(large.total_page_items, 17179869184);
}
#[test]
fn trace_pages_cover_only_materialized_nodes_and_reject_stale_scope() {
    let bytes = chain();
    let first = trace(&bytes, json!({"roots":["Sheet1!C1"],"page":{"limit":1}}));
    assert_eq!(first.remaining_page_items, 1);
    let next = first.next_request.unwrap();
    let second = trace(&bytes, next.clone());
    assert_ne!(
        first.items[0]["cell"]["address"],
        second.items[0]["cell"]["address"]
    );
    assert!(second.next_request.is_none());
    let mut changed = next;
    changed["limits"]["max_depth"] = json!(2);
    assert!(matches!(
        trace_xlsx_bytes(&bytes, serde_json::from_value(changed).unwrap()),
        Err(ReadError::Stale)
    ));
}
#[test]
fn quoted_cross_sheet_relative_families_and_dependents() {
    let mut parts = fixture::stale_parts();
    parts.insert(
        "xl/workbook.xml".into(),
        parts["xl/workbook.xml"].replace("Sheet1", "O&apos;Brien Space"),
    );
    parts.insert(fixture::SHEET.into(),format!("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><v>21</v></c><c r=\"B1\"><f>A1*2</f><v>0</v></c></row><row r=\"2\"><c r=\"A2\"><v>30</v></c><c r=\"B2\"><f>A2*2</f><v>0</v></c></row></sheetData></worksheet>"));
    let bytes = fixture::pack(&parts);
    for (root, target) in [("B1", "A1"), ("B2", "A2")] {
        let report = trace(
            &bytes,
            json!({"roots":[format!("'O''Brien Space'!{root}")]}),
        );
        assert_eq!(
            report.items[1]["cell"]["address"],
            format!("'O''Brien Space'!{target}")
        );
    }
    let dependents = trace(
        &bytes,
        json!({"roots":["'O''Brien Space'!A2"],"direction":"dependents"}),
    );
    assert_eq!(
        dependents.items[1]["cell"]["address"],
        "'O''Brien Space'!B2"
    );
}
#[test]
fn cycles_shared_targets_ranges_and_unresolved_are_factual() {
    let bytes = fixture::pack(&fixture::parts(
        "<row r=\"1\"><c r=\"A1\"><f>B1</f><v>0</v></c><c r=\"B1\"><f>A1</f><v>0</v></c><c r=\"C1\"><f>A1</f><v>0</v></c><c r=\"D1\"><f>SUM(A2:A100000)</f><v>0</v></c><c r=\"E1\"><f>MissingName</f><v>0</v></c></row>",
    ));
    let cycle = trace(
        &bytes,
        json!({"roots":["Sheet1!A1"],"limits":{"max_depth":2}}),
    );
    assert!(serde_json::to_string(&cycle).unwrap().contains("Cycle"));
    let shared = trace(&bytes, json!({"roots":["Sheet1!B1","Sheet1!C1"]}));
    assert_eq!(shared.total_page_items, 3);
    assert!(
        serde_json::to_string(&shared)
            .unwrap()
            .contains("Convergent")
    );
    let range = trace(&bytes, json!({"roots":["Sheet1!D1"]}));
    assert_eq!(range.total_page_items, 1);
    assert_eq!(range.items[0]["links"][0]["reference"]["cell_count"], 99999);
    assert_eq!(range.hints[0]["kind"], "inspect_range");
    let unknown = trace(&bytes, json!({"roots":["Sheet1!E1"]}));
    assert!(
        serde_json::to_string(&unknown)
            .unwrap()
            .to_lowercase()
            .contains("unresolved")
    );
}
#[test]
fn invalid_or_unsupported_requests_do_not_pretend_empty_success() {
    assert!(
        serde_json::from_value::<TraceRequest>(json!({"roots":["Sheet1!B1"],"calculate":true}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<InspectRequest>(
            json!({"selection":{"kind":"cells","addresses":["Sheet1!A1"]},"unknown":1})
        )
        .is_err()
    );
    let bytes = chain();
    assert!(
        inspect_xlsx_bytes(
            &bytes,
            serde_json::from_value(json!({"selection":{"kind":"range","address":"Sheet1!A:A"}}))
                .unwrap()
        )
        .is_err()
    );
    assert!(
        trace_xlsx_bytes(&bytes, serde_json::from_value(json!({"roots":[]})).unwrap()).is_err()
    );
    let limited = trace(
        &bytes,
        json!({"roots":["Sheet1!C1"],"limits":{"max_work":0}}),
    );
    assert_eq!(limited.coverage["engine_truncation"]["incomplete"], true);
    assert_eq!(
        limited.coverage["engine_truncation"]["omitted"]["kind"],
        "at_least"
    );
    let boundary = trace(&bytes, json!({"roots":["Sheet1!C1"]}));
    assert_eq!(
        boundary.coverage["engine_truncation"]["omitted"]["kind"],
        "unknown"
    );
    let nodes_limited = trace(
        &bytes,
        json!({"roots":["Sheet1!C1"],"limits":{"max_nodes":1}}),
    );
    assert_eq!(
        nodes_limited.items[0]["links"][0]["omitted"]["kind"],
        "at_least"
    );
    let leaf = trace(&bytes, json!({"roots":["Sheet1!A1"]}));
    assert!(leaf.coverage["transitive_graph_complete"].is_null());
    assert!(leaf.coverage["engine_truncation"]["omitted"].is_null());
    assert_eq!(limited.coverage["stop_reason"], "not_reported_by_engine");
}

#[test]
fn shared_formula_members_and_cross_sheet_reroot_use_native_engine_addresses() {
    let mut parts = fixture::parts(
        "<row r=\"1\"><c r=\"A1\"><v>2</v></c><c r=\"B1\"><f t=\"shared\" si=\"0\" ref=\"B1:B3\">A1*2</f><v>0</v></c></row><row r=\"2\"><c r=\"A2\"><v>3</v></c><c r=\"B2\"><f t=\"shared\" si=\"0\"/><v>0</v></c></row><row r=\"3\"><c r=\"A3\"><v>4</v></c><c r=\"B3\"><f t=\"shared\" si=\"0\"/><v>0</v></c></row>",
    );
    parts.insert(
        "xl/workbook.xml".into(),
        parts["xl/workbook.xml"].replace(
            "</sheets>",
            "<sheet name=\"Other Sheet\" sheetId=\"2\" r:id=\"rId2\"/></sheets>",
        ),
    );
    parts.insert("xl/_rels/workbook.xml.rels".into(),parts["xl/_rels/workbook.xml.rels"].replace("</Relationships>","<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>"));
    parts.insert("xl/worksheets/sheet2.xml".into(),"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><f>Sheet1!B3</f><v>0</v></c></row></sheetData></worksheet>".into());
    let bytes = fixture::pack(&parts);
    for row in 1..=3 {
        let report = trace(&bytes, json!({"roots":[format!("Sheet1!B{row}")]}));
        assert_eq!(report.items[1]["cell"]["address"], format!("Sheet1!A{row}"));
    }
    let report = trace(&bytes, json!({"roots":["'Other Sheet'!A1"]}));
    assert_eq!(report.items[1]["cell"]["address"], "Sheet1!B3");
    let again = trace(&bytes, report.items[1]["next_trace_request"].clone());
    assert_eq!(again.items[1]["cell"]["address"], "Sheet1!A3");
}
#[test]
fn names_tables_external_and_calc_settings_have_honest_import_coverage() {
    let mut parts = fixture::parts(
        "<row r=\"1\"><c r=\"A1\"><v>21</v></c><c r=\"B1\"><f>Rate*2</f><v>0</v></c></row>",
    );
    parts.insert("xl/workbook.xml".into(),parts["xl/workbook.xml"].replace("</workbook>","<definedNames><definedName name=\"Rate\">Sheet1!$A$1</definedName><definedName name=\"Broken\">#REF!</definedName></definedNames><calcPr calcMode=\"manual\" iterate=\"0\" fullCalcOnLoad=\"1\"/></workbook>"));
    let bytes = fixture::pack(&parts);
    let named = trace(&bytes, json!({"roots":["Sheet1!B1"]}));
    assert_eq!(
        named.coverage["loaded_calc_settings"]["calc_mode"],
        "manual"
    );
    assert!(
        serde_json::to_string(&named.items)
            .unwrap()
            .contains("Rate")
    );
    for formula in ["MissingTable[Col]", "'[1]Remote'!A1"] {
        let bytes = fixture::pack(&fixture::parts(&format!(
            "<row r=\"1\"><c r=\"A1\"><f>{formula}</f><v>8</v></c></row>"
        )));
        let source = inspect(
            &bytes,
            json!({"selection":{"kind":"cells","addresses":["Sheet1!A1"]}}),
        );
        assert_eq!(
            source.items[0]["stored_formula_cache"]["freshness"],
            "unverified"
        );
        match trace_xlsx_bytes(
            &bytes,
            serde_json::from_value(json!({"roots":["Sheet1!A1"]})).unwrap(),
        ) {
            Ok(report) => {
                assert_ne!(report.coverage["transitive_graph_complete"], true);
                assert!(
                    report.coverage["external_refusal_and_table_import_diagnostics"]
                        .as_str()
                        .unwrap()
                        .contains("not exposed")
                );
                assert!(
                    serde_json::to_string(&report.items)
                        .unwrap()
                        .contains("unsupported")
                        || serde_json::to_string(&report.items)
                            .unwrap()
                            .contains("unresolved")
                );
            }
            Err(error) => {
                assert_eq!(error.code(), "INSPECTION_FAILED");
                assert!(!error.to_string().is_empty());
            }
        }
    }
}

#[test]
fn table_resolution_or_native_coverage_gap_is_exposed_without_an_alternate_graph() {
    let mut parts = fixture::parts(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Val</t></is></c><c r=\"B1\"><f>SUM(Data[Val])</f><v>0</v></c></row><row r=\"2\"><c r=\"A2\"><v>2</v></c></row><row r=\"3\"><c r=\"A3\"><v>3</v></c></row>",
    );
    parts.insert(fixture::SHEET.into(),parts[fixture::SHEET].replace("</worksheet>","<tableParts count=\"1\"><tablePart xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"rId1\"/></tableParts></worksheet>"));
    parts.insert("[Content_Types].xml".into(), parts["[Content_Types].xml"].replace("</Types>", "<Override PartName=\"/xl/tables/table1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml\"/></Types>"));
    parts.insert("xl/worksheets/_rels/sheet1.xml.rels".into(),"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/table\" Target=\"../tables/table1.xml\"/></Relationships>".into());
    parts.insert("xl/tables/table1.xml".into(),"<table xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" id=\"1\" name=\"Data\" displayName=\"Data\" ref=\"A1:A3\" headerRowCount=\"1\"><tableColumns count=\"1\"><tableColumn id=\"1\" name=\"Val\"/></tableColumns></table>".into());
    let report = trace(&fixture::pack(&parts), json!({"roots":["Sheet1!B1"]}));
    let reference = &report.items[0]["links"][0]["reference"];
    if reference["kind"] == "table" {
        assert_eq!(reference["name"], "Data");
        assert_eq!(reference["resolved"]["start_row"], 2);
    } else {
        assert_eq!(reference["kind"], "unsupported");
        assert!(
            reference["reason"]
                .as_str()
                .unwrap()
                .contains("structured reference")
        );
    }
}

#[test]
fn typed_numbers_mark_negative_zero_in_source_cache_and_engine_only_when_negative() {
    let bytes = fixture::pack(&fixture::parts(
        r#"<row r="1"><c r="A1"><v>-0.0</v></c><c r="B1"><f>A1</f><v>-0.0</v></c><c r="C1"><v>0.0</v></c><c r="D1"><f>C1</f><v>0.0</v></c></row>"#,
    ));
    let source = inspect(
        &bytes,
        json!({"selection":{"kind":"range","address":"Sheet1!A1:D1"}}),
    );
    assert_eq!(source.items[0]["source_value"]["negative_zero"], true);
    assert_eq!(
        source.items[1]["stored_formula_cache"]["value"]["negative_zero"],
        true
    );
    assert!(
        source.items[2]["source_value"]
            .get("negative_zero")
            .is_none()
    );
    assert!(
        source.items[3]["stored_formula_cache"]["value"]
            .get("negative_zero")
            .is_none()
    );
    let report = trace(&bytes, json!({"roots":["Sheet1!A1","Sheet1!C1"]}));
    assert_eq!(report.items[0]["cell"]["value"]["negative_zero"], true);
    assert!(
        report.items[1]["cell"]["value"]
            .get("negative_zero")
            .is_none()
    );
    assert!(
        report.items[0]["cell"]["value"]["value"]
            .as_f64()
            .unwrap()
            .is_sign_negative()
    );
    assert_eq!(source.items[0]["source_value"]["kind"], "number");
}

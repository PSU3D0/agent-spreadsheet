#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_spreadsheet::canonical_write::{WriteRequest, execute_write, execute_write_on_bytes};
use agent_spreadsheet::config::{OutputProfile, RecalcBackendKind, ServerConfig, TransportKind};
use agent_spreadsheet::model::WorkbookId;
use agent_spreadsheet::operations::ResourceId;
use agent_spreadsheet::state::AppState;
use agent_spreadsheet::tools::filters::WorkbookFilter;
use agent_spreadsheet::utils::hash_bytes_sha256_hex;
use serde_json::{Value, json};

fn config(path: &Path) -> Arc<ServerConfig> {
    Arc::new(ServerConfig {
        workspace_root: path.parent().unwrap().to_path_buf(),
        screenshot_dir: path.parent().unwrap().join("screenshots"),
        path_mappings: Vec::new(),
        cache_capacity: 2,
        supported_extensions: vec!["xlsx".to_string()],
        single_workbook: Some(path.to_path_buf()),
        enabled_tools: None,
        transport: TransportKind::Stdio,
        http_bind_address: "127.0.0.1:0".parse().unwrap(),
        recalc_enabled: true,
        recalc_backend: RecalcBackendKind::Formualizer,
        vba_enabled: false,
        max_concurrent_recalcs: 1,
        tool_timeout_ms: None,
        max_response_bytes: None,
        output_profile: OutputProfile::TokenDense,
        max_payload_bytes: None,
        max_cells: None,
        max_items: None,
        allow_overwrite: true,
        slim_surface: true,
    })
}

fn workbook_bytes() -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    book.new_sheet("Data").unwrap();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A1").set_value("Name");
    sheet.get_cell_mut("B1").set_value("Amount");
    sheet.get_cell_mut("A2").set_value("Ada");
    sheet.get_cell_mut("B2").set_value_number(10.0);
    sheet.get_cell_mut("C2").set_formula("SUM(B2:B2)");
    sheet.add_defined_name("Rate", "Sheet1!$B$2").unwrap();
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
    bytes
}

fn ops() -> Vec<Value> {
    vec![
        json!({"kind":"set_cells","sheet_name":"Sheet1","cells":{"A2":{"kind":"value","value":1}}}),
        json!({"kind":"clear_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"}}),
        json!({"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"value":"1"}),
        json!({"kind":"replace_in_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"find":"Name","replace":"Label"}),
        json!({"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":1}]]}),
        json!({"kind":"merge_cells","sheet_name":"Sheet1","target_range":"A1:B1"}),
        json!({"kind":"unmerge_cells","sheet_name":"Sheet1","target_range":"A1:B1"}),
        json!({"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}),
        json!({"kind":"delete_rows","sheet_name":"Sheet1","start_row":2,"count":1}),
        json!({"kind":"insert_cols","sheet_name":"Sheet1","at_col":"B","count":1}),
        json!({"kind":"delete_cols","sheet_name":"Sheet1","start_col":"B","count":1}),
        json!({"kind":"rename_sheet","old_name":"Sheet1","new_name":"Renamed"}),
        json!({"kind":"create_sheet","name":"Created"}),
        json!({"kind":"delete_sheet","name":"Data"}),
        json!({"kind":"copy_range","sheet_name":"Sheet1","src_range":"A1:A1","dest_anchor":"B3"}),
        json!({"kind":"move_range","sheet_name":"Sheet1","src_range":"A1:A1","dest_anchor":"B3"}),
        json!({"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"patch":{"font":{"bold":true}}}),
        json!({"kind":"column_size","sheet_name":"Sheet1","target":{"kind":"columns","range":"A:A"},"size":{"kind":"width","width_chars":12.0}}),
        json!({"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1}),
        json!({"kind":"set_zoom","sheet_name":"Sheet1","zoom_percent":100}),
        json!({"kind":"set_gridlines","sheet_name":"Sheet1","show":false}),
        json!({"kind":"set_page_margins","sheet_name":"Sheet1","left":1.0,"right":1.0,"top":1.0,"bottom":1.0}),
        json!({"kind":"set_page_setup","sheet_name":"Sheet1","orientation":"portrait"}),
        json!({"kind":"set_print_area","sheet_name":"Sheet1","range":"A1:B2"}),
        json!({"kind":"set_page_breaks","sheet_name":"Sheet1","row_breaks":[2],"col_breaks":[2]}),
        json!({"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"A1:A1","validation":{"kind":"list","formula1":"\"a,b\""}}),
        json!({"kind":"add_conditional_format","sheet_name":"Sheet1","target_range":"A1:A1","rule":{"kind":"expression","formula":"A1>0"}}),
        json!({"kind":"set_conditional_format","sheet_name":"Sheet1","target_range":"A1:A1","rule":{"kind":"expression","formula":"A1>0"}}),
        json!({"kind":"clear_conditional_formats","sheet_name":"Sheet1","target_range":"A1:A1"}),
        json!({"kind":"formula_pattern","sheet_name":"Sheet1","target_range":"C2:C3","anchor_cell":"C2","base_formula":"=B2"}),
        json!({"kind":"replace_in_formulas","sheet_name":"Sheet1","find":"SUM","replace":"AVERAGE"}),
        json!({"kind":"define_name","name":"OtherRate","refers_to":"Sheet1!$A$1","scope":"workbook"}),
        json!({"kind":"update_name","name":"Rate","refers_to":"Sheet1!$A$2"}),
        json!({"kind":"delete_name","name":"Rate"}),
        json!({"kind":"import_grid","sheet_name":"Sheet1","anchor":"D1","grid":{"sheet":"Sheet1","anchor":"A1","rows":[{"cells":[{"offset":[0,0],"v":"x"}]}]}}),
        json!({"kind":"import_csv","sheet_name":"Sheet1","anchor":"D1","csv":"a,b\n1,2\n"}),
        json!({"kind":"append_rows","sheet_name":"Sheet1","region_id":0,"rows":[[{"v":"x"}]]}),
        json!({"kind":"clone_row","sheet_name":"Sheet1","source_row":2,"insert_at":3}),
        json!({"kind":"clone_row_band","sheet_name":"Sheet1","source_rows":"1:2","insert_at":3}),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn all_39_write_kinds_match_native_bytes_and_result_details() {
    let initial = workbook_bytes();
    assert_eq!(ops().len(), 39);
    for op in ops() {
        let kind = op["kind"].as_str().unwrap().to_string();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.xlsx");
        std::fs::write(&source, &initial).unwrap();
        let state = Arc::new(AppState::new(config(&source)));
        let workbook = state
            .list_workbooks(WorkbookFilter::default())
            .unwrap()
            .workbooks[0]
            .workbook_id
            .clone();
        let fork = agent_spreadsheet::tools::fork::create_fork(
            state.clone(),
            agent_spreadsheet::tools::fork::CreateForkParams {
                workbook_or_fork_id: workbook,
            },
        )
        .await
        .unwrap();
        let fork_id = WorkbookId(fork.fork_id);
        let fork_path: PathBuf = state
            .fork_registry()
            .unwrap()
            .get_fork_path(fork_id.as_str())
            .unwrap();
        let native_revision = agent_spreadsheet::utils::hash_file_sha256_hex(&fork_path).unwrap();
        let native_request: WriteRequest = serde_json::from_value(json!({
            "resource_id": ResourceId::bind_workbook(&fork_id).unwrap(),
            "expected_revision": native_revision,
            "mode": "apply",
            "ops": [op.clone()]
        }))
        .unwrap();
        let native = execute_write(state, native_request)
            .await
            .unwrap_or_else(|error| panic!("native {kind} failed: {error}"));
        let native_bytes = std::fs::read(&fork_path).unwrap();

        let revision = hash_bytes_sha256_hex(&initial);
        let memory_request: WriteRequest = serde_json::from_value(json!({
            "resource_id": "session:test",
            "expected_revision": revision,
            "mode": "apply",
            "ops": [op]
        }))
        .unwrap();
        let (memory, memory_bytes) = execute_write_on_bytes(&initial, &revision, memory_request)
            .unwrap_or_else(|error| panic!("memory {kind} failed: {error}"));
        let memory_bytes = memory_bytes.expect("applied bytes");
        assert_eq!(native_bytes, memory_bytes, "byte mismatch for {kind}");
        let native_json = serde_json::to_value(native).unwrap();
        let memory_json = serde_json::to_value(memory).unwrap();
        assert_eq!(
            native_json["results"][0]["detail"], memory_json["results"][0]["detail"],
            "detail mismatch for {kind}"
        );
    }
}

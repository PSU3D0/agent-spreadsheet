use std::fs;
use std::sync::Arc;

use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::repository::{PathWorkspaceRepository, WorkbookRepository};
use agent_spreadsheet_mcp::tools::filters::WorkbookFilter;
use agent_spreadsheet_mcp::utils::hash_path_metadata;

mod support;

fn make_repo(config: Arc<agent_spreadsheet_mcp::config::ServerConfig>) -> PathWorkspaceRepository {
    #[cfg(feature = "recalc")]
    {
        PathWorkspaceRepository::new(config, None)
    }
    #[cfg(not(feature = "recalc"))]
    {
        PathWorkspaceRepository::new(config)
    }
}

#[test]
fn path_repo_stable_id_and_revision_behavior() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("finance/model.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_formula("A1*2");
    });

    let config = Arc::new(workspace.config());
    let repo = make_repo(config);

    let list1 = repo.list(&WorkbookFilter::default())?;
    assert_eq!(list1.workbooks.len(), 1);
    let first = &list1.workbooks[0];
    let stable_id_1 = first.workbook_id.clone();
    let revision_1 = first.revision_id.clone().expect("revision id");

    // Legacy path+metadata ID still resolves as compatibility alias.
    let legacy = hash_path_metadata(&path, &fs::metadata(&path)?);
    let resolved_legacy = repo.resolve(&WorkbookId(legacy))?;
    assert_eq!(resolved_legacy.workbook_id, stable_id_1);

    // Mutate workbook content in place.
    let mut book = umya_spreadsheet::reader::xlsx::read(&path)?;
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A1").set_value_number(33);
    umya_spreadsheet::writer::xlsx::write(&book, &path)?;

    let list2 = repo.list(&WorkbookFilter::default())?;
    assert_eq!(list2.workbooks.len(), 1);
    let second = &list2.workbooks[0];
    let stable_id_2 = second.workbook_id.clone();
    let revision_2 = second.revision_id.clone().expect("revision id");

    assert_eq!(stable_id_1, stable_id_2, "stable id should not churn");
    assert_ne!(revision_1, revision_2, "revision id should track content");

    // Short id should resolve to canonical stable id.
    let resolved_short = repo.resolve(&WorkbookId(first.short_id.clone()))?;
    assert_eq!(resolved_short.workbook_id, stable_id_1);
    Ok(())
}

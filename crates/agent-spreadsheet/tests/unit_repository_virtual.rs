use std::sync::Arc;

use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::repository::{
    VirtualWorkbookInput, VirtualWorkspaceRepository, WorkbookRepository,
};

mod support;

#[test]
fn virtual_repo_register_resolve_and_load() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("virtual_source.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("hello");
    });

    let bytes = std::fs::read(path)?;
    let config = Arc::new(workspace.config());
    let repo = VirtualWorkspaceRepository::new(config);

    let workbook_id = repo.register(VirtualWorkbookInput {
        key: "book-1.xlsx".to_string(),
        slug: Some("book-1".to_string()),
        bytes,
    });

    let by_key = repo.resolve(&WorkbookId("book-1.xlsx".to_string()))?;
    let by_short = repo.resolve(&WorkbookId(by_key.short_id.clone()))?;
    let by_id = repo.resolve(&workbook_id)?;

    assert_eq!(by_key.workbook_id, workbook_id);
    assert_eq!(by_short.workbook_id, workbook_id);
    assert_eq!(by_id.workbook_id, workbook_id);

    let list = repo.list(&agent_spreadsheet_mcp::tools::filters::WorkbookFilter::default())?;
    assert_eq!(list.workbooks.len(), 1);
    assert!(list.workbooks[0].revision_id.is_some());

    let ctx = repo.load_context(&by_id)?;
    assert_eq!(ctx.sheet_names(), vec!["Sheet1".to_string()]);
    Ok(())
}

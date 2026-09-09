#![allow(dead_code)]
pub mod builders;
pub mod docker;
pub mod mcp;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_spreadsheet_mcp::state::AppState;
use agent_spreadsheet_mcp::{OutputProfile, ServerConfig, SpreadsheetServer, TransportKind};
use anyhow::Result;
use tempfile::{TempDir, tempdir};
use umya_spreadsheet::{self, Workbook as Spreadsheet};

const DEFAULT_EXTENSIONS: &[&str] = &["xlsx", "xlsm", "xls", "xlsb"];

#[allow(dead_code)]
pub fn build_workbook<F>(f: F) -> PathBuf
where
    F: FnOnce(&mut Spreadsheet),
{
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("fixture.xlsx");
    write_workbook_to_path(&path, f);
    std::mem::forget(tmp);
    path
}

#[allow(dead_code)]
pub fn write_workbook_to_path<F>(path: &Path, f: F)
where
    F: FnOnce(&mut Spreadsheet),
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dir");
    }
    let mut book = umya_spreadsheet::new_file();
    f(&mut book);
    umya_spreadsheet::writer::xlsx::write(&book, path).expect("write workbook");
}

pub struct TestWorkspace {
    _tempdir: TempDir,
    root: PathBuf,
}

impl TestWorkspace {
    pub fn new() -> Self {
        let tempdir = tempdir().expect("tempdir");
        let root = tempdir.path().to_path_buf();
        Self {
            _tempdir: tempdir,
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn create_workbook<F>(&self, name: &str, f: F) -> PathBuf
    where
        F: FnOnce(&mut Spreadsheet),
    {
        let path = self.path(name);
        write_workbook_to_path(&path, f);
        path
    }

    pub fn copy_workbook(&self, src: &Path, name: &str) -> PathBuf {
        let dest = self.path(name);
        std::fs::copy(src, &dest).expect("copy workbook");
        dest
    }

    pub fn config(&self) -> ServerConfig {
        ServerConfig {
            workspace_root: self.root.clone(),
            screenshot_dir: self.root.join("screenshots"),
            path_mappings: Vec::new(),
            cache_capacity: 8,
            supported_extensions: DEFAULT_EXTENSIONS
                .iter()
                .map(|ext| ext.to_string())
                .collect(),
            single_workbook: None,
            enabled_tools: None,
            transport: TransportKind::Http,
            http_bind_address: "127.0.0.1:8079".parse().unwrap(),
            recalc_enabled: false,
            recalc_backend: agent_spreadsheet_mcp::config::RecalcBackendKind::Auto,
            vba_enabled: false,
            max_concurrent_recalcs: 2,
            tool_timeout_ms: Some(30_000),
            max_response_bytes: Some(1_000_000),
            output_profile: OutputProfile::TokenDense,
            max_payload_bytes: Some(65_536),
            max_cells: Some(10_000),
            max_items: Some(500),
            allow_overwrite: false,
            slim_surface: true,
        }
    }

    pub fn config_with<F>(&self, configure: F) -> ServerConfig
    where
        F: FnOnce(&mut ServerConfig),
    {
        let mut config = self.config();
        configure(&mut config);
        config
    }

    pub fn app_state(&self) -> Arc<AppState> {
        let config = Arc::new(self.config());
        Arc::new(AppState::new(config))
    }

    pub async fn server(&self) -> Result<SpreadsheetServer> {
        let config = Arc::new(self.config());
        SpreadsheetServer::new(config).await
    }
}

pub fn app_state_with_config(config: ServerConfig) -> Arc<AppState> {
    let config = Arc::new(config);
    Arc::new(AppState::new(config))
}

pub fn touch_file(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dir");
    }
    std::fs::write(path, b"test").expect("write file");
}

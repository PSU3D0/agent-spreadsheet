use super::RecalcConfig;
use super::executor::{RecalcExecutor, RecalcResult};
use crate::utils::hash_file_sha256_hex;
use crate::workbook::is_spreadsheet_error;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time;

use super::macro_uri::recalc_and_save_uri;

pub struct FireAndForgetExecutor {
    soffice_path: PathBuf,
    timeout: Duration,
}

impl FireAndForgetExecutor {
    pub fn new(config: &RecalcConfig) -> Self {
        Self {
            soffice_path: config
                .soffice_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("/usr/bin/soffice")),
            timeout: Duration::from_millis(config.timeout_ms.unwrap_or(30_000)),
        }
    }
}

#[async_trait]
impl RecalcExecutor for FireAndForgetExecutor {
    async fn recalculate(&self, workbook_path: &Path) -> Result<RecalcResult> {
        let start = Instant::now();

        // LibreOffice will try to write cache/config under HOME and XDG dirs.
        // In containerized environments we want a known-writable location.
        // This also prevents attempts to write to '/.cache' when HOME is unset.
        let _ = std::fs::create_dir_all("/tmp/.cache");
        let _ = std::fs::create_dir_all("/tmp/.config");

        let abs_path = workbook_path
            .canonicalize()
            .map_err(|e| anyhow!("failed to canonicalize path: {}", e))?;

        let file_path = abs_path.to_string_lossy().to_string();
        let macro_uri = recalc_and_save_uri(&file_path)?;

        let output_result = time::timeout(self.timeout, {
            let mut cmd = Command::new(&self.soffice_path);
            if let Ok(root) = std::env::var("SPREADSHEET_MCP_LIBREOFFICE_USER_INSTALLATION")
                && !root.trim().is_empty()
            {
                let root = root.trim();
                let uri = if root.starts_with("file://") {
                    root.to_string()
                } else {
                    format!("file:///{}", root.trim_start_matches('/'))
                };
                cmd.arg(format!("-env:UserInstallation={}", uri));
            }
            cmd.args([
                "--headless",
                "--norestore",
                "--nodefault",
                "--nofirststartwizard",
                "--nolockcheck",
                "--calc",
                &macro_uri,
            ])
            .env("HOME", "/tmp")
            .env("XDG_CACHE_HOME", "/tmp/.cache")
            .env("XDG_CONFIG_HOME", "/tmp/.config")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        })
        .await
        .map_err(|_| anyhow!("soffice timed out after {:?}", self.timeout))
        .and_then(|res| res.map_err(|e| anyhow!("failed to spawn soffice: {}", e)));

        let output = output_result?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "soffice failed (exit {}): stderr={}, stdout={}",
                output.status.code().unwrap_or(-1),
                stderr,
                stdout
            ));
        }

        // LibreOffice can sometimes exit 0 even when the macro failed.
        // Heuristic: treat obvious Basic/macro errors as failures so callers don't silently
        // proceed with stale / missing cached results.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr_trim = stderr.trim();
        let stdout_trim = stdout.trim();

        // Surface logs to help diagnose macro issues in Docker.
        // Keep it bounded to avoid blowing up logs.
        let truncate = |s: &str, max: usize| {
            if s.len() <= max {
                return s.to_string();
            }
            let mut end = max;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...[truncated]", &s[..end])
        };
        if !stderr_trim.is_empty() || !stdout_trim.is_empty() {
            tracing::warn!(
                soffice_stderr = %truncate(stderr_trim, 16 * 1024),
                soffice_stdout = %truncate(stdout_trim, 16 * 1024),
                "soffice recalc macro output"
            );
        }
        if !stderr_trim.is_empty() {
            let lower = stderr_trim.to_ascii_lowercase();
            if lower.contains("basic")
                || lower.contains("script could not be found")
                || lower.contains("macro")
                || lower.contains("uno exception")
            {
                return Err(anyhow!(
                    "soffice reported a macro error (exit 0): stderr={}, stdout={}",
                    stderr_trim,
                    stdout_trim
                ));
            }
        }

        let book = crate::xlsx_import::read(workbook_path)?;
        let mut formula_cells = 0u64;
        let mut error_formula_cells = 0u64;
        for sheet in book.get_sheet_collection() {
            for cell in sheet.cells() {
                if cell.is_formula() {
                    formula_cells += 1;
                    if is_spreadsheet_error(&cell.get_value()) {
                        error_formula_cells += 1;
                    }
                }
            }
        }

        Ok(RecalcResult {
            duration_ms: start.elapsed().as_millis() as u64,
            was_warm: false,
            backend_name: "libreoffice",
            cells_evaluated: None,
            eval_errors: None,
            evaluation_coverage: crate::model::EvaluationCoverage {
                formula_cells,
                evaluated_formula_cells: formula_cells,
                unsupported_formula_cells: 0,
                error_formula_cells,
                source: crate::model::EvaluationSource::TrustedCache,
                freshness: crate::model::EvaluationFreshness::CurrentRevision,
                revision_id: hash_file_sha256_hex(workbook_path)?,
            },
            incomplete: false,
        })
    }

    fn is_available(&self) -> bool {
        self.soffice_path.exists()
    }
}

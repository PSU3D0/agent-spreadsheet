#[cfg(feature = "cli")]
mod cli_args;
#[cfg(feature = "cli")]
pub use cli_args::CliArgs;

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

// Defaults only apply when a configuration is assembled from argv/config file,
// which is the `cli` surface. Byte/session hosts build `ServerConfig` directly.
#[cfg(feature = "cli")]
mod defaults {
    pub(super) const DEFAULT_CACHE_CAPACITY: usize = 5;
    pub(super) const DEFAULT_MAX_RECALCS: usize = 2;
    pub(super) const DEFAULT_EXTENSIONS: &[&str] = &["xlsx", "xlsm", "xls", "xlsb"];
    pub(super) const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8079";
    pub(super) const DEFAULT_TOOL_TIMEOUT_MS: u64 = 30_000;
    pub(super) const DEFAULT_MAX_RESPONSE_BYTES: u64 = 1_000_000;
    pub(super) const DEFAULT_MAX_PAYLOAD_BYTES: u64 = 65_536;
    pub(super) const DEFAULT_MAX_CELLS: u64 = 10_000;
    pub(super) const DEFAULT_MAX_ITEMS: u64 = 500;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    #[cfg_attr(feature = "cli", value(alias = "stream-http", alias = "stream_http"))]
    #[serde(alias = "stream-http", alias = "stream_http")]
    Http,
    Stdio,
}

impl std::fmt::Display for TransportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportKind::Http => write!(f, "http"),
            TransportKind::Stdio => write!(f, "stdio"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "snake_case")]
pub enum OutputProfile {
    #[default]
    TokenDense,
    Verbose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum RecalcBackendKind {
    Formualizer,
    Libreoffice,
    #[default]
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub workspace_root: PathBuf,
    /// Directory to write screenshot PNGs into (screenshot_sheet).
    pub screenshot_dir: PathBuf,
    /// Optional mapping from server-internal paths to client/host-visible paths.
    /// This is primarily useful when the server runs in Docker and volumes are mounted.
    pub path_mappings: Vec<PathMapping>,
    pub cache_capacity: usize,
    pub supported_extensions: Vec<String>,
    pub single_workbook: Option<PathBuf>,
    pub enabled_tools: Option<HashSet<String>>,
    pub transport: TransportKind,
    pub http_bind_address: SocketAddr,
    pub recalc_enabled: bool,
    pub recalc_backend: RecalcBackendKind,
    pub vba_enabled: bool,
    pub max_concurrent_recalcs: usize,
    pub tool_timeout_ms: Option<u64>,
    pub max_response_bytes: Option<u64>,
    pub output_profile: OutputProfile,
    pub max_payload_bytes: Option<u64>,
    pub max_cells: Option<u64>,
    pub max_items: Option<u64>,
    pub allow_overwrite: bool,
    /// When true (default), register only the consolidated write surface
    /// (mutate_batch + edit_batch) and hide the per-family batch tools.
    pub slim_surface: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathMapping {
    pub internal_prefix: PathBuf,
    pub client_prefix: PathBuf,
}

impl PathMapping {
    #[cfg(feature = "cli")]
    fn parse(spec: &str) -> Result<Self> {
        let (internal, client) = spec.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("invalid path mapping '{spec}' (expected INTERNAL=CLIENT)")
        })?;

        let internal_prefix = PathBuf::from(internal.trim());
        let client_prefix = PathBuf::from(client.trim());

        anyhow::ensure!(
            !internal_prefix.as_os_str().is_empty() && !client_prefix.as_os_str().is_empty(),
            "invalid path mapping '{spec}' (empty prefix)"
        );

        Ok(Self {
            internal_prefix,
            client_prefix,
        })
    }
}

impl ServerConfig {
    pub fn ensure_workspace_root(&self) -> Result<()> {
        anyhow::ensure!(
            self.workspace_root.exists(),
            "workspace root {:?} does not exist",
            self.workspace_root
        );
        anyhow::ensure!(
            self.workspace_root.is_dir(),
            "workspace root {:?} is not a directory",
            self.workspace_root
        );
        if let Some(workbook) = self.single_workbook.as_ref() {
            anyhow::ensure!(
                workbook.exists(),
                "configured workbook {:?} does not exist",
                workbook
            );
            anyhow::ensure!(
                workbook.is_file(),
                "configured workbook {:?} is not a file",
                workbook
            );
        }
        Ok(())
    }

    pub fn map_path_for_client<P: AsRef<Path>>(&self, internal_path: P) -> Option<PathBuf> {
        let internal_path = internal_path.as_ref();
        for m in &self.path_mappings {
            if internal_path.starts_with(&m.internal_prefix) {
                let suffix = internal_path.strip_prefix(&m.internal_prefix).ok()?;
                return Some(m.client_prefix.join(suffix));
            }
        }
        None
    }

    pub fn map_path_from_client<P: AsRef<Path>>(&self, client_path: P) -> Option<PathBuf> {
        let client_path = client_path.as_ref();
        for m in &self.path_mappings {
            if client_path.starts_with(&m.client_prefix) {
                let suffix = client_path.strip_prefix(&m.client_prefix).ok()?;
                return Some(m.internal_prefix.join(suffix));
            }
        }
        None
    }

    /// Resolve a user-supplied path for tools (e.g. save_fork target_path).
    /// - If the path is absolute and matches a configured client path mapping, map it to internal.
    /// - Otherwise, treat absolute paths as internal.
    /// - Relative paths are resolved under workspace_root.
    pub fn resolve_user_path<P: AsRef<Path>>(&self, p: P) -> PathBuf {
        let p = p.as_ref();
        if p.is_absolute() {
            self.map_path_from_client(p)
                .unwrap_or_else(|| p.to_path_buf())
        } else {
            self.workspace_root.join(p)
        }
    }

    pub fn resolve_path<P: AsRef<Path>>(&self, relative: P) -> PathBuf {
        let relative = relative.as_ref();
        if relative.is_absolute() {
            relative.to_path_buf()
        } else {
            self.workspace_root.join(relative)
        }
    }

    pub fn single_workbook(&self) -> Option<&Path> {
        self.single_workbook.as_deref()
    }

    pub fn is_tool_enabled(&self, tool: &str) -> bool {
        match &self.enabled_tools {
            Some(set) => set.contains(&tool.to_ascii_lowercase()),
            None => true,
        }
    }

    pub fn tool_timeout(&self) -> Option<Duration> {
        self.tool_timeout_ms.and_then(|ms| {
            if ms > 0 {
                Some(Duration::from_millis(ms))
            } else {
                None
            }
        })
    }

    pub fn max_response_bytes(&self) -> Option<usize> {
        self.max_response_bytes.and_then(|bytes| {
            if bytes > 0 {
                Some(bytes as usize)
            } else {
                None
            }
        })
    }

    pub fn output_profile(&self) -> OutputProfile {
        self.output_profile
    }

    pub fn max_payload_bytes(&self) -> Option<usize> {
        self.max_payload_bytes.map(|bytes| bytes as usize)
    }

    pub fn max_cells(&self) -> Option<usize> {
        self.max_cells.map(|cells| cells as usize)
    }

    pub fn max_items(&self) -> Option<usize> {
        self.max_items.map(|items| items as usize)
    }
}

//! Stateless experiment: reuse the production byte adapters without a session store.
//! The adapter receiver is a zero-sized compatibility shim, never StoredSession.
#[path = "../../agent-spreadsheet-wasm/src/source_xlsx.rs"]
mod source_xlsx;
#[path = "../../agent-spreadsheet-wasm/src/inspect_xlsx.rs"]
mod inspect_xlsx;
#[cfg(feature = "edit")]
#[path = "../../agent-spreadsheet-wasm/src/edit_xlsx.rs"]
mod edit_xlsx;
pub use source_xlsx::*;
pub use inspect_xlsx::*;
#[cfg(feature = "edit")]
pub use edit_xlsx::*;
#[cfg(target_arch = "wasm32")]
use serde::Serialize;
pub const MAX_WORKBOOK_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PARAMS_JSON_BYTES: usize = 1024 * 1024;
#[derive(Clone, Copy, Default)]
pub struct SessionApi;
impl SessionApi { pub fn new() -> Self { Self } }
#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct SessionApiErrorPayload { code: String, message: String }
#[cfg(target_arch = "wasm32")]
struct SessionApiError { message: String }
#[cfg(target_arch = "wasm32")]
impl SessionApiError {
    fn internal(message: impl Into<String>) -> Self { Self { message: message.into() } }
}
include!("bindings.rs");

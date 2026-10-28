use crate::{MAX_PARAMS_JSON_BYTES, MAX_WORKBOOK_BYTES, SessionApi};
pub use agent_spreadsheet::stateless_read::{ReadError, ReadReport};
use agent_spreadsheet::stateless_read::{inspect_xlsx_bytes, trace_xlsx_bytes};
fn validate(input: &[u8], request: &str) -> Result<(), ReadError> {
    if input.len() > MAX_WORKBOOK_BYTES || request.len() > MAX_PARAMS_JSON_BYTES {
        return Err(ReadError::Invalid(
            "workbook/request exceeds existing adapter input limit".into(),
        ));
    }
    Ok(())
}
impl SessionApi {
    pub fn inspect_xlsx(&self, input: &[u8], request: &str) -> Result<ReadReport, ReadError> {
        validate(input, request)?;
        inspect_xlsx_bytes(
            input,
            serde_json::from_str(request).map_err(|e| ReadError::Invalid(e.to_string()))?,
        )
    }
    pub fn trace_xlsx(&self, input: &[u8], request: &str) -> Result<ReadReport, ReadError> {
        validate(input, request)?;
        trace_xlsx_bytes(
            input,
            serde_json::from_str(request).map_err(|e| ReadError::Invalid(e.to_string()))?,
        )
    }
}

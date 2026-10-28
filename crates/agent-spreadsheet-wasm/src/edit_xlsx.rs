use crate::{
    MAX_PARAMS_JSON_BYTES, MAX_WORKBOOK_BYTES, SessionApi, SourceXlsxError, SourceXlsxMetadata,
    SourceXlsxOptions, SourceXlsxOutput,
};
use agent_spreadsheet::stateless_edit::{
    EditedArtifact, StatelessEditError, StatelessEditReceipt, StatelessEditRequest, edit_xlsx_bytes,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct EditXlsxMetadata {
    pub edit: StatelessEditReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recalculation: Option<SourceXlsxMetadata>,
}
#[derive(Debug)]
pub struct EditXlsxOutput {
    pub bytes: Vec<u8>,
    pub metadata: EditXlsxMetadata,
}
#[derive(Debug, thiserror::Error)]
pub enum EditXlsxError {
    #[error("invalid edit request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Source(#[from] SourceXlsxError),
    #[error(transparent)]
    Edit(#[from] StatelessEditError),
}
impl EditXlsxError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidRequest(_) => "INVALID_ARGUMENT",
            Self::Source(error) => error.code(),
            Self::Edit(StatelessEditError::InvalidRequest(_)) => "INVALID_ARGUMENT",
            Self::Edit(StatelessEditError::Operation { code, .. }) => code,
            Self::Edit(StatelessEditError::Recalculate(_)) => "RECALCULATION_FAILED",
        }
    }
}
fn parse_options(json: Option<&str>) -> Result<SourceXlsxOptions, EditXlsxError> {
    match json {
        None => Ok(SourceXlsxOptions::default()),
        Some(json) => {
            if json.len() > MAX_PARAMS_JSON_BYTES {
                return Err(EditXlsxError::InvalidRequest(
                    "recalculation options exceed adapter JSON limit".into(),
                ));
            }
            serde_json::from_str(json)
                .map_err(|error| EditXlsxError::InvalidRequest(error.to_string()))
        }
    }
}
impl SessionApi {
    /// Stateless atomic editing. Session IDs/revisions are never caller inputs.
    pub fn edit_xlsx(
        &self,
        input: &[u8],
        request_json: &str,
        recalc_options_json: Option<&str>,
    ) -> Result<EditXlsxOutput, EditXlsxError> {
        if input.len() > MAX_WORKBOOK_BYTES {
            return Err(SourceXlsxError::HostLimit { direction: "input" }.into());
        }
        if request_json.len() > MAX_PARAMS_JSON_BYTES {
            return Err(EditXlsxError::InvalidRequest(
                "edit request exceeds adapter JSON limit".into(),
            ));
        }
        let request: StatelessEditRequest = serde_json::from_str(request_json)
            .map_err(|error| EditXlsxError::InvalidRequest(error.to_string()))?;
        // Validate even when calculation is skipped; never silently ignore bad options.
        let options = parse_options(recalc_options_json)?.into_upstream()?;
        let output = edit_xlsx_bytes(input, request, options).map_err(|error| match error {
            StatelessEditError::Recalculate(error) => {
                EditXlsxError::Source(SourceXlsxError::Recalculate(error))
            }
            error => EditXlsxError::Edit(error),
        })?;
        let (bytes, mut recalculation) = match output.artifact {
            EditedArtifact::Document(bytes) => (bytes, None),
            EditedArtifact::Deferred {
                bytes,
                reason,
                context,
            } => {
                let result = SourceXlsxOutput::deferred(bytes, reason, context);
                (result.bytes, Some(result.metadata))
            }
            EditedArtifact::Recalculated(result) => {
                let result = SourceXlsxOutput::from(result);
                (result.bytes, Some(result.metadata))
            }
        };
        if let Some(metadata) = &mut recalculation {
            metadata.bytes_unchanged = bytes == input;
        }
        if bytes.len() > MAX_WORKBOOK_BYTES {
            return Err(SourceXlsxError::HostLimit {
                direction: "output",
            }
            .into());
        }
        Ok(EditXlsxOutput {
            bytes,
            metadata: EditXlsxMetadata {
                edit: output.receipt,
                recalculation,
            },
        })
    }
}

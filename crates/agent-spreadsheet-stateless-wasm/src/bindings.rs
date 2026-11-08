#[cfg(target_arch = "wasm32")]
pub mod wasm_bindings {
    use super::*;
    use wasm_bindgen::prelude::*;

    fn api() -> SessionApi {
        thread_local! { static API: SessionApi = SessionApi::new(); }
        API.with(Clone::clone)
    }

    fn to_js_error(err: SessionApiError) -> JsValue {
        to_js_value(&SessionApiErrorPayload { code: "INTERNAL".into(), message: err.message })
            .unwrap_or_else(|_| JsValue::from_str("serialization failed"))
    }
    fn to_js_value<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(value).map_err(|e| JsValue::from_str(&e.to_string()))
    }
    #[wasm_bindgen]
    pub fn version() -> String { "0.17.0-stateless-experiment".into() }
    #[wasm_bindgen]
    pub fn capabilities() -> String {
        serde_json::json!({
            "source_recalculate": true, "inspect_xlsx": true, "trace_xlsx": true,
            "edit_xlsx": cfg!(feature = "edit"), "sessions": false, "render": false
        }).to_string()
    }

    #[wasm_bindgen]
    pub struct ReadXlsxResult {
        metadata_json: String,
    }
    #[wasm_bindgen]
    impl ReadXlsxResult {
        #[wasm_bindgen(getter, js_name = metadataJson)]
        pub fn metadata_json(&self) -> String {
            self.metadata_json.clone()
        }
    }
    fn read_error(error: ReadError) -> JsValue {
        to_js_value(&SessionApiErrorPayload {
            code: error.code().into(),
            message: error.to_string(),
        })
        .unwrap_or_else(|_| JsValue::from_str(&error.to_string()))
    }
    fn read_xlsx(
        input: js_sys::Uint8Array,
        request: String,
        trace: bool,
    ) -> Result<ReadXlsxResult, JsValue> {
        if input.length() as usize > MAX_WORKBOOK_BYTES {
            return Err(read_error(ReadError::Invalid(
                "workbook exceeds adapter input limit".into(),
            )));
        }
        let bytes = input.to_vec();
        let result = if trace {
            api().trace_xlsx(&bytes, &request)
        } else {
            api().inspect_xlsx(&bytes, &request)
        }.map_err(read_error)?;
        Ok(ReadXlsxResult {
            metadata_json: serde_json::to_string(&result)
                .map_err(|e| read_error(ReadError::Native(e.to_string())))?,
        })
    }

    /// Source-package result: typed bytes and compact JSON diagnostics are
    /// separate. This value owns no session or resident evaluator.
    #[wasm_bindgen]
    pub struct SourceXlsxResult {
        bytes: Vec<u8>,
        metadata_json: String,
    }

    #[wasm_bindgen]
    impl SourceXlsxResult {
        #[wasm_bindgen(getter)]
        pub fn bytes(&self) -> Vec<u8> {
            self.bytes.clone()
        }

        #[wasm_bindgen(getter, js_name = metadataJson)]
        pub fn metadata_json(&self) -> String {
            self.metadata_json.clone()
        }
    }

    /// Synchronous original-source cache repair before createSession.
    #[wasm_bindgen(js_name = recalculateSourceXlsx)]
    pub fn recalculate_source_xlsx_js(
        workbook_bytes: js_sys::Uint8Array,
        options_json: Option<String>,
    ) -> Result<SourceXlsxResult, JsValue> {
        let source_error = |error: SourceXlsxError| {
            // Serialize a struct, not serde_json::Value: the latter's map
            // becomes a JavaScript Map and JSON.stringify would lose its fields.
            to_js_value(&SessionApiErrorPayload {
                code: error.code().to_owned(),
                message: error.to_string(),
            })
            .unwrap_or_else(|_| JsValue::from_str(&error.to_string()))
        };
        let byte_length = workbook_bytes.length() as usize;
        if byte_length > MAX_WORKBOOK_BYTES {
            return Err(source_error(SourceXlsxError::HostLimit {
                direction: "input",
            }));
        }
        let mut bytes = vec![0; byte_length];
        workbook_bytes.copy_to(&mut bytes);
        let output = api()
            .recalculate_source_xlsx(&bytes, options_json.as_deref())
            .map_err(source_error)?;
        let metadata_json = serde_json::to_string(&output.metadata)
            .map_err(|error| to_js_error(SessionApiError::internal(error.to_string())))?;
        Ok(SourceXlsxResult {
            bytes: output.bytes,
            metadata_json,
        })
    }

    /// Inspect source values and unverified formula caches without loading an engine.
    #[wasm_bindgen(js_name = inspectXlsx)]
    pub fn inspect_xlsx_js(input: js_sys::Uint8Array, request: String) -> Result<ReadXlsxResult, JsValue> {
        read_xlsx(input, request, false)
    }
    /// Read-only engine navigation; never evaluates formulas.
    #[wasm_bindgen(js_name = traceXlsx)]
    pub fn trace_xlsx_js(input: js_sys::Uint8Array, request: String) -> Result<ReadXlsxResult, JsValue> {
        read_xlsx(input, request, true)
    }

    /// Atomic document edits, optionally followed by strict cache recalculation.
    #[cfg(feature = "edit")]
    #[wasm_bindgen(js_name = editXlsx)]
    pub fn edit_xlsx_js(
        workbook_bytes: js_sys::Uint8Array,
        request_json: String,
        recalc_options_json: Option<String>,
    ) -> Result<SourceXlsxResult, JsValue> {
        let edit_error = |error: EditXlsxError| {
            to_js_value(&SessionApiErrorPayload {
                code: error.code().to_owned(),
                message: error.to_string(),
            })
            .unwrap_or_else(|_| JsValue::from_str(&error.to_string()))
        };
        if workbook_bytes.length() as usize > MAX_WORKBOOK_BYTES {
            return Err(edit_error(SourceXlsxError::HostLimit {
                direction: "input",
            }.into()));
        }
        let mut bytes = vec![0; workbook_bytes.length() as usize];
        workbook_bytes.copy_to(&mut bytes);
        let output = api()
            .edit_xlsx(&bytes, &request_json, recalc_options_json.as_deref())
            .map_err(edit_error)?;
        let metadata_json = serde_json::to_string(&output.metadata)
            .map_err(|error| to_js_error(SessionApiError::internal(error.to_string())))?;
        Ok(SourceXlsxResult {
            bytes: output.bytes,
            metadata_json,
        })
    }

}

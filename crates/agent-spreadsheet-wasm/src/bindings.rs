#[cfg(target_arch = "wasm32")]
pub mod wasm_bindings {
    use super::*;
    use wasm_bindgen::prelude::*;

    fn api() -> SessionApi {
        thread_local! { static API: SessionApi = SessionApi::new(); }
        API.with(Clone::clone)
    }

    fn to_js_error(err: SessionApiError) -> JsValue {
        let payload = SessionApiErrorPayload::from(err);
        serde_wasm_bindgen::to_value(&payload)
            .unwrap_or_else(|_| JsValue::from_str(&payload.message))
    }

    fn canonical_to_js_error(err: CanonicalErrorEnvelope) -> JsValue {
        serde_wasm_bindgen::to_value(&err).unwrap_or_else(|_| JsValue::from_str(&err.error.message))
    }

    fn to_js_value<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(value).map_err(|err| {
            to_js_error(SessionApiError::Internal {
                message: format!("failed to serialize response: {err}"),
            })
        })
    }

    fn from_js_value<T: for<'de> Deserialize<'de>>(value: JsValue) -> SessionResult<T> {
        serde_wasm_bindgen::from_value(value).map_err(|err| SessionApiError::InvalidArgument {
            message: format!("invalid params: {err}"),
        })
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

    #[wasm_bindgen(js_name = createSession)]
    pub fn create_session_js(workbook_bytes: js_sys::Uint8Array) -> Result<String, JsValue> {
        let byte_length = workbook_bytes.length() as usize;
        if byte_length > MAX_WORKBOOK_BYTES {
            return Err(to_js_error(SessionApiError::InvalidArgument {
                message: format!("workbook exceeds the {MAX_WORKBOOK_BYTES}-byte session limit"),
            }));
        }
        let mut bytes = vec![0; byte_length];
        workbook_bytes.copy_to(&mut bytes);
        api().create_session(&bytes).map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = sessionMetadata)]
    pub fn session_metadata_js(session_id: String) -> Result<String, JsValue> {
        let metadata = api().session_metadata(&session_id).map_err(to_js_error)?;
        serde_json::to_string(&metadata)
            .map_err(|error| to_js_error(SessionApiError::internal(error.to_string())))
    }

    #[wasm_bindgen(js_name = operations)]
    pub fn operations_js() -> Result<String, JsValue> {
        api().operations_json().map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = executeOperation)]
    pub async fn execute_operation_js(
        session_id: String,
        operation_name: String,
        params_json: String,
        request_id: Option<String>,
    ) -> Result<String, JsValue> {
        let api = api();
        match request_id {
            Some(id) => {
                api.execute_operation_with_request_id(
                    &session_id,
                    &operation_name,
                    &params_json,
                    &id,
                )
                .await
            }
            None => {
                api.execute_operation(&session_id, &operation_name, &params_json)
                    .await
            }
        }
        .map_err(canonical_to_js_error)
    }

    #[wasm_bindgen(js_name = listSheets)]
    pub fn list_sheets_js(session_id: String) -> Result<JsValue, JsValue> {
        let sheets = api().list_sheets(&session_id).map_err(to_js_error)?;
        to_js_value(&sheets)
    }

    #[wasm_bindgen(js_name = describeWorkbook)]
    pub fn describe_workbook_js(session_id: String) -> Result<JsValue, JsValue> {
        let result = api().describe_workbook(&session_id).map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = namedRanges)]
    pub fn named_ranges_js(session_id: String) -> Result<JsValue, JsValue> {
        let result = api().named_ranges(&session_id).map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = defineName)]
    pub fn define_name_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct DefineNameJsParams {
            name: String,
            refers_to: String,
            scope: Option<String>,
            scope_sheet_name: Option<String>,
        }
        let p: DefineNameJsParams = from_js_value(params).map_err(to_js_error)?;
        let result = api()
            .define_name(
                &session_id,
                &p.name,
                &p.refers_to,
                p.scope.as_deref(),
                p.scope_sheet_name.as_deref(),
            )
            .map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = updateName)]
    pub fn update_name_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct UpdateNameJsParams {
            name: String,
            refers_to: Option<String>,
            scope: Option<String>,
            scope_sheet_name: Option<String>,
        }
        let p: UpdateNameJsParams = from_js_value(params).map_err(to_js_error)?;
        let result = api()
            .update_name(
                &session_id,
                &p.name,
                p.refers_to.as_deref(),
                p.scope.as_deref(),
                p.scope_sheet_name.as_deref(),
            )
            .map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = deleteName)]
    pub fn delete_name_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct DeleteNameJsParams {
            name: String,
            scope: Option<String>,
            scope_sheet_name: Option<String>,
        }
        let p: DeleteNameJsParams = from_js_value(params).map_err(to_js_error)?;
        let result = api()
            .delete_name(
                &session_id,
                &p.name,
                p.scope.as_deref(),
                p.scope_sheet_name.as_deref(),
            )
            .map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = sheetOverview)]
    pub fn sheet_overview_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        let params: SheetOverviewParams = from_js_value(params).map_err(to_js_error)?;
        let result = api()
            .sheet_overview(&session_id, params)
            .map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = findValue)]
    pub fn find_value_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        let params: FindValueParams = from_js_value(params).map_err(to_js_error)?;
        let result = api().find_value(&session_id, params).map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = readTable)]
    pub fn read_table_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        let params: ReadTableParams = from_js_value(params).map_err(to_js_error)?;
        let result = api().read_table(&session_id, params).map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = rangeValues)]
    pub fn range_values_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        let params: RangeValuesParams = from_js_value(params).map_err(to_js_error)?;
        let result = api()
            .range_values(&session_id, params)
            .map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = sheetPage)]
    pub fn sheet_page_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        let params: SheetPageParams = from_js_value(params).map_err(to_js_error)?;
        let result = api().sheet_page(&session_id, params).map_err(to_js_error)?;
        to_js_value(&result)
    }

    #[wasm_bindgen(js_name = gridExport)]
    pub fn grid_export_js(session_id: String, params: JsValue) -> Result<JsValue, JsValue> {
        let params: GridExportParams = from_js_value(params).map_err(to_js_error)?;
        let payload = api()
            .grid_export(&session_id, params)
            .map_err(to_js_error)?;
        to_js_value(&payload)
    }

    #[wasm_bindgen(js_name = transformBatch)]
    pub fn transform_batch_js(
        session_id: String,
        ops: JsValue,
        options: Option<JsValue>,
    ) -> Result<JsValue, JsValue> {
        let ops: Vec<SessionTransformOp> = from_js_value(ops).map_err(to_js_error)?;
        let options = match options {
            Some(value) => from_js_value(value).map_err(to_js_error)?,
            None => TransformBatchOptions::default(),
        };

        let summary = api()
            .transform_batch(&session_id, ops, options)
            .map_err(to_js_error)?;
        to_js_value(&summary)
    }

    #[wasm_bindgen(js_name = exportWorkbook)]
    pub fn export_workbook_js(session_id: String) -> Result<Vec<u8>, JsValue> {
        api().export_workbook(&session_id).map_err(to_js_error)
    }

    /// Artifact bytes for a handle produced in this session. Rejects with the
    /// canonical error envelope, exactly like `executeOperation`.
    #[wasm_bindgen(js_name = readArtifact)]
    pub fn read_artifact_js(session_id: String, handle: String) -> Result<Vec<u8>, JsValue> {
        api()
            .read_artifact(&session_id, &handle)
            .map_err(canonical_to_js_error)
    }

    /// Release one artifact slot. `false` when the handle was already gone.
    #[wasm_bindgen(js_name = disposeArtifact)]
    pub fn dispose_artifact_js(session_id: String, handle: String) -> Result<bool, JsValue> {
        api()
            .dispose_artifact(&session_id, &handle)
            .map_err(canonical_to_js_error)
    }

    #[wasm_bindgen(js_name = disposeSession)]
    pub fn dispose_session_js(session_id: String) -> Result<bool, JsValue> {
        api().dispose_session(&session_id).map_err(to_js_error)
    }
}

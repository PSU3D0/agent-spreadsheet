//! Shared cold projection; resident documents and evaluators are not reimported.
pub(crate) use formualizer_workbook::backends::umya3::{
    write_document_path as write, write_document_writer as write_writer,
};

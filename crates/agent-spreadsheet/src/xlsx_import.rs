//! Every cold document import uses the same Formualizer/Umya compatibility reader.
pub(crate) use formualizer_workbook::backends::umya3::read_document_path as read;
use std::io::Read;

pub(crate) fn read_reader(
    reader: impl Read,
    eager: bool,
) -> Result<umya_spreadsheet::Workbook, umya_spreadsheet::XlsxError> {
    if !eager {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "document imports must be eager",
        )
        .into());
    }
    formualizer_workbook::backends::umya3::read_document_reader(reader)
}

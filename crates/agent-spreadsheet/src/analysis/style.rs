use umya_spreadsheet::Cell;

#[derive(Debug, Clone)]
pub struct StyleTagging {
    pub tags: Vec<String>,
    pub example_cell: String,
}

pub fn tag_cell(cell: &Cell) -> Option<(String, StyleTagging)> {
    let style = cell.get_style();
    let mut tags = Vec::new();

    if let Some(font) = style.get_font() {
        if font.get_bold() {
            tags.push("header".to_string());
        }
        if font.get_italic() {
            tags.push("emphasis".to_string());
        }
    }

    if let Some(number_format) = style.get_number_format() {
        let format_code = number_format.get_format_code().to_ascii_lowercase();
        if format_code.contains("$") {
            tags.push("currency".to_string());
        } else if format_code.contains("%") {
            tags.push("percentage".to_string());
        } else if format_code.contains("yy") {
            tags.push("date".to_string());
        }
    }

    if tags.is_empty() {
        return None;
    }

    let coordinate = cell.get_coordinate();
    let address = coordinate.get_coordinate();
    let key = tags.join("|");

    Some((
        key,
        StyleTagging {
            tags,
            example_cell: address,
        },
    ))
}

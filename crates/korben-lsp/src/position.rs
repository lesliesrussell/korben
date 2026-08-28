//! Converting between Korben's byte offsets and the protocol's positions.
//!
//! A protocol position is a zero-based line and a zero-based character, and
//! that character is counted in UTF-16 code units -- not bytes, and not
//! characters. Korben counts bytes. Every conversion between the two lives
//! here, so an editor and the compiler cannot drift apart on where something is.

// korben-efd

/// A zero-based line and UTF-16 character offset, as the protocol counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// The position of a byte offset within `text`.
pub fn to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let line = text[..line_start].matches('\n').count();
    let character = text[line_start..offset].encode_utf16().count();
    Position { line: line as u32, character: character as u32 }
}

/// The byte offset of a position within `text`.
///
/// A position past the end of its line clamps to the line's end, and a line
/// past the end of the text clamps to the text's end: an editor may ask about a
/// position that its buffer has but the server's copy does not yet.
pub fn to_offset(text: &str, position: Position) -> usize {
    let mut line_start = 0;
    for _ in 0..position.line {
        match text[line_start..].find('\n') {
            Some(index) => line_start += index + 1,
            None => return text.len(),
        }
    }
    let line_end =
        text[line_start..].find('\n').map(|index| line_start + index).unwrap_or(text.len());
    let line = &text[line_start..line_end];
    let mut units = 0u32;
    for (index, character) in line.char_indices() {
        if units >= position.character {
            return line_start + index;
        }
        units += character.len_utf16() as u32;
    }
    line_end
}

/// The identifier surrounding `offset`, as a byte range.
///
/// Korben symbols are generous -- `split-once`, `empty?`, `set!`, `std.string`,
/// `Cell.new` all name one thing -- so the run is delimited by the characters
/// that cannot appear in a name rather than by an alphanumeric rule.
pub fn word_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let offset = offset.min(text.len());
    let is_name =
        |character: char| !character.is_whitespace() && !"()[]{}\"';`,~@\\".contains(character);
    // A cursor sitting just past a name still means that name, which is where
    // an editor puts it when you finish typing.
    let start_from = if offset == text.len() || !text[offset..].starts_with(is_name) {
        text[..offset].char_indices().next_back().map(|(index, _)| index)?
    } else {
        offset
    };
    if !text[start_from..].starts_with(is_name) {
        return None;
    }
    let mut start = start_from;
    for (index, character) in text[..start_from].char_indices().rev() {
        if !is_name(character) {
            break;
        }
        start = index;
    }
    let mut end = start_from;
    for (index, character) in text[start_from..].char_indices() {
        if !is_name(character) {
            break;
        }
        end = start_from + index + character.len_utf8();
    }
    (start < end).then_some((start, end))
}

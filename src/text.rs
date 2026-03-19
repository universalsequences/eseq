pub enum Direction {
    Forward,
    Backward,
}

pub struct SExpParser<'a> {
    lines: &'a [String],
    cursor: (usize, usize),
    reached_start: bool,
    reached_end: bool,
    direction: Direction,
}

impl<'a> SExpParser<'a> {
    pub fn new(lines: &'a [String], cursor: (usize, usize)) -> Self {
        SExpParser {
            lines,
            cursor,
            reached_start: false,
            reached_end: false,
            direction: Direction::Backward,
        }
    }

    pub fn set_direction(&mut self, direction: Direction) {
        self.direction = direction;
    }

    pub fn position(&self) -> (usize, usize) {
        self.cursor
    }

    pub fn peek(&self) -> Option<char> {
        if self.reached_start || self.reached_end {
            return None;
        }
        if let Some(line) = self.lines.get(self.cursor.0) {
            return line.chars().nth(self.cursor.1);
        }
        None
    }

    pub fn next(&mut self) -> Option<char> {
        if self.reached_start || self.reached_end {
            return None;
        }
        if let Some(line) = self.lines.get(self.cursor.0) {
            let next = line.chars().nth(self.cursor.1);
            match self.direction {
                Direction::Backward => {
                    if self.cursor.1 > 0 {
                        self.cursor.1 = self.cursor.1.saturating_sub(1);
                    } else if self.cursor.0 == 0 {
                        self.reached_start = true;
                    } else {
                        self.cursor.0 = self.cursor.0.saturating_sub(1);
                        if let Some(prev_line) = self.lines.get(self.cursor.0) {
                            if prev_line.is_empty() {
                                self.cursor.1 = 0;
                            } else {
                                self.cursor.1 = prev_line.len() - 1;
                            }
                        }
                    }
                }
                Direction::Forward => {
                    if line.is_empty() || self.cursor.1 == line.len() - 1 {
                        // next line
                        if self.cursor.0 < self.lines.len() - 1 {
                            self.cursor.0 = self.cursor.0.saturating_add(1);
                            self.cursor.1 = 0;
                        } else {
                            self.reached_end = true;
                        }
                    } else {
                        // move cursor right
                        self.cursor.1 = self.cursor.1.saturating_add(1);
                    }
                }
            }
            return next;
        }
        None
    }
}

/// Find the matching paren for the paren under the cursor.
///
/// Returns the (row, col) of the matching paren, or None if the cursor is
/// not on a paren or no match exists.
pub fn matching_paren(lines: &[String], cursor: (usize, usize)) -> Option<(usize, usize)> {
    let mut cursor = cursor;
    let line = lines.get(cursor.0)?;
    if line.is_empty() {
        return None;
    }
    if cursor.1 >= line.len() {
        cursor.1 = line.len() - 1;
    }

    let chars: Vec<Vec<char>> = lines.iter().map(|line| line.chars().collect()).collect();
    let start = *chars.get(cursor.0)?.get(cursor.1)?;

    match start {
        '(' => {
            let mut depth = 0usize;
            for (row, line_chars) in chars.iter().enumerate().skip(cursor.0) {
                let start_col = if row == cursor.0 { cursor.1 } else { 0 };
                for (col, ch) in line_chars.iter().enumerate().skip(start_col) {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                return Some((row, col));
                            }
                        }
                        _ => {}
                    }
                }
            }
            None
        }
        ')' => {
            let mut depth = 0usize;
            for row in (0..=cursor.0).rev() {
                let line_chars = &chars[row];
                if line_chars.is_empty() {
                    continue;
                }
                let end_col = if row == cursor.0 {
                    cursor.1
                } else {
                    line_chars.len().saturating_sub(1)
                };
                for col in (0..=end_col).rev() {
                    match line_chars[col] {
                        ')' => depth += 1,
                        '(' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                return Some((row, col));
                            }
                        }
                        _ => {}
                    }
                }
            }
            None
        }
        _ => None,
    }
}

pub fn follow_parens(parser: &mut SExpParser) -> String {
    let mut p = 0;
    let mut s = "".to_string();
    while let Some(ch) = parser.peek() {
        match ch {
            ')' => match parser.direction {
                Direction::Forward => {
                    p -= 1;
                    if p <= 0 {
                        s.push(ch);
                        break;
                    }
                }
                Direction::Backward => {
                    p += 1;
                }
            },
            '(' => match parser.direction {
                Direction::Backward => {
                    p -= 1;
                    if p <= 0 {
                        s.push(ch);
                        break;
                    }
                }
                Direction::Forward => {
                    p += 1;
                }
            },
            _ => {}
        }
        s.push(ch);
        parser.next();
    }
    s
}

/// Find the outermost s-expression at or enclosing the cursor.
///
/// Returns the text of the sexp as a String, or None if the cursor
/// is not inside any parenthesized expression.
///
///
/// Hints:
///   - Flatten `lines` into one string (joining with '\n'), then find the
///     cursor's offset in that flat string.
///   - Scan backwards from the cursor to find the opening '(', tracking
///     nesting depth so you find the *outermost* enclosing paren.
///   - From that '(', scan forwards to the matching ')'.
///   - Return the substring from '(' to ')' inclusive.
pub fn sexp_at_cursor(lines: &[String], cursor: (usize, usize)) -> Option<String> {
    let (start, end) = sexp_range_at_cursor(lines, cursor)?;

    let mut flat = String::new();
    let mut line_starts = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        line_starts.push(flat.len());
        flat.push_str(line);
        if idx + 1 < lines.len() {
            flat.push('\n');
        }
    }

    let start_idx = line_starts[start.0] + start.1;
    let end_idx = line_starts[end.0] + end.1;
    Some(flat[start_idx..=end_idx].replace('\n', ""))
}

pub fn sexp_range_at_cursor(
    lines: &[String],
    cursor: (usize, usize),
) -> Option<((usize, usize), (usize, usize))> {
    sexp_range_at_cursor_with_selector(lines, cursor, RangeSelector::Outermost)
}

pub fn innermost_sexp_range_at_cursor(
    lines: &[String],
    cursor: (usize, usize),
) -> Option<((usize, usize), (usize, usize))> {
    sexp_range_at_cursor_with_selector(lines, cursor, RangeSelector::Innermost)
}

enum RangeSelector {
    Outermost,
    Innermost,
}

fn sexp_range_at_cursor_with_selector(
    lines: &[String],
    cursor: (usize, usize),
    selector: RangeSelector,
) -> Option<((usize, usize), (usize, usize))> {
    let line = lines.get(cursor.0)?;
    let cursor_col = cursor.1.min(line.len());

    let mut flat = String::new();
    let mut line_starts = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        line_starts.push(flat.len());
        flat.push_str(line);
        if idx + 1 < lines.len() {
            flat.push('\n');
        }
    }

    if flat.is_empty() {
        return None;
    }

    let bytes = flat.as_bytes();
    let cursor_idx = (line_starts[cursor.0] + cursor_col).min(flat.len());

    let mut stack: Vec<usize> = Vec::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (idx, byte) in bytes.iter().enumerate() {
        match *byte {
            b'(' => stack.push(idx),
            b')' => {
                if let Some(open) = stack.pop() {
                    pairs.push((open, idx));
                }
            }
            _ => {}
        }
    }

    let pair = if cursor_idx < bytes.len() && bytes[cursor_idx] == b'(' {
        pairs.iter().find(|(open, _)| *open == cursor_idx).copied()
    } else if cursor_idx > 0 && bytes[cursor_idx - 1] == b')' {
        pairs
            .iter()
            .find(|(_, close)| *close == cursor_idx - 1)
            .copied()
    } else {
        let enclosing = pairs
            .iter()
            .filter(|(open, close)| *open < cursor_idx && cursor_idx <= *close);
        match selector {
            RangeSelector::Outermost => enclosing.min_by_key(|(open, _)| *open).copied(),
            RangeSelector::Innermost => enclosing.max_by_key(|(open, _)| *open).copied(),
        }
        .or_else(|| {
            pairs
                .iter()
                .filter(|(_, close)| *close < cursor_idx)
                .max_by_key(|(_, close)| *close)
                .copied()
        })
    }?;

    Some((
        flat_index_to_cursor(&line_starts, pair.0),
        flat_index_to_cursor(&line_starts, pair.1),
    ))
}

fn flat_index_to_cursor(line_starts: &[usize], idx: usize) -> (usize, usize) {
    let row = line_starts
        .partition_point(|start| *start <= idx)
        .saturating_sub(1);
    (row, idx.saturating_sub(line_starts[row]))
}

#[cfg(test)]
mod tests {
    use super::{
        innermost_sexp_range_at_cursor, matching_paren, sexp_at_cursor, sexp_range_at_cursor,
    };

    #[test]
    fn test_sexp_at_cursor() {
        let lines = [
            "alec".to_string(),
            "hello".to_string(),
            "(+ a ( a ) ".to_string(),
            " a ) a".to_string(),
        ];
        let cursor = (3, 3);
        let s = sexp_at_cursor(&lines, cursor);
        assert_eq!(s.unwrap(), "(+ a ( a )  a )", "s exp match");
    }

    #[test]
    fn test_sexp_before_cursor() {
        let lines = ["(+ 5 4)".to_string(), "hello".to_string()];
        let cursor = (1, 5);
        let s = sexp_at_cursor(&lines, cursor);
        assert_eq!(s.unwrap(), "(+ 5 4)", "sexp match");
    }

    #[test]
    fn test_sexp_at_end_of_form() {
        let lines = ["(seq-toggle-step 1)".to_string()];
        let cursor = (0, lines[0].len());
        let s = sexp_at_cursor(&lines, cursor);
        assert_eq!(s.unwrap(), "(seq-toggle-step 1)");
    }

    #[test]
    fn test_sexp_at_end_of_form_returns_none_when_no_form_exists() {
        let lines = ["hello".to_string()];
        let cursor = (0, lines[0].len());
        assert_eq!(sexp_at_cursor(&lines, cursor), None);
    }

    #[test]
    fn test_sexp_on_opening_paren_prefers_current_form() {
        let lines = ["(older)".to_string(), "(+ 5 5)".to_string()];
        let cursor = (1, 0);
        let s = sexp_at_cursor(&lines, cursor);
        assert_eq!(s.unwrap(), "(+ 5 5)");
    }

    #[test]
    fn matching_paren_returns_none_for_unmatched_open() {
        let lines = ["(".to_string(), "".to_string()];
        assert_eq!(matching_paren(&lines, (0, 1)), None);
    }

    #[test]
    fn matching_paren_finds_real_match_for_open() {
        let lines = ["(a".to_string(), ")".to_string()];
        assert_eq!(matching_paren(&lines, (0, 0)), Some((1, 0)));
    }

    #[test]
    fn sexp_range_returns_enclosing_form_bounds() {
        let lines = ["(if (< (rand-int 8) 4) :4t :32))".to_string()];
        let range = sexp_range_at_cursor(&lines, (0, 21)).unwrap();
        assert_eq!(range, ((0, 0), (0, 30)));
    }

    #[test]
    fn innermost_sexp_range_returns_current_nested_form_bounds() {
        let lines = ["(if (< (rand-int 8) 4) :4t :32))".to_string()];
        let range = innermost_sexp_range_at_cursor(&lines, (0, 21)).unwrap();
        assert_eq!(range, ((0, 4), (0, 21)));
    }
}

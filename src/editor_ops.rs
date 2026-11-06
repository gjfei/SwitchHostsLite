//! Line-oriented editing commands on plain text, using UTF-8 byte offsets.

use std::ops::Range;

/// A single replacement plus the selection to set after applying it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Byte range in the original text to replace.
    pub range: Range<usize>,
    pub new_text: String,
    /// Byte selection in the edited text.
    pub selection: Range<usize>,
}

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// End of the line containing `pos`, excluding its newline.
fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |i| pos + i)
}

/// Order and clamp `selection` to the text.
fn clamp(text: &str, selection: &Range<usize>) -> Range<usize> {
    let end = selection.start.max(selection.end).min(text.len());
    selection.start.min(selection.end).min(end)..end
}

/// The lines touched by `selection` as `start..end`, `end` excluding the
/// trailing newline. A non-empty selection ending at column 0 does not touch
/// that last line.
fn touched_block(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = line_start(text, selection.start);
    let mut last = selection.end;
    if selection.end > selection.start && line_start(text, selection.end) == selection.end {
        last = selection.end - 1;
    }
    start..line_end(text, last)
}

fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn shift(selection: &Range<usize>, delta: isize) -> Range<usize> {
    let move_by = |pos: usize| pos.saturating_add_signed(delta);
    move_by(selection.start)..move_by(selection.end)
}

/// Comment the touched lines with `# `, or uncomment them when every
/// non-blank line already starts with `#`.
pub fn toggle_comment(text: &str, selection: Range<usize>) -> Option<Edit> {
    let selection = clamp(text, &selection);
    let block = touched_block(text, &selection);
    let lines: Vec<&str> = text[block.clone()].split('\n').collect();
    let non_blank: Vec<&str> = lines.iter().copied().filter(|l| !is_blank(l)).collect();
    let uncomment = !non_blank.is_empty()
        && non_blank
            .iter()
            .all(|l| l[indent_len(l)..].starts_with('#'));
    let comment_col = non_blank.iter().map(|l| indent_len(l)).min().unwrap_or(0);

    // Per line: (column, removed bytes, inserted text).
    let mut changes = Vec::with_capacity(lines.len());
    let mut new_lines = Vec::with_capacity(lines.len());
    for line in &lines {
        let change = if uncomment {
            if is_blank(line) {
                None
            } else {
                let col = indent_len(line);
                let removed = if line[col + 1..].starts_with(' ') { 2 } else { 1 };
                Some((col, removed, ""))
            }
        } else if non_blank.is_empty() || !is_blank(line) {
            Some((comment_col, 0, "# "))
        } else {
            None
        };
        new_lines.push(match change {
            Some((col, removed, insert)) => {
                format!("{}{insert}{}", &line[..col], &line[col + removed..])
            }
            None => line.to_string(),
        });
        changes.push(change);
    }
    let new_text = new_lines.join("\n");

    let new_selection = if selection.is_empty() {
        let cursor = selection.start - block.start;
        let cursor = match changes[0] {
            Some((col, removed, insert)) if cursor > col => {
                (cursor.max(col + removed) - removed + insert.len()).min(new_text.len())
            }
            Some((col, _, insert)) if cursor == col => col + insert.len(),
            _ => cursor,
        };
        let pos = block.start + cursor;
        pos..pos
    } else {
        block.start..block.start + new_text.len()
    };

    Some(Edit {
        range: block,
        new_text,
        selection: new_selection,
    })
}

/// Swap the touched lines with the line above.
pub fn move_line_up(text: &str, selection: Range<usize>) -> Option<Edit> {
    let selection = clamp(text, &selection);
    let block = touched_block(text, &selection);
    if block.start == 0 {
        return None;
    }
    let prev_start = line_start(text, block.start - 1);
    let prev = &text[prev_start..block.start - 1];
    Some(Edit {
        range: prev_start..block.end,
        new_text: format!("{}\n{prev}", &text[block.clone()]),
        selection: shift(&selection, -(prev.len() as isize + 1)),
    })
}

/// Swap the touched lines with the line below. The empty line after a final
/// newline does not count as a line to swap with.
pub fn move_line_down(text: &str, selection: Range<usize>) -> Option<Edit> {
    let selection = clamp(text, &selection);
    let block = touched_block(text, &selection);
    if block.end + 1 >= text.len() {
        return None;
    }
    let next_start = block.end + 1;
    let next_end = line_end(text, next_start);
    let next = &text[next_start..next_end];
    Some(Edit {
        range: block.start..next_end,
        new_text: format!("{next}\n{}", &text[block.clone()]),
        selection: shift(&selection, next.len() as isize + 1),
    })
}

/// Insert a copy of the touched lines below them and select the copy.
pub fn duplicate_line(text: &str, selection: Range<usize>) -> Option<Edit> {
    let selection = clamp(text, &selection);
    let block = touched_block(text, &selection);
    let lines = &text[block.clone()];
    Some(Edit {
        range: block.end..block.end,
        new_text: format!("\n{lines}"),
        selection: shift(&selection, lines.len() as isize + 1),
    })
}

/// Delete the touched lines with their newline and put the cursor at the start
/// of the line that takes their place.
pub fn delete_line(text: &str, selection: Range<usize>) -> Option<Edit> {
    if text.is_empty() {
        return None;
    }
    let selection = clamp(text, &selection);
    let block = touched_block(text, &selection);
    let (range, cursor) = if block.end < text.len() {
        (block.start..block.end + 1, block.start)
    } else if block.start > 0 {
        let prev_start = line_start(text, block.start - 1);
        (block.start - 1..block.end, prev_start)
    } else {
        (0..text.len(), 0)
    };
    Some(Edit {
        range,
        new_text: String::new(),
        selection: cursor..cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edit: &Edit) -> String {
        let mut out = text.to_string();
        out.replace_range(edit.range.clone(), &edit.new_text);
        out
    }

    /// Run `op` on `text` and return the edited text and selection.
    fn run(
        op: fn(&str, Range<usize>) -> Option<Edit>,
        text: &str,
        selection: Range<usize>,
    ) -> Option<(String, Range<usize>)> {
        op(text, selection).map(|edit| (apply(text, &edit), edit.selection))
    }

    #[test]
    fn comment_cursor_line() {
        let text = "a\n1.1.1.1 b\nc";
        let (out, sel) = run(toggle_comment, text, 5..5).unwrap();
        assert_eq!(out, "a\n# 1.1.1.1 b\nc");
        assert_eq!(sel, 7..7);

        let (back, sel) = run(toggle_comment, &out, sel).unwrap();
        assert_eq!(back, text);
        assert_eq!(sel, 5..5);
    }

    #[test]
    fn comment_cursor_at_line_start() {
        let (out, sel) = run(toggle_comment, "x\ny", 2..2).unwrap();
        assert_eq!(out, "x\n# y");
        assert_eq!(sel, 4..4);
        let (out, sel) = run(toggle_comment, &out, 2..2).unwrap();
        assert_eq!(out, "x\ny");
        assert_eq!(sel, 2..2);
    }

    #[test]
    fn comment_multi_line_selection_reselects_block() {
        let text = "a\nb\nc\nd";
        let (out, sel) = run(toggle_comment, text, 3..5).unwrap();
        assert_eq!(out, "a\n# b\n# c\nd");
        assert_eq!(&out[sel], "# b\n# c");
    }

    #[test]
    fn selection_ending_at_column_zero_excludes_that_line() {
        let text = "a\nb\nc";
        let (out, _) = run(toggle_comment, text, 0..4).unwrap();
        assert_eq!(out, "# a\n# b\nc");
    }

    #[test]
    fn mixed_lines_get_commented() {
        let text = "# a\nb\n\n# c";
        let (out, _) = run(toggle_comment, text, 0..text.len()).unwrap();
        assert_eq!(out, "# # a\n# b\n\n# # c");
    }

    #[test]
    fn uncomment_skips_blank_lines_and_handles_missing_space() {
        let text = "#a\n\n# b\n  #  c";
        let (out, sel) = run(toggle_comment, text, 0..text.len()).unwrap();
        assert_eq!(out, "a\n\nb\n   c");
        assert_eq!(sel, 0..out.len());
    }

    #[test]
    fn comment_at_minimum_indent() {
        let text = "    a\n  b\n\n  c";
        let (out, _) = run(toggle_comment, text, 0..text.len()).unwrap();
        assert_eq!(out, "  #   a\n  # b\n\n  # c");
        let (out, _) = run(toggle_comment, "  a\n\tb", 0..6).unwrap();
        assert_eq!(out, " #  a\n\t# b");
    }

    #[test]
    fn indented_comment_toggle_roundtrip() {
        let text = "  1.1.1.1 a\n  2.2.2.2 b";
        let (out, sel) = run(toggle_comment, text, 0..text.len()).unwrap();
        assert_eq!(out, "  # 1.1.1.1 a\n  # 2.2.2.2 b");
        let (back, _) = run(toggle_comment, &out, sel).unwrap();
        assert_eq!(back, text);
    }

    #[test]
    fn all_blank_lines_get_commented() {
        let (out, _) = run(toggle_comment, "a\n\n  \nb", 2..5).unwrap();
        assert_eq!(out, "a\n# \n#   \nb");
        let (out, sel) = run(toggle_comment, "", 0..0).unwrap();
        assert_eq!((out.as_str(), sel), ("# ", 2..2));
    }

    #[test]
    fn comment_with_multibyte_content() {
        let text = "中文 a\n127.0.0.1 测试";
        let cursor = text.find("测").unwrap();
        let (out, sel) = run(toggle_comment, text, cursor..cursor).unwrap();
        assert_eq!(out, "中文 a\n# 127.0.0.1 测试");
        assert_eq!(&out[sel.start..], "测试");
        let (out, _) = run(toggle_comment, &out, 0..out.len()).unwrap();
        assert_eq!(out, "# 中文 a\n# # 127.0.0.1 测试");
    }

    #[test]
    fn move_up_and_down() {
        let text = "a\nb\nc\n";
        let (out, sel) = run(move_line_up, text, 3..3).unwrap();
        assert_eq!(out, "b\na\nc\n");
        assert_eq!(sel, 1..1);
        let (out, sel) = run(move_line_down, &out, sel).unwrap();
        assert_eq!(out, text);
        assert_eq!(sel, 3..3);
    }

    #[test]
    fn move_edges_are_noops() {
        assert!(move_line_up("a\nb", 0..1).is_none());
        assert!(move_line_down("a\nb", 2..3).is_none());
        assert!(move_line_down("a\nb\n", 2..2).is_none());
    }

    #[test]
    fn move_without_trailing_newline() {
        let (out, sel) = run(move_line_up, "a\nb\nc", 5..5).unwrap();
        assert_eq!(out, "a\nc\nb");
        assert_eq!(sel, 3..3);
        let (out, sel) = run(move_line_down, "a\nb\nc", 2..3).unwrap();
        assert_eq!(out, "a\nc\nb");
        assert_eq!(&out[sel], "b");
    }

    #[test]
    fn move_multi_line_block_keeps_selection() {
        let text = "a\nb\nc\nd";
        let (out, sel) = run(move_line_down, text, 2..5).unwrap();
        assert_eq!(out, "a\nd\nb\nc");
        assert_eq!(&out[sel.clone()], "b\nc");
        let (out, sel) = run(move_line_up, &out, sel).unwrap();
        assert_eq!(out, text);
        assert_eq!(&out[sel], "b\nc");
    }

    #[test]
    fn move_multibyte_lines() {
        let text = "甲\n乙乙\n";
        let (out, sel) = run(move_line_down, text, 0..0).unwrap();
        assert_eq!(out, "乙乙\n甲\n");
        assert_eq!(&out[sel.start..], "甲\n");
    }

    #[test]
    fn duplicate_cursor_line_and_block() {
        let (out, sel) = run(duplicate_line, "a\nb", 3..3).unwrap();
        assert_eq!(out, "a\nb\nb");
        assert_eq!(sel, 5..5);

        let text = "a\nb\nc\n";
        let (out, sel) = run(duplicate_line, text, 0..3).unwrap();
        assert_eq!(out, "a\nb\na\nb\nc\n");
        assert_eq!(&out[sel], "a\nb");
    }

    #[test]
    fn duplicate_multibyte() {
        let (out, sel) = run(duplicate_line, "中文", 3..3).unwrap();
        assert_eq!(out, "中文\n中文");
        assert_eq!(&out[sel.start..], "文");
    }

    #[test]
    fn delete_lines() {
        let (out, sel) = run(delete_line, "a\nb\nc", 2..2).unwrap();
        assert_eq!((out.as_str(), sel), ("a\nc", 2..2));

        let (out, sel) = run(delete_line, "a\nb\nc", 5..5).unwrap();
        assert_eq!((out.as_str(), sel), ("a\nb", 2..2));

        let (out, sel) = run(delete_line, "a\nb\nc\n", 0..3).unwrap();
        assert_eq!((out.as_str(), sel), ("c\n", 0..0));

        let (out, sel) = run(delete_line, "only", 1..1).unwrap();
        assert_eq!((out.as_str(), sel), ("", 0..0));

        assert!(delete_line("", 0..0).is_none());
    }

    #[test]
    fn crlf_line_ends_stay_with_their_line() {
        let text = "a\r\nb\r\n";
        let (out, sel) = run(toggle_comment, text, 0..0).unwrap();
        assert_eq!((out.as_str(), sel.clone()), ("# a\r\nb\r\n", 2..2));
        let (out, _) = run(toggle_comment, &out, sel).unwrap();
        assert_eq!(out, text);

        let (out, _) = run(toggle_comment, "#\r\n", 0..3).unwrap();
        assert_eq!(out, "\r\n");
        let (out, _) = run(move_line_down, text, 0..0).unwrap();
        assert_eq!(out, "b\r\na\r\n");
        let (out, _) = run(duplicate_line, text, 3..3).unwrap();
        assert_eq!(out, "a\r\nb\r\nb\r\n");
        let (out, _) = run(delete_line, text, 0..0).unwrap();
        assert_eq!(out, "b\r\n");
    }

    #[test]
    fn reversed_selection_is_ordered() {
        let text = "a\nb\nc\nd";
        let reversed = |start, end| Range { start, end };
        assert_eq!(toggle_comment(text, reversed(5, 3)), toggle_comment(text, 3..5));
        assert_eq!(move_line_down(text, reversed(5, 2)), move_line_down(text, 2..5));
        assert_eq!(delete_line(text, reversed(99, 2)), delete_line(text, 2..text.len()));
    }

    #[test]
    fn cursor_after_final_newline() {
        let text = "a\n";
        let (out, sel) = run(toggle_comment, text, 2..2).unwrap();
        assert_eq!((out.as_str(), sel), ("a\n# ", 4..4));
        assert!(move_line_down(text, 2..2).is_none());
        let (out, sel) = run(move_line_up, text, 2..2).unwrap();
        assert_eq!((out.as_str(), sel), ("\na", 0..0));
        let (out, sel) = run(duplicate_line, text, 2..2).unwrap();
        assert_eq!((out.as_str(), sel), ("a\n\n", 3..3));
        let (out, sel) = run(delete_line, text, 2..2).unwrap();
        assert_eq!((out.as_str(), sel), ("a", 0..0));
    }

    #[test]
    fn uncomment_bare_hash_line() {
        for cursor in 0..=1 {
            let (out, sel) = run(toggle_comment, "#", cursor..cursor).unwrap();
            assert_eq!((out.as_str(), sel), ("", 0..0));
        }
        let (out, sel) = run(toggle_comment, "x\n  #", 5..5).unwrap();
        assert_eq!((out.as_str(), sel), ("x\n  ", 4..4));
    }

    #[test]
    fn delete_multibyte_line() {
        let (out, sel) = run(delete_line, "中\n文\n", 4..4).unwrap();
        assert_eq!((out.as_str(), sel), ("中\n", 4..4));
    }
}

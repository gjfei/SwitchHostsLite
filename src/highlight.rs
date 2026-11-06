use std::{
    net::IpAddr,
    ops::Range,
    rc::Rc,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui_kit::component::{Rope, RopeExt as _};
use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory,
};
use gpui_kit::{Context, HighlightStyle, Hsla, SharedString, Window, hsla};

/// Whether the dark theme is active. The style resolver does not tell, and
/// the fallback colors must stay readable on either background.
static DARK: AtomicBool = AtomicBool::new(false);
/// The theme's danger color. Syntax themes have no style for invalid text.
static DANGER: Mutex<Option<Hsla>> = Mutex::new(None);

pub fn set_theme_colors(dark: bool, danger: Hsla) {
    DARK.store(dark, Ordering::Relaxed);
    if let Ok(mut color) = DANGER.lock() {
        *color = Some(danger);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    Comment,
    Ip,
    Invalid,
}

impl Token {
    fn fallback(self, dark: bool) -> Hsla {
        match (self, dark) {
            (Token::Comment, false) => hsla(0., 0., 0.5, 1.),
            (Token::Comment, true) => hsla(0., 0., 0.6, 1.),
            (Token::Ip, false) => hsla(210. / 360., 0.75, 0.45, 1.),
            (Token::Ip, true) => hsla(210. / 360., 0.85, 0.68, 1.),
            (Token::Invalid, false) => hsla(0., 0.7, 0.5, 1.),
            (Token::Invalid, true) => hsla(0., 0.85, 0.68, 1.),
        }
    }

    fn style(self, resolver: &dyn HighlightStyleResolver) -> HighlightStyle {
        let dark = DARK.load(Ordering::Relaxed);
        let style = match self {
            Token::Comment => resolver.style("comment"),
            Token::Ip => resolver.style("number"),
            Token::Invalid => None,
        };
        style.unwrap_or_else(|| {
            let danger = (self == Token::Invalid).then(|| DANGER.lock().ok().and_then(|c| *c)).flatten();
            HighlightStyle {
                color: Some(danger.unwrap_or_else(|| self.fallback(dark))),
                ..Default::default()
            }
        })
    }
}

/// Highlights hosts files: comments, IP addresses, and lines whose first
/// field is not a valid IP address.
#[derive(Default)]
struct HostsHighlighter {
    /// Ordered by position. Tokens never span lines, so an edit only
    /// retokenizes the lines it touches.
    tokens: Vec<(Range<usize>, Token)>,
    /// Whether `tokens` covers the whole text, so edits can be applied to it.
    synced: bool,
}

/// Append the tokens of one line (without its line end) starting at byte `line_start`.
fn tokenize_line(line: &str, line_start: usize, tokens: &mut Vec<(Range<usize>, Token)>) {
    let body = line.trim_end_matches(['\n', '\r']);
    let (code, comment_at) = match body.find('#') {
        Some(ix) => (&body[..ix], Some(ix)),
        None => (body, None),
    };

    let trimmed = code.trim_start();
    if !trimmed.is_empty() {
        let start = code.len() - trimmed.len();
        let field = trimmed.split_whitespace().next().unwrap_or_default();
        let kind = if field.parse::<IpAddr>().is_ok() {
            Token::Ip
        } else {
            Token::Invalid
        };
        let abs = line_start + start;
        tokens.push((abs..abs + field.len(), kind));
    }
    if let Some(ix) = comment_at {
        tokens.push((line_start + ix..line_start + body.len(), Token::Comment));
    }
}

#[cfg(test)]
fn tokenize(text: &str) -> Vec<(Range<usize>, Token)> {
    let mut tokens = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        tokenize_line(line, line_start, &mut tokens);
        line_start += line.len();
    }
    tokens
}

/// Tokens of the lines `rows`, read from the rope without copying the whole text.
fn tokenize_rows(text: &Rope, rows: Range<usize>) -> Vec<(Range<usize>, Token)> {
    let mut tokens = Vec::new();
    for row in rows {
        let line = std::borrow::Cow::<str>::from(text.slice_line(row));
        tokenize_line(&line, text.line_start_offset(row), &mut tokens);
    }
    tokens
}

impl HostsHighlighter {
    /// Retokenize the lines touched by `edit`, shifting the tokens after them.
    fn apply_edit(&mut self, edit: &InputEdit, text: &Rope) {
        let first = text.offset_to_point(edit.start_byte).row;
        let last = text.offset_to_point(edit.new_end_byte).row;
        let start = text.line_start_offset(first);
        let new_end = text.line_start_offset(last + 1);
        // Everything after the edit only moved, so the old end of the
        // retokenized lines is their new end minus the growth.
        let delta = edit.new_end_byte as isize - edit.old_end_byte as isize;
        let old_end = (new_end as isize - delta) as usize;

        let from = self.tokens.partition_point(|(r, _)| r.start < start);
        let to = self.tokens.partition_point(|(r, _)| r.start < old_end);
        for (range, _) in &mut self.tokens[to..] {
            range.start = (range.start as isize + delta) as usize;
            range.end = (range.end as isize + delta) as usize;
        }
        self.tokens.splice(from..to, tokenize_rows(text, first..last + 1));
    }
}

impl InputHighlighter for HostsHighlighter {
    fn language(&self) -> SharedString {
        "hosts".into()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        match edit {
            Some(edit) if self.synced => self.apply_edit(&edit, text),
            _ => {
                self.tokens = tokenize_rows(text, 0..text.lines_len());
                self.synced = true;
            }
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut out = Vec::new();
        let mut cursor = range.start;
        let first = self.tokens.partition_point(|(r, _)| r.end <= range.start);
        for (token_range, token) in &self.tokens[first..] {
            if token_range.start >= range.end {
                break;
            }
            let start = token_range.start.max(range.start);
            let end = token_range.end.min(range.end);
            if start > cursor {
                out.push((cursor..start, HighlightStyle::default()));
            }
            out.push((start..end, token.style(resolver)));
            cursor = end;
        }
        if cursor < range.end {
            out.push((cursor..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

pub fn factory() -> InputHighlighterFactory {
    Rc::new(|_| Some(Box::new(HostsHighlighter::default()) as Box<dyn InputHighlighter>))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_hosts_lines() {
        let text = "# c\n127.0.0.1 a # x\nbad host\n  ::1 b\n";
        let kinds: Vec<_> = tokenize(text)
            .into_iter()
            .map(|(r, t)| (&text[r], t))
            .collect();
        assert!(kinds[0] == ("# c", Token::Comment));
        assert!(kinds[1] == ("127.0.0.1", Token::Ip));
        assert!(kinds[2] == ("# x", Token::Comment));
        assert!(kinds[3] == ("bad", Token::Invalid));
        assert!(kinds[4] == ("::1", Token::Ip));
    }

    #[test]
    fn edits_match_a_full_retokenize() {
        use gpui_kit::component::input::Point;

        let mut text = Rope::from_str("# c\n127.0.0.1 a # x\nbad host\n  ::1 b\n");
        let mut highlighter = HostsHighlighter {
            tokens: tokenize(&text.to_string()),
            synced: true,
        };
        let edits: [(Range<usize>, &str); 6] = [
            (0..0, "10.0.0.1 new\n"),
            (17..19, "#"),
            (20..33, ""),
            (5..5, "\n\nx y # z"),
            (0..4, "::1"),
            (text.len()..text.len(), "1.1.1.1 end"),
        ];
        for (range, new_text) in edits {
            let range = range.start.min(text.len())..range.end.min(text.len());
            text.replace(range.clone(), new_text);
            let p = Point::new(0, 0);
            let edit = InputEdit {
                start_byte: range.start,
                old_end_byte: range.end,
                new_end_byte: range.start + new_text.len(),
                start_position: p,
                old_end_position: p,
                new_end_position: p,
            };
            highlighter.apply_edit(&edit, &text);
            assert_eq!(highlighter.tokens, tokenize(&text.to_string()), "after {range:?} -> {new_text:?}");
        }
    }

    #[test]
    fn fallbacks_contrast_with_the_background() {
        for token in [Token::Comment, Token::Ip, Token::Invalid] {
            // Light backgrounds need darker text, dark backgrounds lighter text.
            assert!(token.fallback(false).l <= 0.5, "{token:?}");
            assert!(token.fallback(true).l >= 0.6, "{token:?}");
        }
    }
}

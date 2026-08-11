//! Syntax highlighting for the related-code view, via `syntect`.
//!
//! Highlighting degrades gracefully: if a syntax or theme can't be resolved we
//! fall back to un-highlighted text rather than failing the whole UI.

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Owns the syntax/theme sets and hands out per-file highlighters.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub fn new() -> Self {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let mut themes = ThemeSet::load_defaults();
        // A dark theme that reads well in most terminals.
        let theme = themes
            .themes
            .remove("base16-ocean.dark")
            .or_else(|| themes.themes.remove("base16-eighties.dark"))
            .unwrap_or_else(|| ThemeSet::load_defaults().themes["InspiredGitHub"].clone());
        Highlighter { syntaxes, theme }
    }

    /// Resolve a syntax for a file by extension, falling back to plain text.
    pub fn syntax_for<'a>(&'a self, file: &str) -> &'a SyntaxReference {
        let ext = std::path::Path::new(file)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        self.syntaxes
            .find_syntax_by_extension(ext)
            .or_else(|| {
                // A couple of common names without a helpful extension.
                let name = std::path::Path::new(file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                self.syntaxes.find_syntax_by_token(name)
            })
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
    }

    /// Begin highlighting a fresh block with the given syntax.
    pub fn line_highlighter<'a>(&'a self, syntax: &'a SyntaxReference) -> LineHighlighter<'a> {
        LineHighlighter {
            inner: HighlightLines::new(syntax, &self.theme),
            syntaxes: &self.syntaxes,
        }
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Highlights successive lines of one code block, carrying multi-line state.
pub struct LineHighlighter<'a> {
    inner: HighlightLines<'a>,
    syntaxes: &'a SyntaxSet,
}

impl LineHighlighter<'_> {
    /// Highlight one line, returning `(fg color, text)` spans. On any error the
    /// whole line is returned as a single span with no color.
    pub fn highlight(&mut self, line: &str) -> Vec<(Option<Color>, String)> {
        match self.inner.highlight_line(line, self.syntaxes) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(style, text)| (Some(to_color(style.foreground)), text.to_string()))
                .collect(),
            Err(_) => vec![(None, line.to_string())],
        }
    }
}

fn to_color(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_syntax_by_extension() {
        let h = Highlighter::new();
        let rs = h.syntax_for("src/main.rs");
        assert_eq!(rs.name.to_lowercase(), "rust");
        // unknown extension -> plain text, still returns something
        let _ = h.syntax_for("weird.zzz");
    }

    #[test]
    fn highlights_a_rust_line() {
        let h = Highlighter::new();
        let syntax = h.syntax_for("a.rs");
        let mut lh = h.line_highlighter(syntax);
        let spans = lh.highlight("fn main() {}\n");
        // We should get at least one span and the concatenation preserves text.
        let joined: String = spans.iter().map(|(_, t)| t.as_str()).collect();
        assert!(joined.contains("fn main"));
        assert!(!spans.is_empty());
    }
}

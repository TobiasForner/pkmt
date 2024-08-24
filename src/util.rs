use logos::{Lexer, Logos};

pub fn indent_level(line: &str, spaces_per_indent: usize) -> usize {
    let indent_pattern = " ".repeat(spaces_per_indent);
    let line = line.replace("\t", &indent_pattern);
    let mut res = 0;
    let mut pos = 0;
    while pos < line.len() && line[pos..].starts_with(&indent_pattern) {
        res += 1;
        pos += spaces_per_indent;
    }
    res
}

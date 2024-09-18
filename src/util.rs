use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use regex::Captures;

pub const SPACES_PER_INDENT: usize = 4;

pub fn apply_substitutions(text: &str) -> String {
    text.replace('−', "-")
        .replace('∗', "*")
        .replace('∈', "\\in ")
        .replace("“", "\"")
        .replace("”", "\"")
        .replace("∃", "EXISTS")
        .replace("’", "'")
        .replace("–", "-")
}

pub fn indent_level(line: &str) -> usize {
    let indent_pattern = " ".repeat(SPACES_PER_INDENT);
    let line = line.replace("\t", &indent_pattern);
    let mut res = 0;
    let mut pos = 0;
    while pos < line.len() && line[pos..].starts_with(&indent_pattern) {
        res += 1;
        pos += SPACES_PER_INDENT;
    }
    res
}

pub fn overlapping_captures(
    text: &str,
    re: regex::Regex,
    move_after_ith_group: usize,
) -> Vec<Captures<'_>> {
    let mut pos = 0;
    let mut res = vec![];
    loop {
        let Some(captures) = re.captures_at(text, pos) else {
            return res;
        };
        pos = captures.get(move_after_ith_group).unwrap().end();
        res.push(captures);
    }
}

pub fn trim_like_first_line_plus(text: &str, extra: usize) -> String {
    let indent_pattern = " ".repeat(SPACES_PER_INDENT);
    let text = text.replace("\t", &indent_pattern);

    let mut res = String::new();
    let mut space_count = 0;
    text.lines().enumerate().for_each(|(i, l)| {
        let mut start = true;
        if i == 0 {
            l.chars().for_each(|c| {
                if start && c == ' ' {
                    space_count += 1;
                } else {
                    start = false;
                    res.push(c);
                }
            })
        } else {
            res.push('\n');
            l.chars().enumerate().for_each(|(char_pos, ch)| {
                if ch != ' ' {
                    start = false;
                }
                if !start || char_pos >= space_count + extra {
                    res.push(ch);
                }
            });
        }
    });
    res
}

pub fn files_in_tree<T: AsRef<Path>>(
    root_dir: T,
    allowed_extensions: &Option<Vec<&str>>,
) -> Result<Vec<PathBuf>> {
    let mut res = vec![];
    let root_dir = root_dir.as_ref().canonicalize()?;
    let dir_entry = root_dir.read_dir()?;
    let tmp: Result<()> = dir_entry.into_iter().try_for_each(|f| {
        let path = f.unwrap().path();
        if path.is_dir() {
            let rec = files_in_tree(&path, allowed_extensions)?;
            res.extend(rec);
        } else if let Some(ext) = path.extension() {
            if let Some(extensions) = allowed_extensions {
                if extensions.contains(&ext.to_str().unwrap_or("should not be found")) {
                    res.push(path.clone());
                }
            } else {
                res.push(path.clone());
            }
        }
        Ok(())
    });
    if tmp.is_err() {
        bail!("Encountered error: {tmp:?}!")
    }
    Ok(res)
}

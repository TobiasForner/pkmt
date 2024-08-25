use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

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

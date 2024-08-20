use anyhow::Result;
use std::path::Path;

pub fn checklist_for_tree<T: AsRef<Path>>(root_dir: T, todo_marker: &str) -> Result<String> {
    let root_dir = root_dir.as_ref().canonicalize()?;
    let dir_entry = root_dir.read_dir()?;
    let mut files = vec![];
    let mut dirs = vec![];
    dir_entry.into_iter().try_for_each(|f| {
        let path = f.unwrap().path();
        if path.is_dir() {
            dirs.push(path);
        } else if let Some(ext) = path.extension() {
            if ["md"].contains(&ext.to_str().unwrap_or("should not be found")) {
                files.push(path);
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;

    let mut lines = vec![format!("- {todo_marker} `{}`", root_dir.to_string_lossy())];
    if !files.is_empty() {
        lines.push(format!("\t- {todo_marker} files in directory"));
        files.iter().for_each(|f| {
            let rel = pathdiff::diff_paths(f, &root_dir).unwrap();
            lines.push(format!("\t\t- {todo_marker} `{}`", rel.to_string_lossy()));
        });
    }
    if !dirs.is_empty() {
        let dir_text = dirs
            .iter()
            .map(|d| {
                let rec = checklist_for_tree(d, todo_marker)?;
                let rec: Vec<String> = rec.lines().map(|l| format!("\t{l}")).collect();
                Ok(rec.join("\n"))
            })
            .collect::<Result<Vec<String>>>()?;
        lines.extend(dir_text);
    }

    Ok(lines.join("\n"))
}

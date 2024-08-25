use anyhow::Result;
use std::path::PathBuf;
use std::result::Result::Ok;

use crate::util::files_in_tree;

pub fn list_empty_files(root_dir: PathBuf) -> Result<()> {
    let empty_files = get_empty_files(root_dir)?;
    empty_files.iter().for_each(|f| println!("{f:?} is empty!"));
    Ok(())
}

fn get_empty_files(root_dir: PathBuf) -> Result<Vec<PathBuf>> {
    let files = files_in_tree(root_dir, &Some(vec!["md"]))?;
    let res = files
        .into_iter()
        .filter(|f| {
            if let Ok(text) = std::fs::read_to_string(f) {
                text.replace("-", "").is_empty()
            } else {
                false
            }
        })
        .collect();
    Ok(res)
}

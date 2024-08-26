use anyhow::Result;
use edit_distance::edit_distance;
use std::collections::HashMap;
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

pub fn similar_file_names(root_dir: PathBuf, threshold: usize) {
    let files = files_in_tree(root_dir, &Some(vec!["md"])).unwrap();
    let file_names: Vec<(String, PathBuf)> = files
        .iter()
        .filter(|f| {
            !f.components()
                .any(|c| c.as_os_str().to_str().unwrap() == "bak")
        })
        .map(|f| {
            (
                f.file_name().unwrap().to_string_lossy().to_string(),
                f.clone(),
            )
        })
        .collect();
    println!("Found {} files!", file_names.len());
    let mut clustering: Vec<usize> = (0..file_names.len()).collect();
    println!("Building initial clustering");
    (0..file_names.len().saturating_sub(1)).for_each(|a| {
        println!("{a}");
        let (first, _) = &file_names[a];
        ((a + 1)..file_names.len()).for_each(|b| {
            let (second, _) = &file_names[b];
            if edit_distance(first, second) <= threshold {
                clustering[a] = b;
            }
        })
    });

    println!("Shortcutting clustering");
    // shortcut clustering
    (0..file_names.len()).rev().for_each(|i| {
        let next = clustering[i];
        clustering[i] = clustering[next]
    });

    println!("Building final clusters");
    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    clustering.iter().enumerate().for_each(|(id, cluster_id)| {
        if let Some(v) = clusters.get_mut(cluster_id) {
            v.push(id);
        } else {
            clusters.insert(*cluster_id, vec![id]);
        }
    });

    let clusters = clusters;
    clusters.iter().for_each(|(_, components)| {
        if components.len() > 1 {
            let files: Vec<&PathBuf> = file_names
                .iter()
                .enumerate()
                .filter(|(index, _)| components.contains(index))
                .map(|(_, (_, f))| f)
                .collect();

            println!("The following files have very similar names:");
            files.iter().for_each(|f| println!("\t{f:?}"));
        }
    });
}

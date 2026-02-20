use anyhow::{Context, Result, bail};
use edit_distance::edit_distance;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::result::Result::Ok;

use crate::document_component::DocumentComponent;
use crate::parsing::parse_all_files_in_dir;
use crate::util::files_in_tree;

pub fn doctor(root_dir: &Path, fix: bool) -> Result<()> {
    list_empty_files(root_dir)?;
    similar_file_names(root_dir, 4);
    problematic_file_titles(root_dir, fix)
}

fn problematic_file_titles(root_dir: &Path, fix: bool) -> Result<()> {
    let mut overwrite_all = false;
    parse_all_files_in_dir(&root_dir.to_path_buf(), &crate::parsing::TextMode::Zk)?
        .iter_mut()
        .for_each(|pd| {
            let orig_text = pd.to_zk_text(&None);
            if let Some(p) = pd.get_file_path() {
                let heading = pd.get_document_component_mut(&|comp| {
                    matches!(comp, DocumentComponent::Heading(_, _))
                });
                if let Some(title) = heading
                    && let DocumentComponent::Heading(_, title) = title
                    && title.starts_with('"')
                    && title.ends_with('"')
                {
                    println!(
                        "Title surrounded by quotation marks: {title:?}! (file: {:?})",
                        p
                    );
                    if fix {
                        let new_title = title.trim_matches('"');
                        *title = new_title.to_string();
                        let new_text = pd.to_zk_text(&None);
                        println!("========== {p:?}\n{orig_text}\n =>\n{new_text}");
                        let mut should_change = overwrite_all;
                        while !overwrite_all
                            && let Ok(answer) =
                                get_user_input("Should the text be replaced (y/n/a)?")
                            && !(answer == "y" || answer == "n")
                        {
                            if answer == "y" {
                                should_change = true;
                                break;
                            } else if answer == "n" {
                                // should_change defaults to false
                                break;
                            } else if answer == "a" {
                                overwrite_all = true;
                                should_change = true;
                            }
                        }
                        if should_change {
                            let _ = std::fs::write(&p, new_text)
                                .context("Failed to write to file {p:?}");
                        }
                    }
                };
            }
        });
    Ok(())
}

fn get_user_input(prompt: &str) -> Result<String> {
    println!("{prompt}: ");
    let mut answer = Default::default();
    if std::io::stdin().read_line(&mut answer).is_ok() {
        Ok(answer.trim().to_string())
    } else {
        bail!("Failed to get input!")
    }
}

fn list_empty_files(root_dir: &Path) -> Result<()> {
    let empty_files = get_empty_files(root_dir)?;
    empty_files.iter().for_each(|f| println!("{f:?} is empty!"));
    Ok(())
}

fn get_empty_files(root_dir: &Path) -> Result<Vec<PathBuf>> {
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

fn similar_file_names(root_dir: &Path, threshold: usize) {
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

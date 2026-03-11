use anyhow::{Context, Result, bail};
use edit_distance::edit_distance;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::result::Result::Ok;

use crate::document_component::{
    DocumentComponent, FileInfo, MentionedFile, ParsedDocument, PropValue, Property,
};
use crate::parsing::{TextMode, parse_all_files_in_dir};
use crate::util::files_in_tree;

pub fn doctor(root_dir: &Path, fix: bool) -> Result<()> {
    let mode = TextMode::Zk;
    let mut parsed_documents = parse_all_files_in_dir(&root_dir.to_path_buf(), &mode)?;
    list_empty_files(root_dir)?;
    problematic_file_titles(&mut parsed_documents, fix)?;
    similar_file_names(&mut parsed_documents, root_dir, 2, &mode, fix)?;
    check_creator_file(root_dir, 2, fix)
}

fn problematic_file_titles(parsed_documents: &mut [ParsedDocument], fix: bool) -> Result<()> {
    let mut overwrite_all = false;
    parsed_documents.iter_mut().for_each(|pd| {
        let file_info = pd.get_file_path().map(FileInfo::new_same_file);
        let orig_text = pd.to_zk_text(&file_info);
        if let Some(p) = pd.get_file_path() {
            // TODO: also try to find the title based on frontmatter
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
                        && let Ok(answer) = get_user_input("Should the text be replaced (y/n/a)?")
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
                        let _ =
                            std::fs::write(&p, new_text).context("Failed to write to file {p:?}");
                    }
                }
            };
        }
    });
    Ok(())
}

fn get_user_input_choices(prompt: &str, choices: Vec<String>) -> Option<String> {
    let mut answer = None;
    while answer.is_none()
        && let Ok(a) = get_user_input(prompt)
    {
        if choices.contains(&a) {
            answer = Some(a);
        }
    }
    answer
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

fn similar_file_names(
    parsed_documents: &mut Vec<ParsedDocument>,
    root_dir: &Path,
    threshold: usize,
    mode: &TextMode,
    fix: bool,
) -> Result<()> {
    println!("Checking for similar file names...");

    let dot_zk_dir = root_dir.to_path_buf().join(".zk");
    let journal_dir = root_dir.to_path_buf().join("journal");
    //file name, file path, parsed file
    let file_names: Vec<(usize, String, PathBuf, &mut ParsedDocument)> = parsed_documents
        .iter_mut()
        .enumerate()
        .filter_map(|(index, pd)| {
            if let Some(fp) = pd.get_file_path() {
                if !fp
                    .components()
                    .any(|c| c.as_os_str().to_str().unwrap() == "bak")
                    && !fp.starts_with(&dot_zk_dir)
                    && !fp.starts_with(&journal_dir)
                {
                    let file_name = pd.get_title(mode);
                    file_name.map(|file_name| (index, file_name, fp, pd))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    println!("Found {} files!", file_names.len());
    // maps index_a to index_b, both are indices into file_names. Note that file names is a
    // filtered version of parsed_documents (with some more data extracted from the pd)
    let mut clustering: Vec<usize> = (0..file_names.len()).collect();
    println!("Building initial clustering");
    (0..file_names.len().saturating_sub(1)).for_each(|a| {
        let (_, first, _, _) = &file_names[a];
        ((a + 1)..file_names.len()).for_each(|b| {
            let (_, second, _, _) = &file_names[b];
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
    // maps clustering index to a vec of indices into parsed_documents
    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    clustering.iter().enumerate().for_each(|(id, cluster_id)| {
        let pd_id = file_names[id].0;
        if let Some(v) = clusters.get_mut(cluster_id) {
            v.push(pd_id);
        } else {
            clusters.insert(*cluster_id, vec![pd_id]);
        }
    });

    let clusters = clusters;
    let mut all_invalid_indices = vec![];
    clusters.iter().for_each(|(_, components)| {
        if components.len() > 1 {
            if fix {
                let res = resolve_duplicates_in_cluster(parsed_documents, components);
                if let Ok(mut invalid_indices) = res {
                    all_invalid_indices.append(&mut invalid_indices);
                }
            } else {
                println!("The following files have very similar names:",);
                components.iter().for_each(|index| {
                    let pd = &parsed_documents[*index];
                    let title = pd.get_title(mode).unwrap();
                    let path = pd.get_file_path().unwrap();
                    println!("\t{title} ({path:?})")
                });
            }
        }
    });
    all_invalid_indices.sort();
    all_invalid_indices.into_iter().rev().for_each(|index| {
        parsed_documents.remove(index);
    });
    Ok(())
}

/// returns indices in parsed_documents that are invalid because the associated files have been
/// deleted
fn resolve_duplicates_in_cluster(
    parsed_documents: &mut [ParsedDocument],
    duplicate_indices: &[usize],
) -> Result<Vec<usize>> {
    use DocumentComponent::Frontmatter as DCF;
    use PropValue::String as PVS;
    let extract_date = |properties: &Vec<Property>| {
        if let Some(prop) = properties.iter().find(|p| p.has_name("date"))
            && let Some(PVS(s)) = prop.values.first()
        {
            Some(s.to_string())
        } else {
            None
        }
    };
    let mut date_annotated: Vec<_> = parsed_documents
        .iter()
        .enumerate()
        .filter(|(index, _)| duplicate_indices.iter().any(|i| i == index))
        .filter_map(|(index, pd)| {
            if let Some(DCF(properties)) = pd.get_document_component(&|comp| matches!(comp, DCF(_)))
            {
                let file = pd.get_file_path().unwrap();
                extract_date(&properties).map(|date| (index, file, pd, date))
            } else {
                println!("did not find date!");
                None
            }
        })
        .collect();
    date_annotated.sort_by(|(_, _, _, date1), (_, _, _, date2)| date1.cmp(date2));
    println!("###############");
    println!("The following files have very similar names:");

    date_annotated.iter().for_each(|(_, path, pd, date)| {
        println!(
            "----- {path:?} ({date}) -----\n{}",
            pd.to_zk_text(&Some(FileInfo::new_same_file(path.to_path_buf())))
        )
    });
    let from_with_pd_indices: Vec<_> = date_annotated[1..]
        .iter()
        .map(|(index, p, _, _)| (*index, p.to_path_buf()))
        .collect();
    let from: Vec<PathBuf> = from_with_pd_indices
        .iter()
        .map(|(_, p)| p.to_path_buf())
        .collect();
    let to = date_annotated[0].1.to_path_buf();

    let res = redirect_file_mentions(parsed_documents, &from, &to);
    let mut pd_indices_to_delete = Vec::new();
    if let Ok(all_overwritten) = res
        && all_overwritten
    {
        let to_del_paths: Vec<&str> = from_with_pd_indices
            .iter()
            .map(|(_, p)| p.to_str().unwrap())
            .collect();
        let answer = get_user_input_choices(
            &format!(
                "Would you like to delete the old files? This would delete the following files: {} [y/n]",
                to_del_paths.join(", ")
            ),
            vec!["y".to_string(), "n".to_string()],
        );
        if answer == Some("y".to_string()) {
            let success = from_with_pd_indices
                .iter()
                .try_for_each(|(pd_index, to_remove)| {
                    let is_deleted = std::fs::remove_file(to_remove);
                    if is_deleted.is_ok() {
                        pd_indices_to_delete.push(*pd_index);
                    }
                    is_deleted
                });
            if success.is_ok() {
                println!("Success!");
            } else {
                println!("ERROR: {success:?}");
            }
        }
    } else {
        println!("error: {res:?}");
    }
    Ok(pd_indices_to_delete)
}

fn redirect_file_mentions(
    parsed_documents: &mut [ParsedDocument],
    from: &[PathBuf],
    to: &Path,
) -> Result<bool> {
    use DocumentComponent::FileLink as DCFL;
    use DocumentComponent::Properties as DCP;
    use PropValue::FileLink as PVFL;
    let from: HashSet<PathBuf> = from.iter().cloned().collect();
    let mut all_overwritten = true;
    parsed_documents.iter_mut().for_each(|pd| {
        let path = pd.get_file_path().unwrap();
        let file_info = Some(FileInfo::new(path.to_path_buf(), None, None));
        let old_text = pd.to_zk_text(&file_info);
        let from_contains = |p: &Path| from.contains(p);
        let to_change = pd.get_all_document_components_mut(&|comp| {
            let pred = |pv: &PropValue| {
                matches!(pv,
                    PVFL(MentionedFile::FilePath(mf_path), _, _) if from_contains(mf_path))
            };
            matches!(comp, DCFL(MentionedFile::FilePath(p), _, _) if from_contains(p))
                || matches!(comp, DCP(props) if props.iter().any(|p|p.has_value_pred(&pred)))
        });

        if !to_change.is_empty() {
            println!("======= old =======\n{old_text}");
            to_change.into_iter().for_each(|comp| match comp {
                DCFL(MentionedFile::FilePath(p), _section, _rename) => {
                    *p = to.to_path_buf();
                }
                DocumentComponent::Properties(props) => {
                    props.iter_mut().for_each(|p| {
                        p.values.iter_mut().for_each(|pv| {
                            if let PVFL(MentionedFile::FilePath(file_path), _, _) = pv
                                && from.iter().any(|f| f == file_path)
                            {
                                *file_path = to.to_path_buf();
                            }
                        });
                    });
                }
                _ => {}
            });
            let new_text = pd.to_zk_text(&file_info);
            println!("====== new ====== ({path:?})\n{new_text}\n============\n\n");
            let choices = vec!["y".to_string(), "n".to_string()];
            let answer =
                get_user_input_choices("Do you want do overwrite the old file? [y/n/s]", choices);
            match &answer {
                Some(val) if val == "y" => {
                    let _ =
                        std::fs::write(&path, new_text).context("Failed to write to file {p:?}");
                }
                _ => {
                    all_overwritten = false;
                }
            }
        }
    });

    Ok(all_overwritten)
}

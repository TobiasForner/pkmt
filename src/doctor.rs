use anyhow::{Context, Result, bail};
use edit_distance::edit_distance;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
//use ratatui::crossterm::style::Stylize;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::{DefaultTerminal, Frame};
use ratatui::{prelude::*, widgets::*};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::result::Result::Ok;

use crate::convert::FileInfo;
use crate::document_component::{
    DocumentComponent, MentionedFile, ParsedDocument, PropType, PropValue, Property,
};
use crate::parsing::{TextMode, parse_all_files_in_dir};
use crate::util::files_in_tree;

pub fn doctor(root_dir: &Path, fix: bool) -> Result<()> {
    let mode = TextMode::Zk;
    let mut parsed_documents = parse_all_files_in_dir(&root_dir.to_path_buf(), &mode)?;
    list_empty_files(root_dir)?;
    problematic_file_titles(&mut parsed_documents, fix)?;
    similar_file_names(&mut parsed_documents, root_dir, 2, &mode, fix)?;
    mentiened_files_titles(&mut parsed_documents, &mode)?;
    contracted_frontmatter_lists(&mut parsed_documents, fix, &mode)
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

    // simple clustering: all titles pairs are considered. If the edit distance is <= threshold,
    // the pair is considered similar and the first file name is assigned to the cluster of the
    // first one
    // later on, when building the actual clusters ("shortcutting"), the file_names are again
    // considered in inverse order.
    // This way, when considering index i<j, we know that clustering[j] is already fully resolved
    // and it is enough to follow the cluster assignment for a single step

    // maps index_a to index_b, both are indices into file_names. Note that file names is a
    // filtered version of parsed_documents (with some more data extracted from the pd)
    // clustering[a] = b means that the ath file_name is part of the same cluster as the bth file name
    let mut clustering: Vec<usize> = (0..file_names.len()).collect();
    println!("Building initial clustering");
    let ignore_similar = IgnoredSimilarFiles::read();
    (0..file_names.len().saturating_sub(1)).for_each(|a| {
        let (_, first, a_path, _) = &file_names[a];
        ((a + 1)..file_names.len()).for_each(|b| {
            let (_, second, b_path, _) = &file_names[b];
            if edit_distance(first, second) <= threshold && !ignore_similar.contains(a_path, b_path)
            {
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
                println!("did not find date for {:?}", pd.get_file_path());
                None
            }
        })
        .collect();
    date_annotated.sort_by(|(_, _, _, date1), (_, _, _, date2)| date1.cmp(date2));
    let file_paths: Vec<(PathBuf, SimilarNameResolution)> = date_annotated
        .iter()
        .enumerate()
        .map(|(index, (_, pb, _, _))| {
            (
                pb.to_path_buf(),
                if index == 0 {
                    SimilarNameResolution::Main
                } else {
                    SimilarNameResolution::Skip
                },
            )
        })
        .collect();
    let mut similar_names_state = SimilarNamesState::new(file_paths);
    ratatui::run(|terminal| similar_names_state.run(terminal)).unwrap();
    let main = similar_names_state.file_paths.iter().find_map(|(fp, res)| {
        if *res == SimilarNameResolution::Main {
            Some(fp)
        } else {
            None
        }
    });
    let mut ignore_similar = IgnoredSimilarFiles::read();
    let mut path_index_to_delete = vec![];
    let to_del: Vec<PathBuf> = similar_names_state
        .file_paths
        .iter()
        .enumerate()
        .filter_map(|(index, (fp, res))| {
            if *res == SimilarNameResolution::Remove {
                path_index_to_delete.push((fp.clone(), date_annotated[index].0));

                Some(fp.to_path_buf())
            } else if *res == SimilarNameResolution::Ignore {
                ignore_similar.ensure_contains(main.unwrap(), fp);
                None
            } else {
                None
            }
        })
        .collect();
    redirect_file_mentions(parsed_documents, &to_del, main.unwrap())?;
    let mut pd_indices_to_delete = vec![];
    let success = path_index_to_delete
        .iter()
        .try_for_each(|(to_remove, pd_index)| {
            let is_deleted = std::fs::remove_file(to_remove);
            if is_deleted.is_ok() {
                pd_indices_to_delete.push(*pd_index);
            }
            is_deleted
        });
    if success.is_ok() {
        Ok(pd_indices_to_delete)
    } else {
        bail!("Failed to delete duplicate files: {success:?}")
    }

    /*println!("###############");
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
    Ok(pd_indices_to_delete)*/
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
            //println!("======= old =======\n{old_text}");
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
            let mut redirect_state = OverwriteConfirmState::new(
                old_text,
                new_text.clone(),
                "This is the result of the substitution:".to_string(),
            );
            ratatui::run(|terminal| redirect_state.run(terminal)).unwrap();
            if redirect_state.choice {
                let _ = std::fs::write(&path, new_text).context("Failed to write to file {p:?}");
            } else {
                all_overwritten = false;
            }
            /*println!("====== new ====== ({path:?})\n{new_text}\n============\n\n");
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
            }*/
        }
    });

    Ok(all_overwritten)
}

fn mentiened_files_titles(parsed_documents: &mut [ParsedDocument], mode: &TextMode) -> Result<()> {
    let title_by_path: HashMap<PathBuf, String> = parsed_documents
        .iter()
        .filter_map(|pd| {
            if let Some(fp) = pd.get_file_path() {
                pd.get_title(mode).map(|title| (fp, title))
            } else {
                None
            }
        })
        .collect();

    parsed_documents.iter_mut().try_for_each(|pd| {
        let pd_path = pd
            .get_file_path()
            .context("No file path for parsed document!")?;
        //they come from parse_all_files_in_dir
        // comps that have been found already
        // this is used to prevent finding the same component every time
        let mut found_comps = Vec::new();
        while let Some(DocumentComponent::FileLink(mf, sec, name)) =
            pd.get_document_component_mut(&|comp| {
                if !found_comps.iter().any(|c| c == comp)
                    && let DocumentComponent::FileLink(
                        MentionedFile::FilePath(mf_path),
                        _,
                        Some(fl_name),
                    ) = comp
                    && let Some(actual_title) = title_by_path.get(mf_path)
                    && actual_title != fl_name
                {
                    true
                } else {
                    false
                }
            })
        {
            let comp = DocumentComponent::FileLink(mf.clone(), sec.clone(), name.clone());
            if let MentionedFile::FilePath(mf_path) = mf {
                let actual_title = title_by_path.get(mf_path).unwrap();
                println!("{pd_path:?}:");
                println!("\tfound: {mf:?}\n\ttitle: {name:?}\n\tactual title: {actual_title:?}",);
            }
            found_comps.push(comp);
        }
        Ok(())
    })
}

fn contracted_frontmatter_lists(
    parsed_documents: &mut [ParsedDocument],
    fix: bool,
    mode: &TextMode,
) -> Result<()> {
    let res: Result<()> = parsed_documents.iter_mut().try_for_each(|pd| {
        let path = pd.get_file_path().context("No file path found!")?;
        let file_info = Some(FileInfo::new(path.to_path_buf(), None, None));
        let old_text = pd.to_zk_text(&file_info);
        let old_pd = pd.clone();
        if let Some(DocumentComponent::Frontmatter(properties)) =
            pd.get_document_component_mut(&|comp| {
                matches!(comp, DocumentComponent::Frontmatter(props) if props.iter().any(|p|
                {
                        p.prop_type == PropType::CompactList && p.name == "tags"
                    }
                ))
            })
        {
            properties.iter_mut().for_each(|p| {
                if p.name == "tags" && p.prop_type == PropType::CompactList {
                    p.prop_type = PropType::List;
                }
            });
        };
        let new_text = pd.to_string(mode, &file_info);
        if old_text != new_text {
            if fix {
                let mut state = OverwriteConfirmState::new(
                    old_text,
                    new_text.clone(),
                    format!("Should the file {:?} be overwritten?", path),
                );
                ratatui::run(|terminal| state.run(terminal))?;
                if state.choice {
                    std::fs::write(path, new_text)?;
                } else {
                    *pd = old_pd;
                }
            } else {
                println!(
                    "\nFile {path:?} contains a compressed list in its frontmatter:\n{old_text}"
                );
            }
        }
        Ok(())
    });
    res
}

struct OverwriteConfirmState {
    old: String,
    new: String,
    info: String,
    choice: bool,
    exit: bool,
}

impl OverwriteConfirmState {
    fn new(old: String, new: String, info: String) -> Self {
        Self {
            old,
            new,
            info,
            choice: false,
            exit: false,
        }
    }
    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> Result<()> {
        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event);
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('y') => {
                self.choice = true;
                self.exit = true;
            }
            KeyCode::Char('n') => {
                self.choice = false;
                self.exit = true;
            }
            _ => {}
        }
    }
}

impl Widget for &OverwriteConfirmState {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let row_constrains = vec![
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(3),
        ];
        let vertical = Layout::vertical(row_constrains);
        let rows = vertical.split(area);

        // info (top)
        Paragraph::new(self.info.clone()).render(rows[0], buf);

        // old/new
        let col_constraints = vec![Constraint::Percentage(50), Constraint::Percentage(50)];
        let horizontal = Layout::horizontal(col_constraints);
        let cols = horizontal.split(rows[1]);
        Paragraph::new(self.old.clone())
            .block(Block::default().title_top("old").borders(Borders::ALL))
            .render(cols[0], buf);
        Paragraph::new(self.new.clone())
            .block(Block::default().title_top("new").borders(Borders::ALL))
            .render(cols[1], buf);

        // instructions
        Paragraph::new(" <y> overwrite | <n> keep old ")
            .block(Block::default().borders(Borders::ALL))
            .render(rows[2], buf);
    }
}

#[derive(Eq, PartialEq)]
enum SimilarNameResolution {
    Main,
    None,
    Ignore,
    Remove,
    Skip,
}

struct SimilarNamesState {
    file_paths: Vec<(PathBuf, SimilarNameResolution)>,
    position: usize,
    exit: bool,
}

impl SimilarNamesState {
    fn new(file_paths: Vec<(PathBuf, SimilarNameResolution)>) -> Self {
        let len = file_paths.len();
        Self {
            file_paths,
            position: 1.min(len.saturating_sub(1)),
            exit: false,
        }
    }
    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> Result<()> {
        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event);
            }
            _ => {}
        };
        Ok(())
    }

    fn change_resolution_for_current_position(&mut self, resolution: SimilarNameResolution) {
        self.change_resolution_for_position(resolution, self.position);
    }

    fn change_resolution_for_position(
        &mut self,
        resolution: SimilarNameResolution,
        position: usize,
    ) {
        if self.file_paths[position].1 != SimilarNameResolution::Main {
            let new_res = (self.file_paths[self.position].0.clone(), resolution);
            self.file_paths[position] = new_res;
        }
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        use SimilarNameResolution::*;
        match key_event.code {
            KeyCode::Char('s') => {
                self.change_resolution_for_current_position(Skip);
            }
            KeyCode::Char('i') => {
                self.change_resolution_for_current_position(Ignore);
            }
            KeyCode::Char('d') => {
                self.change_resolution_for_current_position(Remove);
            }
            KeyCode::Char('m') => {
                let main_pos = self
                    .file_paths
                    .iter()
                    .enumerate()
                    .find(|(_, (_, r))| matches!(*r, Main))
                    .unwrap()
                    .0;
                self.change_resolution_for_position(None, main_pos);
                self.change_resolution_for_current_position(Main);
            }
            KeyCode::Char('q') => {
                self.exit = true;
            }
            KeyCode::Up => {
                self.position = self.position.saturating_sub(1);
            }
            KeyCode::Down => {
                self.position = self
                    .position
                    .saturating_add(1)
                    .min(self.file_paths.len().saturating_sub(1));
            }
            _ => {}
        };
    }
}

impl Widget for &SimilarNamesState {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let row_constrains = vec![
            Constraint::Length(1),
            Constraint::Length(self.file_paths.len() as u16),
            Constraint::Min(10),
            Constraint::Length(3),
        ];
        let vertical = Layout::vertical(row_constrains);
        let rows = vertical.split(area);

        // info (top): row 0
        Paragraph::new("The following files have very similar file names:").render(rows[0], buf);

        // file path selection: row 1
        let lines: Vec<Line> = self
            .file_paths
            .iter()
            .enumerate()
            .map(|(i, (p, r))| {
                let resolution_text = match r {
                    SimilarNameResolution::Main => "<main>",
                    SimilarNameResolution::None => "",
                    SimilarNameResolution::Ignore => "<ignore>",
                    SimilarNameResolution::Remove => "<remove>",
                    SimilarNameResolution::Skip => "<skip>",
                };
                if i == self.position {
                    Line::from(vec![
                        resolution_text.yellow(),
                        " ".into(),
                        p.to_str().unwrap().bold().blue(),
                    ])
                } else {
                    Line::from(vec![
                        resolution_text.yellow(),
                        " ".into(),
                        p.to_str().unwrap().into(),
                    ])
                }
            })
            .collect();
        Paragraph::new(lines).render(rows[1], buf);

        // file preview: row 2
        let col_constraints = vec![Constraint::Percentage(50), Constraint::Percentage(50)];
        let horizontal = Layout::horizontal(col_constraints);
        let cols = horizontal.split(rows[2]);
        let main_text = std::fs::read_to_string(&self.file_paths[0].0).unwrap();
        let pos_text = std::fs::read_to_string(&self.file_paths[self.position].0).unwrap();
        Paragraph::new(main_text)
            .block(
                Block::default()
                    .title_top(format!("main ({:?})", self.file_paths[0].0))
                    .borders(Borders::ALL),
            )
            .render(cols[0], buf);
        Paragraph::new(pos_text)
            .block(
                Block::default()
                    .title_top(self.file_paths[self.position].0.to_str().unwrap())
                    .borders(Borders::ALL),
            )
            .render(cols[1], buf);

        // instructions
        Paragraph::new(" <s> skip | <i> ignore | <d> delete | <m> make main | <q> quit ")
            .block(Block::default().borders(Borders::ALL))
            .render(rows[3], buf);
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct IgnoredSimilarFiles {
    ignore_similar: Vec<(PathBuf, PathBuf)>,
}

impl IgnoredSimilarFiles {
    fn read() -> Self {
        let text = std::fs::read_to_string(IgnoredSimilarFiles::get_file_path()).context("");
        if let Ok(text) = text {
            toml::from_str(&text)
                .context("failed to parse toml!")
                .unwrap()
        } else {
            println!("failed to read");
            IgnoredSimilarFiles::default()
        }
    }

    fn write(&self) -> Result<()> {
        std::fs::write(IgnoredSimilarFiles::get_file_path(), toml::to_string(self)?)
            .context("Failed to write ignore similar toml")
    }

    fn get_file_path() -> PathBuf {
        let dirs = directories::ProjectDirs::from("TF", "TF", "pkmt").unwrap();
        dirs.config_local_dir().join("ignore_similar.toml")
    }

    fn contains(&self, a: &PathBuf, b: &PathBuf) -> bool {
        self.ignore_similar
            .iter()
            .any(|(x, y)| x == a && y == b || x == b && y == a)
    }

    fn ensure_contains(&mut self, a: &PathBuf, b: &PathBuf) {
        if !self.contains(a, b) {
            self.ignore_similar.push((a.clone(), b.clone()));
            self.write().unwrap();
        }
    }
}

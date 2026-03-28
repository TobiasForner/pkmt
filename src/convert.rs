use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::{
    parsing::{TextMode, parse_file},
    util::files_in_tree,
};

/// original_file is the file that was parsed
/// destination_file: the file the current contents will be written to. If this is set to 'None',
/// it is assumed that the original_file is the destination_file as well
/// image dirs: (original_image_dir, destination_image_dir)
#[derive(Clone, Debug)]
pub struct FileInfo {
    pub original_file: PathBuf,
    pub destination_file: Option<PathBuf>,
    pub image_dirs: Option<(PathBuf, PathBuf)>,
}

impl FileInfo {
    /// returns Some(original_file, destination_file, image_dir) if all are set and None otherwise.
    pub fn get_all(&self) -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
        if let (Some(dest), Some((image_in, image_out))) =
            (&self.destination_file, &self.image_dirs)
        {
            return Some((
                self.original_file.clone(),
                dest.clone(),
                image_in.clone(),
                image_out.clone(),
            ));
        }
        None
    }
    pub fn new_same_file(file: PathBuf) -> Self {
        Self {
            original_file: file,
            destination_file: None,
            image_dirs: None,
        }
    }

    pub fn new(
        original_file: PathBuf,
        destination_file: Option<PathBuf>,
        image_dirs: Option<(PathBuf, PathBuf)>,
    ) -> Self {
        Self {
            original_file,
            destination_file,
            image_dirs,
        }
    }
}

pub fn convert_tree(
    root_dir: PathBuf,
    target_dir: PathBuf,
    inmode: TextMode,
    outmode: TextMode,
    image_dirs: Option<(PathBuf, PathBuf)>,
) -> Result<Vec<String>> {
    let root_dir = root_dir.canonicalize()?;
    let files = files_in_tree(&root_dir, &Some(vec!["md"]))?;
    if !target_dir.exists() {
        std::fs::create_dir_all(&target_dir)?;
    }
    let target_dir = target_dir.canonicalize()?;

    let mentioned_files = files
        .iter()
        .map(|f| {
            let rel = pathdiff::diff_paths(f, &root_dir).unwrap();
            let target = target_dir.join(&rel);
            let file_info = FileInfo::new(f.clone(), Some(target), image_dirs.clone());
            convert_file(file_info, inmode.clone(), outmode.clone())
        })
        .collect::<Result<Vec<Vec<String>>>>();
    match mentioned_files {
        Ok(v) => Ok(v.into_iter().flat_map(|v| v.into_iter()).collect()),
        Err(e) => Err(e),
    }
}

pub fn convert_file(
    file_info: FileInfo,
    inmode: TextMode,
    outmode: TextMode,
) -> Result<Vec<String>> {
    let file = &file_info.original_file;
    let pd = parse_file(file, &inmode);

    if let Ok(pd) = pd {
        let mentioned_files = pd.mentioned_files();

        let text = pd.to_string(&outmode, &Some(file_info.clone()));
        let dest_file = file_info
            .destination_file
            .clone()
            .context(format!("No destination file: {file_info:?}"))?;

        let res =
            std::fs::write(&dest_file, text).context(format!("Failed to write to {dest_file:?}"));
        if res.is_err() {
            bail!("Encountered: {res:?}!");
        }
        Ok(mentioned_files)
    } else {
        bail!("Could not convert the file {file:?} to obsidian: {pd:?}")
    }
}

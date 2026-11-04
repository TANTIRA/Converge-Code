use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::git::Workspace;

pub struct WeightEdit {
    pub parent: String,
    pub child: String,
    pub parent_classification: String,
    pub child_classification: String,
    pub uri: String,
}

pub fn promote(home: &Path, workspace: &Workspace, edit: &WeightEdit) -> Result<PathBuf> {
    if !workspace.toplevel.join("weights").is_dir() {
        return Err(Error::new("this repo has no weights"));
    }
    if rank(&edit.child_classification) > rank(&edit.parent_classification) {
        return Err(Error::new(
            "classification check failed: the child is broader than the parent",
        ));
    }
    let dir = home
        .join("projects")
        .join(&workspace.repo_key)
        .join("weights");
    fs::create_dir_all(&dir)
        .map_err(|error| Error::new(format!("create {}: {error}", dir.display())))?;
    let path = dir.join(format!("{}.json", Uuid::new_v4()));
    let record = json!({
        "parent": edit.parent,
        "child": edit.child,
        "classification_check": "passed",
        "uri": edit.uri,
        "lineage": {
            "parent": edit.parent,
            "child": edit.child,
        },
        "edit_status": "promoted",
    });
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| Error::new(format!("encode weight edit: {error}")))?;
    fs::write(&path, bytes)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    Ok(path)
}

pub fn abort(path: &Path) -> Result<()> {
    let bytes =
        fs::read(path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let mut record: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    record["edit_status"] = json!("aborted");
    let encoded = serde_json::to_vec_pretty(&record)
        .map_err(|error| Error::new(format!("encode weight edit: {error}")))?;
    fs::write(path, encoded)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

fn rank(classification: &str) -> i32 {
    match classification {
        "restricted" => 0,
        "internal" => 1,
        "public" => 2,
        _ => 1,
    }
}

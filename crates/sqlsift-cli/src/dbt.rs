//! dbt project detection for query files

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Directories of a dbt project that hold no models to check: installed packages,
/// compiled output and macros. Query files in them that were matched by a glob
/// pattern (not named on their own) are skipped when Jinja templating is on.
pub const SKIPPED_DIRS: &[&str] = &["dbt_packages", "target", "macros"];

/// Finds the dbt project (the nearest directory with a `dbt_project.yml`) of query
/// files, caching the answer per directory
#[derive(Default)]
pub struct DbtProjects {
    roots: HashMap<PathBuf, Option<PathBuf>>,
}

impl DbtProjects {
    /// The root of the dbt project `file` is in: the nearest of its ancestor
    /// directories that holds a `dbt_project.yml`
    pub fn root_of(&mut self, file: &Path) -> Option<PathBuf> {
        let file = std::env::current_dir().ok()?.join(file);
        let dir = file.parent()?;
        if let Some(root) = self.roots.get(dir) {
            return root.clone();
        }
        let root = dir
            .ancestors()
            .find(|d| d.join("dbt_project.yml").is_file())
            .map(Path::to_path_buf);
        self.roots.insert(dir.to_path_buf(), root.clone());
        root
    }

    /// Whether `file` is in one of the [`SKIPPED_DIRS`] of its dbt project
    pub fn is_skipped(&mut self, file: &Path) -> bool {
        let Some(root) = self.root_of(file) else {
            return false;
        };
        let Ok(file) = std::env::current_dir().map(|cwd| cwd.join(file)) else {
            return false;
        };
        file.strip_prefix(&root)
            .ok()
            .and_then(|relative| relative.components().next())
            .and_then(|first| first.as_os_str().to_str())
            .is_some_and(|first| SKIPPED_DIRS.contains(&first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_nearest_project_and_skipped_dirs() {
        let dir = std::env::temp_dir().join(format!("sqlsift-dbt-{}", std::process::id()));
        let project = dir.join("analytics");
        std::fs::create_dir_all(project.join("models/staging")).unwrap();
        std::fs::write(project.join("dbt_project.yml"), "name: a\n").unwrap();

        let mut projects = DbtProjects::default();
        let model = project.join("models/staging/stg.sql");
        assert_eq!(projects.root_of(&model), Some(project.clone()));
        assert!(!projects.is_skipped(&model));
        assert!(projects.is_skipped(&project.join("macros/m.sql")));
        assert!(projects.is_skipped(&project.join("dbt_packages/p/models/x.sql")));
        assert!(projects.is_skipped(&project.join("target/compiled/x.sql")));
        assert_eq!(projects.root_of(&dir.join("other.sql")), None);
        assert!(!projects.is_skipped(&dir.join("macros/m.sql")));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

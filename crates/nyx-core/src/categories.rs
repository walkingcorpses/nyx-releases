// categories.rs - named categories, each with a default save folder. Pure logic.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One category as stored in settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryDef {
    pub name: String,
    pub save_dir: PathBuf,
}

/// The user's category list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Categories {
    defs: Vec<CategoryDef>,
}

impl Categories {
    /// Builds the list from stored settings.
    pub fn from_defs(defs: Vec<CategoryDef>) -> Categories {
        Categories { defs }
    }

    /// The list in a form that can be saved back to settings.
    pub fn defs(&self) -> &[CategoryDef] {
        &self.defs
    }

    /// Adds a category, or changes its folder if the name already exists.
    pub fn add(&mut self, name: &str, save_dir: &Path) {
        match self.defs.iter_mut().find(|d| d.name == name) {
            Some(d) => d.save_dir = save_dir.to_path_buf(),
            None => self.defs.push(CategoryDef { name: name.to_string(), save_dir: save_dir.to_path_buf() }),
        }
    }

    /// Removes a category. Torrents that used it fall back to the default folder.
    pub fn remove(&mut self, name: &str) {
        self.defs.retain(|d| d.name != name);
    }

    /// Where a torrent in `category` should save: the category's own folder, else `default`.
    pub fn save_dir_for(&self, category: Option<&str>, default: &Path) -> PathBuf {
        category
            .and_then(|c| self.defs.iter().find(|d| d.name == c))
            .map(|d| d.save_dir.clone())
            .unwrap_or_else(|| default.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_uses_own_dir_else_default() {
        let mut c = Categories::default();
        c.add("movies", Path::new("D:/movies"));
        let def = Path::new("D:/dl");
        assert_eq!(c.save_dir_for(Some("movies"), def), PathBuf::from("D:/movies"));
        assert_eq!(c.save_dir_for(Some("nope"), def), PathBuf::from("D:/dl"));
        assert_eq!(c.save_dir_for(None, def), PathBuf::from("D:/dl"));
    }

    #[test]
    fn category_remove_falls_back_to_default() {
        let mut c = Categories::default();
        c.add("movies", Path::new("D:/movies"));
        c.remove("movies");
        assert_eq!(c.save_dir_for(Some("movies"), Path::new("D:/dl")), PathBuf::from("D:/dl"));
    }
}

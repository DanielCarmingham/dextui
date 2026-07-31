//! Registered repositories and their worktrees, flattened for rendering.
//!
//! Mirrors `tree::visible_rows`: a flat list of rows with enough identity to
//! address the thing each one draws, so selection and clicking work the same way
//! they already do in the task tree.

use crate::worktree::Worktree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub path: String,
    pub worktrees: Vec<Worktree>,
    pub open: bool,
    /// Whether this repo is in `repos.toml`.
    ///
    /// The sidebar always carries the store the app is actually reading, even
    /// when nobody has registered it -- an empty pane beside a full task tree
    /// reads as "no repos" while you are plainly looking at one. So `a` means
    /// "keep this one" rather than "make this appear", and this is the flag
    /// that tells the two apart.
    pub registered: bool,
    /// The global store, which is what dex falls back to outside a git repo.
    ///
    /// It is not a repo at all: no worktrees, and `path` is the store itself
    /// rather than a checkout with a `.dex` inside it -- hence [`Repo::store`]
    /// rather than [`store_dir`] at every call site that resolves a row.
    pub is_global: bool,
}

impl Repo {
    /// The dex store behind one of this repo's rows: `worktree` for a worktree
    /// row, `None` for the repo's own row.
    pub fn store(&self, worktree: Option<&Worktree>) -> String {
        if self.is_global {
            return self.path.clone();
        }
        store_dir(worktree.map_or(self.path.as_str(), |w| w.path.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Repo { index: usize },
    Worktree { repo: usize, index: usize },
}

/// Every visible row, top to bottom.
pub fn rows(repos: &[Repo]) -> Vec<Row> {
    let mut out = Vec::new();
    for (i, r) in repos.iter().enumerate() {
        out.push(Row::Repo { index: i });
        if r.open {
            for (j, _) in r.worktrees.iter().enumerate() {
                out.push(Row::Worktree { repo: i, index: j });
            }
        }
    }
    out
}

/// The dex store for a worktree.
pub fn store_dir(worktree_path: &str) -> String {
    format!("{}/.dex", worktree_path.trim_end_matches('/'))
}

/// Whether a worktree has a store yet. A plain on-disk check, deliberately not a
/// dex call: this runs for every row, and a worktree without tasks is an
/// ordinary row rather than an error.
pub fn has_store(worktree_path: &str) -> bool {
    std::path::Path::new(&store_dir(worktree_path)).is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: &str, main: bool) -> Worktree {
        Worktree {
            path: path.to_string(),
            branch: branch.to_string(),
            is_main: main,
            is_locked: false,
            is_detached: false,
        }
    }

    fn repo(name: &str, open: bool) -> Repo {
        Repo {
            name: name.to_string(),
            path: format!("/x/{name}"),
            worktrees: vec![
                wt(&format!("/x/{name}"), "main", true),
                wt(&format!("/x/{name}-feat"), "feat", false),
            ],
            open,
            registered: true,
            is_global: false,
        }
    }

    #[test]
    fn an_open_repo_lists_its_worktrees_beneath_it() {
        let rs = vec![repo("one", true)];
        assert_eq!(
            rows(&rs),
            vec![
                Row::Repo { index: 0 },
                Row::Worktree { repo: 0, index: 0 },
                Row::Worktree { repo: 0, index: 1 },
            ]
        );
    }

    #[test]
    fn a_closed_repo_hides_its_worktrees() {
        let rs = vec![repo("one", false)];
        assert_eq!(rows(&rs), vec![Row::Repo { index: 0 }]);
    }

    #[test]
    fn repos_keep_their_order_and_do_not_interleave() {
        let rs = vec![repo("a", true), repo("b", false), repo("c", true)];
        let r = rows(&rs);
        assert_eq!(r[0], Row::Repo { index: 0 });
        assert_eq!(r[3], Row::Repo { index: 1 });
        assert_eq!(r[4], Row::Repo { index: 2 });
    }

    /// dex stores live in `.dex` under the worktree, and this is the one place
    /// that knows it -- `Dex::for_store` rejects anything else.
    #[test]
    fn a_store_is_the_dex_directory_under_the_worktree() {
        assert_eq!(store_dir("/x/one"), "/x/one/.dex");
        assert_eq!(store_dir("/x/one/"), "/x/one/.dex");
    }

    #[test]
    fn a_repos_rows_resolve_to_the_dex_directory_under_each_worktree() {
        let r = repo("one", true);
        assert_eq!(r.store(None), "/x/one/.dex");
        assert_eq!(r.store(Some(&r.worktrees[1])), "/x/one-feat/.dex");
    }

    /// The global store is the exception the whole `Repo::store` indirection
    /// exists for: dex's out-of-repo fallback is the store directory itself,
    /// so deriving `<path>/.dex` from it points at nothing -- and dex reports
    /// a store that does not exist as an *empty project*, never as an error.
    #[test]
    fn the_global_store_is_its_own_path_not_a_dex_directory_beneath_it() {
        let g = Repo {
            name: "global".into(),
            path: "/home/u/.config/dex/local".into(),
            worktrees: vec![],
            open: true,
            registered: false,
            is_global: true,
        };
        assert_eq!(g.store(None), "/home/u/.config/dex/local");
        assert_eq!(rows(&[g]), vec![Row::Repo { index: 0 }], "no worktree rows");
    }
}

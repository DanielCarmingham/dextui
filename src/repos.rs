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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Repo { index: usize },
    Worktree { repo: usize, index: usize },
}

/// Every visible row, top to bottom.
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn store_dir(worktree_path: &str) -> String {
    format!("{}/.dex", worktree_path.trim_end_matches('/'))
}

/// Whether a worktree has a store yet. A plain on-disk check, deliberately not a
/// dex call: this runs for every row, and a worktree without tasks is an
/// ordinary row rather than an error.
#[allow(dead_code)]
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
}

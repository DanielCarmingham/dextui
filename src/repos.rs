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
    /// Whether this repo is saved in `repos.toml`.
    ///
    /// Decides which *section* it appears under rather than whether it appears
    /// at all: the repo you are in is always on screen, under `here`, saved or
    /// not. Nothing marks the distinction on the row itself -- the heading
    /// above it already does, which is the whole point of having headings.
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
    /// A section label. Not selectable, and carries no store.
    Heading(&'static str),
    Repo { index: usize },
    Worktree { repo: usize, index: usize },
}

impl Row {
    /// Whether the cursor can rest here. Headings are labels, not places.
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Heading(_))
    }
}

/// Every visible row, top to bottom, split into "here" and "saved".
///
/// The two answer different questions and needed separating rather than
/// blending: *where I am* is told by the working directory and needs no
/// memory at all, while *where else I can go* is precisely a remembered set.
/// Showing the first inside the second forced the current repo to be either a
/// member of a list it had not joined, or a member with an asterisk -- an
/// implicit registration or a cryptic marker. Naming the two sections costs a
/// row each and removes the concept entirely.
///
/// A repo that is both current *and* saved appears once, under `here`: it is
/// where you are, which is the more useful thing to say about it.
///
/// Headings appear only when both sections have something in them. With one
/// repo there is nothing to distinguish, and a lone `here` over a single entry
/// is a label explaining itself.
pub fn rows(repos: &[Repo], here: Option<usize>) -> Vec<Row> {
    let saved: Vec<usize> = (0..repos.len())
        .filter(|i| Some(*i) != here && repos[*i].registered)
        .collect();
    let label = here.is_some() && !saved.is_empty();

    let mut out = Vec::new();
    let push_repo = |out: &mut Vec<Row>, i: usize| {
        out.push(Row::Repo { index: i });
        if repos[i].open {
            for (j, _) in repos[i].worktrees.iter().enumerate() {
                out.push(Row::Worktree { repo: i, index: j });
            }
        }
    };

    if let Some(i) = here {
        if label {
            out.push(Row::Heading("here"));
        }
        push_repo(&mut out, i);
    }
    if label {
        out.push(Row::Heading("saved"));
    }
    for i in saved {
        push_repo(&mut out, i);
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

    /// Both sections present, so both are labelled.
    #[test]
    fn here_and_saved_are_labelled_when_both_have_something_in_them() {
        let rs = vec![repo("a", true), repo("b", true)];
        let r = rows(&rs, Some(1));
        assert_eq!(r[0], Row::Heading("here"));
        assert_eq!(r[1], Row::Repo { index: 1 }, "the current repo leads");
        assert_eq!(r[4], Row::Heading("saved"));
        assert_eq!(r[5], Row::Repo { index: 0 });
    }

    /// One section is no distinction, and a lone `here` over a single entry is
    /// a label explaining itself.
    #[test]
    fn a_single_section_gets_no_heading() {
        let rs = vec![repo("a", true)];
        assert!(
            rows(&rs, Some(0)).iter().all(|r| r.selectable()),
            "a lone section should not be labelled"
        );
        assert!(
            rows(&rs, None).iter().all(|r| r.selectable()),
            "saved-only with nothing current should not be labelled either"
        );
    }

    /// Current *and* saved appears once, under `here` -- it is where you are,
    /// which is the more useful thing to say about it.
    #[test]
    fn a_repo_that_is_both_current_and_saved_is_not_listed_twice() {
        let rs = vec![repo("a", true), repo("b", true)];
        let r = rows(&rs, Some(0));
        let firsts: Vec<_> = r.iter().filter(|x| matches!(x, Row::Repo { .. })).collect();
        assert_eq!(firsts.len(), 2, "each repo exactly once: {r:?}");
    }

    /// An unsaved repo you are not in has no section to be in, so it does not
    /// appear at all -- `here` is the only thing that puts an unsaved repo on
    /// screen.
    #[test]
    fn an_unsaved_repo_you_are_not_in_is_not_shown() {
        let mut rs = vec![repo("a", true), repo("b", true)];
        rs[1].registered = false;
        let r = rows(&rs, Some(0));
        assert!(
            !r.contains(&Row::Repo { index: 1 }),
            "an unsaved repo appeared without being current: {r:?}"
        );
    }

    #[test]
    fn an_open_repo_lists_its_worktrees_beneath_it() {
        let rs = vec![repo("one", true)];
        assert_eq!(
            rows(&rs, None),
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
        assert_eq!(rows(&rs, None), vec![Row::Repo { index: 0 }]);
    }

    #[test]
    fn repos_keep_their_order_and_do_not_interleave() {
        let rs = vec![repo("a", true), repo("b", false), repo("c", true)];
        let r = rows(&rs, None);
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
        assert_eq!(rows(&[g], Some(0)), vec![Row::Repo { index: 0 }], "no worktree rows");
    }
}

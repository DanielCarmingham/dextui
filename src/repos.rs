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
    /// at all: the repo you are in is always on screen, saved or not. Nothing
    /// marks the distinction on the row itself -- the heading above it already
    /// does, which is the whole point of having headings.
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
    /// What an empty section says instead of nothing. Not selectable either.
    Hint(&'static str),
    Repo { index: usize },
    Worktree { repo: usize, index: usize },
}

impl Row {
    /// Whether the cursor can rest here. Labels are not places.
    ///
    /// Written as a positive match rather than `!matches!(Heading)`, so a
    /// fourth kind of label defaults to *not* selectable -- the safe way
    /// round, since a cursor that can land on a label resolves to no store.
    pub fn selectable(&self) -> bool {
        matches!(self, Row::Repo { .. } | Row::Worktree { .. })
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
/// **`here` holds the launch repo only while it is unsaved**, and saving moves
/// it down into `saved`. That move is the entire visible answer to "did `a`
/// work?", and it is why the rule is this way round rather than the other.
/// Keeping a saved repo under `here` -- on the grounds that where you are is
/// the more useful thing to say about it -- made `a` change *nothing at all*
/// on screen: the row it marked was already drawn, in the same place, with no
/// marker on it by deliberate policy. The registry was written, the status bar
/// said so for one keystroke, and the pane that exists to show the saved set
/// looked identical before and after. So `here` now means "you are in this,
/// and it is not in your list yet", which is the one state worth a section of
/// its own.
///
/// The **`saved` heading is drawn even when the section is empty**, with a
/// hint under it. It is where `a` puts things, so it has to be visible
/// *before* the press for the move to read as a move afterwards -- a
/// destination that materialises along with its first arrival is a layout
/// change, not a confirmation.
///
/// The `here` heading, by contrast, appears only when that section exists:
/// there is nothing useful to say under an empty one, and an empty `here`
/// while you are plainly somewhere reads as the app having lost you.
pub fn rows(repos: &[Repo], here: Option<usize>) -> Vec<Row> {
    let here = here.filter(|i| !repos[*i].registered);
    // Every saved repo, including the one you are in -- which is exactly what
    // `here` above has just stopped claiming.
    let saved: Vec<usize> = (0..repos.len()).filter(|i| repos[*i].registered).collect();

    let mut out = Vec::new();
    // Nothing at all: no headings over an empty pane, and in particular no
    // "nothing saved yet" hint in a run that has no sidebar content to hint
    // about.
    if here.is_none() && saved.is_empty() {
        return out;
    }

    let push_repo = |out: &mut Vec<Row>, i: usize| {
        out.push(Row::Repo { index: i });
        if repos[i].open {
            for (j, _) in repos[i].worktrees.iter().enumerate() {
                out.push(Row::Worktree { repo: i, index: j });
            }
        }
    };

    if let Some(i) = here {
        out.push(Row::Heading("here"));
        push_repo(&mut out, i);
    }
    out.push(Row::Heading("saved"));
    if saved.is_empty() {
        out.push(Row::Hint("nothing saved yet"));
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

    /// Both sections present, so both are labelled. `b` is where we are and
    /// is not saved yet, so it leads under `here`.
    #[test]
    fn here_and_saved_are_labelled_when_both_have_something_in_them() {
        let mut rs = vec![repo("a", true), repo("b", true)];
        rs[1].registered = false;
        let r = rows(&rs, Some(1));
        assert_eq!(r[0], Row::Heading("here"));
        assert_eq!(r[1], Row::Repo { index: 1 }, "the current repo leads");
        assert_eq!(r[4], Row::Heading("saved"));
        assert_eq!(r[5], Row::Repo { index: 0 });
    }

    /// The whole point of the rule: `a` has to move the row, because moving it
    /// is the only thing on screen that says the press did anything. Before,
    /// the current repo stayed under `here` and the pane was byte-identical
    /// either side of a successful registration.
    #[test]
    fn saving_the_repo_you_are_in_moves_it_from_here_to_saved() {
        let mut rs = vec![repo("a", true)];
        rs[0].registered = false;

        let before = rows(&rs, Some(0));
        assert_eq!(before[0], Row::Heading("here"));
        assert_eq!(before[1], Row::Repo { index: 0 });
        assert!(
            before.contains(&Row::Hint("nothing saved yet")),
            "the destination has to be on screen before the press: {before:?}"
        );

        rs[0].registered = true;
        let after = rows(&rs, Some(0));
        assert!(
            !after.contains(&Row::Heading("here")),
            "`here` should have emptied and gone: {after:?}"
        );
        assert_eq!(after[0], Row::Heading("saved"));
        assert_eq!(after[1], Row::Repo { index: 0 });
        assert_ne!(before, after, "registering must change what is drawn");
    }

    /// A lone `saved` section keeps its heading, so the repo that has just
    /// arrived under it is visibly *under something*. `here` gets no such
    /// treatment -- see `rows`.
    #[test]
    fn saved_keeps_its_heading_alone_but_here_does_not_appear_empty() {
        let rs = vec![repo("a", true)];
        let r = rows(&rs, Some(0));
        assert_eq!(r[0], Row::Heading("saved"));
        assert!(!r.contains(&Row::Heading("here")));
    }

    /// Nothing registered and nowhere to be: an empty pane, not a pair of
    /// headings over a hint about a feature there is no repo to use it on.
    #[test]
    fn an_empty_sidebar_draws_nothing_at_all() {
        assert_eq!(rows(&[], None), vec![]);
        let mut rs = vec![repo("a", true)];
        rs[0].registered = false;
        assert_eq!(rows(&rs, None), vec![], "an unsaved repo you are not in");
    }

    /// Current *and* saved appears once, under `saved`.
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
                Row::Heading("saved"),
                Row::Repo { index: 0 },
                Row::Worktree { repo: 0, index: 0 },
                Row::Worktree { repo: 0, index: 1 },
            ]
        );
    }

    #[test]
    fn a_closed_repo_hides_its_worktrees() {
        let rs = vec![repo("one", false)];
        assert_eq!(
            rows(&rs, None),
            vec![Row::Heading("saved"), Row::Repo { index: 0 }]
        );
    }

    #[test]
    fn repos_keep_their_order_and_do_not_interleave() {
        let rs = vec![repo("a", true), repo("b", false), repo("c", true)];
        let r = rows(&rs, None);
        assert_eq!(r[1], Row::Repo { index: 0 });
        assert_eq!(r[4], Row::Repo { index: 1 });
        assert_eq!(r[5], Row::Repo { index: 2 });
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
        let r = rows(&[g], Some(0));
        assert!(
            !r.iter().any(|x| matches!(x, Row::Worktree { .. })),
            "the global store has no worktrees: {r:?}"
        );
    }
}

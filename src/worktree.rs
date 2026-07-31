//! Git worktrees for a repository. No dex, no UI -- just `git worktree list`.

use std::process::Command;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    /// The branch name with `refs/heads/` stripped, or the short SHA when
    /// detached. Never empty, so a row always has something to show.
    pub branch: String,
    /// The main checkout, which porcelain always lists first.
    pub is_main: bool,
    pub is_locked: bool,
    pub is_detached: bool,
}

/// Parses `git worktree list --porcelain`.
///
/// The format is stanzas separated by blank lines, each starting with a
/// `worktree <path>` line. Attributes that follow are one per line and may be
/// absent -- `branch` is missing entirely when detached, and `locked` appears
/// only when set. Silently ignores `bare` and `prunable` attributes, which do
/// not map to fields in this task.
#[allow(dead_code)]
pub fn parse(porcelain: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;
    let mut head = String::new();

    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(w) = current.take() {
                out.push(w);
            }
            head.clear();
            current = Some(Worktree {
                path: path.to_string(),
                branch: String::new(),
                is_main: out.is_empty(),
                is_locked: false,
                is_detached: false,
            });
        } else if let Some(sha) = line.strip_prefix("HEAD ") {
            head = sha.to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(w) = current.as_mut() {
                w.branch = b.trim_start_matches("refs/heads/").to_string();
            }
        } else if line == "detached" {
            if let Some(w) = current.as_mut() {
                w.is_detached = true;
                // No branch line follows, and a blank row is useless.
                w.branch = head.chars().take(7).collect();
            }
        } else if line == "locked" || line.starts_with("locked ") {
            if let Some(w) = current.as_mut() {
                w.is_locked = true;
            }
        }
    }
    if let Some(w) = current.take() {
        out.push(w);
    }
    out
}

/// Every worktree of `repo_path`, main checkout first.
#[allow(dead_code)]
pub fn list(repo_path: &str) -> Result<Vec<Worktree>, String> {
    let out = Command::new("git")
        .args(["-C", repo_path, "worktree", "list", "--porcelain"])
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;

    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `git worktree list --porcelain` on a real repo with linked
    /// worktrees, all locked -- which is the common case here and was nearly
    /// missed.
    const PORCELAIN: &str = "\
worktree /Users/x/Developer/TaxCommHub
HEAD edaad18c1111111111111111111111111111111
branch refs/heads/main

worktree /Users/x/Developer/TaxCommHub-561
HEAD 65416862222222222222222222222222222222b
branch refs/heads/561-enrollment-window-support
locked

worktree /Users/x/Developer/TaxCommHub-detached
HEAD 06dd22433333333333333333333333333333333
detached

worktree /Users/x/Developer/TaxCommHub-email
HEAD 5db559e64444444444444444444444444444444
branch refs/heads/email-project-prototype
locked
";

    #[test]
    fn the_first_worktree_is_the_main_checkout() {
        let w = parse(PORCELAIN);
        assert_eq!(w.len(), 4);
        assert!(w[0].is_main, "porcelain lists the main checkout first");
        assert!(!w[1].is_main);
    }

    #[test]
    fn branch_names_lose_their_refs_prefix() {
        let w = parse(PORCELAIN);
        assert_eq!(w[0].branch, "main");
        assert_eq!(w[1].branch, "561-enrollment-window-support");
    }

    /// All the real worktrees here are locked, so dropping this attribute would
    /// have looked fine on a toy repo and wrong on every real one.
    #[test]
    fn locked_worktrees_are_marked_not_skipped() {
        let w = parse(PORCELAIN);
        assert!(w[1].is_locked);
        assert!(!w[0].is_locked, "the main checkout is not locked");
        assert_eq!(w.len(), 4, "a locked worktree is still a worktree");
    }

    /// A detached worktree has no branch line at all. Left empty the row would
    /// render blank, so it falls back to the short SHA.
    #[test]
    fn a_detached_worktree_shows_its_sha_rather_than_nothing() {
        let w = parse(PORCELAIN);
        assert!(w[2].is_detached);
        assert_eq!(w[2].branch, "06dd224");
        assert!(!w[2].branch.is_empty());
    }

    #[test]
    fn empty_input_is_no_worktrees_not_a_panic() {
        assert_eq!(parse(""), vec![]);
        assert_eq!(parse("\n\n"), vec![]);
    }
}

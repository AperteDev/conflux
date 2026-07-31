//! Keeping a git *working clone* in step with its own remote.
//!
//! This is the sibling of [`crate::engine`], for sync groups that set
//! [`Sync::git_repo`](crate::config::Sync::git_repo). The engine reconciles two
//! file trees through a content index; here there is only one tree — the clone's
//! working directory — and git's own history is the source of truth. So instead of
//! per-file push/pull decisions, a run is the automation of what a person would
//! type:
//!
//! 1. stage everything and commit, if the working tree has changes;
//! 2. fetch the upstream branch;
//! 3. integrate — fast-forward when the local branch has nothing of its own,
//!    otherwise rebase the local commits on top of upstream;
//! 4. push, if the branch is ahead.
//!
//! Deliberate consequences of doing it this way: history is preserved (nothing is
//! ever hard-reset), `.gitignore` decides what is tracked, and a genuine conflict
//! is *not* resolved by conflux — the rebase is aborted, leaving the repository
//! exactly as it was, and the run fails with a message telling the user to sort it
//! out by hand. Silently picking a winner by timestamp, as the file engine does,
//! would mean discarding a commit.
//!
//! A pull-only run (the periodic `pull_interval` refresh) does steps 2 and 3 only,
//! so a background fetch never commits or pushes work in progress.

use crate::backend::git::GitAuth;
use crate::config::Sync;
use crate::engine::{group_label, PlanOp, PlannedChange, SyncPlan, SyncReport};
use crate::error::{Error, Result};
use crate::relpath::RelPath;
use git2::build::CheckoutBuilder;
use git2::{
    AnnotatedCommit, AutotagOption, Branch, Delta, Diff, DiffOptions, FetchOptions, IndexAddOption,
    PushOptions, Reference, Repository, RepositoryState, Signature,
};
use std::path::Path;
use tracing::debug;

/// Sync one git working clone: commit, fetch, integrate, push.
///
/// With `remote_refresh` set — a periodic `pull_interval` poll — only the fetch and
/// integrate steps run, leaving local changes for the group's own trigger.
pub fn run(sync: &Sync, remote_refresh: bool) -> Result<SyncReport> {
    let repo = open(sync)?;
    let mut report = SyncReport::default();

    if !remote_refresh {
        commit_local_changes(sync, &repo, &mut report)?;
    }

    // Every step re-resolves the branch and its upstream from the repository:
    // git2 references carry the object id they were read with, so one captured
    // before the fetch would still name the pre-fetch commit.
    let branch = branch_name(&current_branch(&repo)?)?;
    fetch(sync, &repo, &branch)?;
    integrate(sync, &repo, &branch, &mut report)?;

    if !remote_refresh {
        push(sync, &repo, &branch)?;
    }
    Ok(report)
}

/// Read-only companion to [`run`]: what the next sync would do.
///
/// It fetches (which changes no branch and no file) so the preview reflects the
/// current upstream, then reports the working-tree changes it would commit and
/// push, and the upstream changes it would bring down. A path listed in both
/// directions is one the rebase may conflict on.
pub fn plan(sync: &Sync) -> Result<SyncPlan> {
    let repo = open(sync)?;
    let branch = branch_name(&current_branch(&repo)?)?;
    fetch(sync, &repo, &branch)?;

    let mut plan = SyncPlan::default();

    // What a commit would capture: the working tree against HEAD.
    let staged = repo
        .diff_tree_to_workdir_with_index(head_tree(&repo)?.as_ref(), Some(&mut diff_options()))
        .map_err(gerr("diff working tree"))?;
    push_changes(&staged, &mut plan, Direction::Push);

    // What integrating would bring down: the freshly-fetched upstream against HEAD.
    let upstream_tree = upstream_of(&repo, &branch)?
        .get()
        .peel_to_tree()
        .map_err(gerr("read upstream tree"))?;
    let incoming = repo
        .diff_tree_to_tree(
            head_tree(&repo)?.as_ref(),
            Some(&upstream_tree),
            Some(&mut diff_options()),
        )
        .map_err(gerr("diff upstream"))?;
    push_changes(&incoming, &mut plan, Direction::Pull);

    Ok(plan)
}

/// Open the clone, refusing to touch a repository that is not in a state where
/// automated commits and rebases are safe.
fn open(sync: &Sync) -> Result<Repository> {
    let path = &sync.local;
    let repo = Repository::open(path).map_err(|_| {
        // libgit2's own message only restates the path, so it adds nothing here.
        let what = if path.exists() {
            "is not a git repository"
        } else {
            "does not exist"
        };
        Error::Precondition(format!(
            "`git_repo` points at `{}`, which {what}: it expects a working clone, as `git clone` \
             leaves behind. Clone it first, or use a `[[remote]]` + `local` sync if you meant to \
             sync files into a plain directory.",
            path.display()
        ))
    })?;
    if repo.is_bare() {
        return Err(Error::Precondition(format!(
            "`{}` is a bare repository, which has no working tree to sync",
            path.display()
        )));
    }
    // Mid-merge, mid-rebase, mid-cherry-pick: the user is in the middle of
    // something and committing everything would bury it.
    if repo.state() != RepositoryState::Clean {
        return Err(Error::Precondition(format!(
            "`{}` is in the middle of a {:?} (see `git status`); conflux will not sync it until \
             the repository is back in a clean state",
            path.display(),
            repo.state(),
        )));
    }
    Ok(repo)
}

/// The checked-out branch. A detached HEAD has no branch to push or rebase, and
/// guessing one would be worse than stopping.
fn current_branch<'r>(repo: &'r Repository) -> Result<Branch<'r>> {
    let head = repo.head().map_err(|e| {
        Error::Precondition(format!(
            "cannot read HEAD (a repository with no commits yet cannot be synced): {e}"
        ))
    })?;
    if !head.is_branch() {
        return Err(Error::Precondition(
            "HEAD is detached, so there is no branch to sync; check out a branch first".to_string(),
        ));
    }
    let name = head
        .shorthand()
        .ok_or_else(|| Error::Backend("HEAD has no branch name".to_string()))?;
    repo.find_branch(name, git2::BranchType::Local)
        .map_err(gerr("find current branch"))
}

/// The branch's configured upstream (`branch.<name>.remote`/`merge`, i.e. what
/// `git push`/`git pull` would use with no arguments), read fresh from `repo`.
fn upstream_of<'r>(repo: &'r Repository, branch: &str) -> Result<Branch<'r>> {
    let local = repo
        .find_branch(branch, git2::BranchType::Local)
        .map_err(gerr("find branch"))?;
    local.upstream().map_err(|e| no_upstream(branch, &e))
}

/// The error for a branch that tracks nothing. Both the upstream lookup and the
/// remote lookup can be the first to notice, and the advice is the same either way.
fn no_upstream(branch: &str, e: &git2::Error) -> Error {
    Error::Precondition(format!(
        "branch `{branch}` has no upstream to sync with, so conflux has nothing to fetch from or \
         push to; set one with `git push -u origin {branch}` ({})",
        e.message(),
    ))
}

/// The current object id of a local or remote-tracking branch, read fresh.
fn branch_oid(repo: &Repository, branch: &Branch<'_>) -> Result<git2::Oid> {
    let refname = ref_name(branch)?;
    repo.refname_to_id(&refname)
        .map_err(gerr("resolve branch reference"))
}

/// A branch's full reference name (`refs/heads/main`, `refs/remotes/origin/main`).
fn ref_name(branch: &Branch<'_>) -> Result<String> {
    branch
        .get()
        .name()
        .map(str::to_string)
        .ok_or_else(|| Error::Backend("branch reference name is not valid UTF-8".to_string()))
}

fn branch_name(branch: &Branch<'_>) -> Result<String> {
    branch
        .name()
        .map_err(gerr("read branch name"))?
        .map(str::to_string)
        .ok_or_else(|| Error::Backend("branch name is not valid UTF-8".to_string()))
}

/// The remote a branch tracks (`branch.<name>.remote`, defaulting to `origin`).
fn remote_name(repo: &Repository, branch: &str) -> Result<String> {
    let refname = format!("refs/heads/{branch}");
    let buf = repo
        .branch_upstream_remote(&refname)
        // Absent `branch.<name>.remote` means the branch tracks nothing, which this
        // lookup can reach before the upstream one does.
        .map_err(|e| no_upstream(branch, &e))?;
    Ok(buf.as_str().unwrap_or("origin").to_string())
}

/// Stage every change (honouring `.gitignore`, like `git add -A`) and commit it.
///
/// The commit is authored with the repository's own `user.name`/`user.email` when
/// configured, so conflux's commits look like the user's; only the fallback is
/// conflux's own identity.
fn commit_local_changes(sync: &Sync, repo: &Repository, report: &mut SyncReport) -> Result<()> {
    let mut index = repo.index().map_err(gerr("open index"))?;
    // `add_all` covers new and modified files; `update_all` covers removals of
    // tracked ones. Together they are `git add -A`, and both skip ignored files.
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(gerr("stage changes"))?;
    index
        .update_all(["*"].iter(), None)
        .map_err(gerr("stage removals"))?;
    index.write().map_err(gerr("write index"))?;
    let tree_oid = index.write_tree().map_err(gerr("write tree"))?;

    let head_tree = head_tree(repo)?;
    if head_tree.as_ref().map(|t| t.id()) == Some(tree_oid) {
        debug!(group = %group_label(sync), "nothing to commit");
        return Ok(());
    }

    // Count what the commit contains, for the run's summary.
    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut diff_options()))
        .map_err(gerr("diff index"))?;
    record_changes(&diff, report);

    let tree = repo.find_tree(tree_oid).map_err(gerr("find tree"))?;
    let sig = signature(repo)?;
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let message = commit_message(sync, repo)?;
    repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)
        .map_err(gerr("commit"))?;
    debug!(group = %group_label(sync), "committed local changes");
    Ok(())
}

/// Fetch the upstream branch so the local `origin/<branch>` ref is current.
fn fetch(sync: &Sync, repo: &Repository, branch: &str) -> Result<()> {
    let remote_name = remote_name(repo, branch)?;
    let mut remote = repo
        .find_remote(&remote_name)
        .map_err(gerr("find the tracked remote"))?;
    let url = remote.url().unwrap_or_default().to_string();
    let auth = GitAuth::for_clone(&sync.local, &url);

    // Fetch the upstream's own branch name, which need not match the local one,
    // into its remote-tracking ref.
    let remote_branch = upstream_branch_name(&upstream_of(repo, branch)?)?;
    let refspec = format!("+refs/heads/{remote_branch}:refs/remotes/{remote_name}/{remote_branch}");
    let mut fo = FetchOptions::new();
    fo.remote_callbacks(auth.callbacks());
    fo.download_tags(AutotagOption::Auto);
    remote
        .fetch(&[&refspec], Some(&mut fo), None)
        .map_err(gerr("fetch"))?;
    Ok(())
}

/// The branch name on the remote, i.e. `origin/main` without the remote prefix.
fn upstream_branch_name(upstream: &Branch<'_>) -> Result<String> {
    let full = branch_name(upstream)?;
    Ok(match full.split_once('/') {
        Some((_remote, branch)) => branch.to_string(),
        None => full,
    })
}

/// Bring the local branch up to date with its upstream: fast-forward when the
/// branch has no commits of its own, rebase when it does, nothing when it is
/// already current or purely ahead.
fn integrate(sync: &Sync, repo: &Repository, branch: &str, report: &mut SyncReport) -> Result<()> {
    let label = group_label(sync);
    let local = repo
        .find_branch(branch, git2::BranchType::Local)
        .map_err(gerr("find branch"))?;
    let upstream = upstream_of(repo, branch)?;
    let local_oid = branch_oid(repo, &local)?;
    let upstream_oid = branch_oid(repo, &upstream)?;

    if local_oid == upstream_oid {
        debug!(group = %label, "already up to date with upstream");
        return Ok(());
    }
    let (ahead, behind) = repo
        .graph_ahead_behind(local_oid, upstream_oid)
        .map_err(gerr("compare with upstream"))?;
    if behind == 0 {
        debug!(group = %label, ahead, "nothing to bring down");
        return Ok(());
    }

    // Remember the tree we start from, so the summary can report what the working
    // tree actually gained.
    let old_tree_id = head_tree(repo)?.map(|t| t.id());

    if ahead == 0 {
        debug!(group = %label, behind, "fast-forwarding to upstream");
        fast_forward(repo, &local, upstream_oid)?;
    } else {
        debug!(group = %label, ahead, behind, "rebasing local commits onto upstream");
        rebase_onto_upstream(repo, &upstream, &label)?;
    }

    // Diff old HEAD against the *new* one rather than against upstream: after a
    // rebase the local commits are replayed on top, so upstream's tree is missing
    // them and would read as though they had been deleted.
    let old_tree = match old_tree_id {
        Some(id) => Some(repo.find_tree(id).map_err(gerr("re-read previous tree"))?),
        None => None,
    };
    let new_tree = head_tree(repo)?;
    let diff = repo
        .diff_tree_to_tree(
            old_tree.as_ref(),
            new_tree.as_ref(),
            Some(&mut diff_options()),
        )
        .map_err(gerr("diff integrated changes"))?;
    record_changes(&diff, report);
    Ok(())
}

/// Move the branch to `target` and check that commit out. A non-force checkout is
/// used deliberately: if it would overwrite an uncommitted local change, it fails
/// and the run reports that instead of destroying the change.
fn fast_forward(repo: &Repository, branch: &Branch<'_>, target: git2::Oid) -> Result<()> {
    let refname = ref_name(branch)?;
    let object = repo
        .find_object(target, None)
        .map_err(gerr("find commit"))?;
    repo.checkout_tree(&object, Some(CheckoutBuilder::new().safe()))
        .map_err(|e| {
            Error::Backend(format!(
                "cannot fast-forward: checking out the upstream commit would overwrite local \
                 changes ({e})"
            ))
        })?;
    let mut reference: Reference = repo
        .find_reference(&refname)
        .map_err(gerr("find branch reference"))?;
    reference
        .set_target(target, "conflux: fast-forward to upstream")
        .map_err(gerr("update branch"))?;
    repo.set_head(&refname).map_err(gerr("set HEAD"))?;
    Ok(())
}

/// Rebase the branch's own commits onto the upstream tip.
///
/// On conflict the rebase is aborted, so the repository is left exactly as it was,
/// and the error tells the user to resolve it with git. That is the one thing
/// conflux must not do automatically: picking a side would throw away a commit.
fn rebase_onto_upstream(repo: &Repository, upstream: &Branch<'_>, label: &str) -> Result<()> {
    let upstream_commit: AnnotatedCommit = repo
        .reference_to_annotated_commit(upstream.get())
        .map_err(gerr("read upstream commit"))?;
    let mut rebase = repo
        .rebase(None, Some(&upstream_commit), None, None)
        .map_err(gerr("start rebase"))?;

    while let Some(op) = rebase.next() {
        op.map_err(gerr("apply rebase step"))?;
        if repo.index().map_err(gerr("open index"))?.has_conflicts() {
            let _ = rebase.abort();
            return Err(Error::Precondition(format!(
                "sync group `{label}`: local commits and the upstream branch both changed the \
                 same lines, so conflux stopped and left the repository untouched — resolve it \
                 with git (e.g. `git pull --rebase`) and the next sync will continue"
            )));
        }
        let sig = signature(repo)?;
        match rebase.commit(None, &sig, None) {
            Ok(_) => {}
            // An empty step means upstream already carries this change.
            Err(e) if e.code() == git2::ErrorCode::Applied => {}
            Err(e) => {
                let _ = rebase.abort();
                return Err(Error::Backend(format!("git rebase step failed: {e}")));
            }
        }
    }
    rebase.finish(None).map_err(gerr("finish rebase"))?;
    Ok(())
}

/// Push the branch to its upstream when it is ahead. A rejected push is reported
/// as an error; the next run fetches and rebases before trying again.
fn push(sync: &Sync, repo: &Repository, branch: &str) -> Result<()> {
    let local = repo
        .find_branch(branch, git2::BranchType::Local)
        .map_err(gerr("find branch"))?;
    let upstream = upstream_of(repo, branch)?;
    let local_oid = branch_oid(repo, &local)?;
    let upstream_oid = branch_oid(repo, &upstream)?;
    if local_oid == upstream_oid {
        return Ok(());
    }
    let (ahead, _behind) = repo
        .graph_ahead_behind(local_oid, upstream_oid)
        .map_err(gerr("compare with upstream"))?;
    if ahead == 0 {
        return Ok(());
    }

    let remote_name = remote_name(repo, branch)?;
    let mut remote = repo
        .find_remote(&remote_name)
        .map_err(gerr("find the tracked remote"))?;
    let url = remote.url().unwrap_or_default().to_string();
    let auth = GitAuth::for_clone(&sync.local, &url);

    let local_ref = ref_name(&local)?;
    let remote_branch = upstream_branch_name(&upstream)?;
    let refspec = format!("{local_ref}:refs/heads/{remote_branch}");

    let mut po = PushOptions::new();
    po.remote_callbacks(auth.callbacks());
    remote.push(&[&refspec], Some(&mut po)).map_err(|e| {
        Error::Backend(format!(
            "push to {remote_name}/{remote_branch} failed (the next sync will fetch and retry): {e}"
        ))
    })?;
    debug!(group = %group_label(sync), "pushed to {remote_name}/{remote_branch}");
    Ok(())
}

/// The commit message: the group's `commit_msg_command` output, else the default
/// `"conflux sync from <hostname>"`.
fn commit_message(sync: &Sync, repo: &Repository) -> Result<String> {
    let Some(cmd) = &sync.commit_msg_command else {
        return Ok(crate::backend::git::default_commit_message());
    };
    let dir = repo.workdir().unwrap_or(&sync.local);
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .map_err(|e| Error::Backend(format!("commit_msg_command failed to run: {e}")))?;
    if !output.status.success() {
        return Err(Error::Backend(format!(
            "commit_msg_command exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let msg = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    Ok(if msg.is_empty() {
        crate::backend::git::default_commit_message()
    } else {
        msg
    })
}

/// The signature to author with: the repository's configured identity, falling
/// back to conflux's own when git has none (a headless box with no `user.email`).
fn signature(repo: &Repository) -> Result<Signature<'static>> {
    match repo.signature() {
        Ok(sig) => Ok(sig.to_owned()),
        Err(_) => Signature::now("conflux", "conflux@localhost").map_err(gerr("signature")),
    }
}

/// The tree of the current HEAD commit, or `None` in a repository whose HEAD has
/// no commit yet.
fn head_tree(repo: &Repository) -> Result<Option<git2::Tree<'_>>> {
    match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
        Some(commit) => Ok(Some(commit.tree().map_err(gerr("read HEAD tree"))?)),
        None => Ok(None),
    }
}

/// Diff options shared by the counting and preview paths: submodules are left
/// alone (conflux does not recurse into them) and renames are not detected, so a
/// rename reads as one add plus one delete.
fn diff_options() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    opts
}

/// Which way a previewed change would travel.
enum Direction {
    Push,
    Pull,
}

/// Tally a diff into a [`SyncReport`], so a git run reports the same
/// `+added -removed *modified` summary as a file-level one.
fn record_changes(diff: &Diff<'_>, report: &mut SyncReport) {
    for delta in diff.deltas() {
        let Some(path) = delta_path(&delta) else {
            continue;
        };
        match delta.status() {
            Delta::Added | Delta::Untracked | Delta::Copied => report.added.push(path),
            Delta::Deleted => report.removed.push(path),
            _ => report.modified.push(path),
        }
    }
}

/// Render a diff as planned changes in one direction, for a dry run.
fn push_changes(diff: &Diff<'_>, plan: &mut SyncPlan, direction: Direction) {
    for delta in diff.deltas() {
        let Some(path) = delta_path(&delta) else {
            continue;
        };
        let op = match (&direction, delta.status()) {
            (Direction::Push, Delta::Deleted) => PlanOp::DeleteRemote,
            (Direction::Push, Delta::Added | Delta::Untracked | Delta::Copied) => {
                PlanOp::Push { update: false }
            }
            (Direction::Push, _) => PlanOp::Push { update: true },
            (Direction::Pull, Delta::Deleted) => PlanOp::DeleteLocal,
            (Direction::Pull, Delta::Added | Delta::Untracked | Delta::Copied) => {
                PlanOp::Pull { update: false }
            }
            (Direction::Pull, _) => PlanOp::Pull { update: true },
        };
        plan.changes.push(PlannedChange { path, op });
    }
}

/// The path a diff delta concerns, as a sync-relative path.
fn delta_path(delta: &git2::DiffDelta<'_>) -> Option<RelPath> {
    let path = delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())?;
    RelPath::from_relative(Path::new(path))
}

/// Map a git2 error to our backend error, tagging the operation.
fn gerr(op: &'static str) -> impl Fn(git2::Error) -> Error {
    move |e| Error::Backend(format!("git {op} failed: {e}"))
}

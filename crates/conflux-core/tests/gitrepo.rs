//! `git_repo` integration tests: two real working clones of one bare repo, with
//! no network. Each `sync()` here is what the daemon does for such a group.

use conflux_core::engine::{PlanOp, SyncReport};
use conflux_core::{gitrepo, Config};
use git2::{Repository, Signature};
use std::fs;
use std::path::Path;
use std::process::Command;

/// A bare repo with one commit on `main`, to clone from.
fn seed_bare(path: &Path) {
    let repo = Repository::init_bare(path).unwrap();
    let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = Signature::now("seed", "seed@localhost").unwrap();
    repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    repo.set_head("refs/heads/main").unwrap();
}

/// Clone `bare` into `dest` with a local identity configured, the way a user's own
/// clone would be. `git2`'s clone leaves the upstream tracking branch set up, which
/// is what a `git_repo` sync relies on.
fn clone(bare: &Path, dest: &Path) {
    let repo = Repository::clone(bare.to_str().unwrap(), dest).unwrap();
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "Test User").unwrap();
    config.set_str("user.email", "test@localhost").unwrap();
}

/// The config a two-line `[[sync]]` produces for a clone.
fn config(repo: &Path) -> Config {
    let toml = format!("[[sync]]\ngit_repo = \"{}\"\n", repo.display());
    Config::from_toml_str(&toml).expect("config should parse")
}

fn sync(cfg: &Config) -> SyncReport {
    gitrepo::run(&cfg.syncs[0], false).expect("sync should succeed")
}

fn fetch_only(cfg: &Config) -> SyncReport {
    gitrepo::run(&cfg.syncs[0], true).expect("pull-only sync should succeed")
}

/// Commit a file in a clone with plain git, standing in for the user's own work.
fn user_commit(repo: &Path, name: &str, contents: &str, message: &str) {
    fs::write(repo.join(name), contents).unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message]);
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@localhost",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should run");
    assert!(
        status.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// The one-line log of a clone's current branch, newest first.
fn log(repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["log", "--format=%s"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    bare: std::path::PathBuf,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
}

/// A bare "server" plus two clones of it, standing in for two machines.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("notes.git");
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    seed_bare(&bare);
    clone(&bare, &a);
    clone(&bare, &b);
    Fixture {
        _tmp: tmp,
        bare,
        a,
        b,
    }
}

#[test]
fn commits_and_pushes_local_changes() {
    let f = fixture();
    let cfg = config(&f.a);

    fs::write(f.a.join("note.md"), "hello").unwrap();
    fs::create_dir_all(f.a.join("sub")).unwrap();
    fs::write(f.a.join("sub/deep.md"), "nested").unwrap();

    let report = sync(&cfg);
    assert_eq!(report.added.len(), 2, "both new files are committed");

    // The commit really reached the bare repo, and the clone's own history grew
    // rather than being replaced.
    let bare = Repository::open_bare(&f.bare).unwrap();
    let head = bare.head().unwrap().peel_to_commit().unwrap();
    assert!(head.message().unwrap().starts_with("conflux sync"));
    assert_eq!(log(&f.a).len(), 2, "one new commit on top of `init`");

    // A second run has nothing to do.
    assert!(sync(&cfg).is_empty(), "second run should be a no-op");
}

#[test]
fn commits_use_the_repositorys_own_identity() {
    let f = fixture();
    fs::write(f.a.join("note.md"), "hello").unwrap();
    sync(&config(&f.a));

    let repo = Repository::open(&f.a).unwrap();
    let author = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .author()
        .to_owned();
    assert_eq!(author.name(), Some("Test User"));
    assert_eq!(author.email(), Some("test@localhost"));
}

#[test]
fn fast_forwards_when_the_clone_has_nothing_of_its_own() {
    let f = fixture();
    let cfg_a = config(&f.a);
    let cfg_b = config(&f.b);

    // B commits and pushes through conflux.
    fs::write(f.b.join("from-b.md"), "b was here").unwrap();
    sync(&cfg_b);

    // A has no local work, so it fast-forwards — no merge commit, and the file
    // lands in the working tree.
    let report = sync(&cfg_a);
    assert_eq!(report.added.len(), 1);
    assert_eq!(
        fs::read_to_string(f.a.join("from-b.md")).unwrap(),
        "b was here"
    );
    assert_eq!(log(&f.a).len(), 2, "fast-forward adds no extra commit");
}

#[test]
fn deletions_travel_as_commits() {
    let f = fixture();
    let (cfg_a, cfg_b) = (config(&f.a), config(&f.b));

    fs::write(f.a.join("note.md"), "hello").unwrap();
    sync(&cfg_a);
    sync(&cfg_b);
    assert!(f.b.join("note.md").exists());

    // Removing the file locally is committed as a deletion and pushed...
    fs::remove_file(f.a.join("note.md")).unwrap();
    let report = sync(&cfg_a);
    assert_eq!(report.removed.len(), 1);

    // ...and the other clone drops it on the next sync.
    let report = sync(&cfg_b);
    assert_eq!(report.removed.len(), 1);
    assert!(!f.b.join("note.md").exists());
}

#[test]
fn rebases_local_commits_onto_upstream_when_both_moved() {
    let f = fixture();
    let (cfg_a, cfg_b) = (config(&f.a), config(&f.b));

    // B pushes a commit.
    fs::write(f.b.join("from-b.md"), "b").unwrap();
    sync(&cfg_b);

    // A has its own commit, made with plain git before it ever fetched: the
    // branches have diverged.
    user_commit(&f.a, "from-a.md", "a", "a's own work");

    let report = sync(&cfg_a);
    // A's local commit is rebased on top of B's, and both files are present.
    assert!(f.a.join("from-a.md").exists());
    assert!(f.a.join("from-b.md").exists());
    assert_eq!(report.added.len(), 1, "B's file arrives");
    // The summary describes what the working tree gained. A's own replayed file
    // must not read as removed just because upstream's tree lacked it.
    assert!(
        report.removed.is_empty(),
        "a rebase removes nothing: {report:?}"
    );
    let history = log(&f.a);
    assert_eq!(
        history.len(),
        3,
        "linear history, no merge commit: {history:?}"
    );
    assert_eq!(history[0], "a's own work", "A's commit is replayed on top");
    assert_eq!(history[2], "init");

    // Both machines converge.
    sync(&cfg_b);
    assert!(f.b.join("from-a.md").exists());
}

#[test]
fn a_real_conflict_aborts_and_leaves_the_repository_untouched() {
    let f = fixture();
    let (cfg_a, cfg_b) = (config(&f.a), config(&f.b));

    // Both clones start from a shared file.
    fs::write(f.a.join("note.md"), "base\n").unwrap();
    sync(&cfg_a);
    sync(&cfg_b);

    // Each side changes the same line, and B pushes first.
    fs::write(f.b.join("note.md"), "b's version\n").unwrap();
    sync(&cfg_b);
    user_commit(&f.a, "note.md", "a's version\n", "a edits the same line");
    let before = log(&f.a);

    let err = gitrepo::run(&cfg_a.syncs[0], false).expect_err("the rebase should not be resolved");
    let message = err.to_string();
    assert!(message.contains("resolve it with git"), "{message}");

    // The abort left A exactly as it was: same history, same content, and no
    // rebase in progress to clean up.
    assert_eq!(log(&f.a), before);
    assert_eq!(
        fs::read_to_string(f.a.join("note.md")).unwrap(),
        "a's version\n"
    );
    let repo = Repository::open(&f.a).unwrap();
    assert_eq!(repo.state(), git2::RepositoryState::Clean);

    // Resolving by hand (here: take A's side) lets the next sync proceed.
    git(&f.a, &["fetch", "origin"]);
    git(&f.a, &["rebase", "--strategy-option=theirs", "origin/main"]);
    sync(&cfg_a);
    assert_eq!(
        fs::read_to_string(f.a.join("note.md")).unwrap(),
        "a's version\n"
    );
}

#[test]
fn refuses_to_sync_a_repository_mid_operation() {
    let f = fixture();
    let cfg = config(&f.a);

    // Leave a merge in progress by hand.
    fs::write(f.a.join(".git/MERGE_HEAD"), "deadbeef\n").unwrap();

    let err = gitrepo::run(&cfg.syncs[0], false).expect_err("should refuse");
    assert!(err.to_string().contains("in the middle of"), "{err}");
}

#[test]
fn refuses_a_detached_head_and_a_plain_directory() {
    let f = fixture();

    // Detached HEAD: no branch to push or rebase.
    git(&f.a, &["checkout", "--detach", "HEAD"]);
    let err = gitrepo::run(&config(&f.a).syncs[0], false).expect_err("should refuse");
    assert!(err.to_string().contains("detached"), "{err}");

    // Not a repository at all.
    let plain = f.b.parent().unwrap().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let err = gitrepo::run(&config(&plain).syncs[0], false).expect_err("should refuse");
    assert!(err.to_string().contains("is not a git repository"), "{err}");
    assert!(
        err.to_string().contains(&plain.display().to_string()),
        "the message names the path: {err}"
    );

    // A path that does not exist says so, rather than blaming the contents.
    let missing = f.b.parent().unwrap().join("missing");
    let err = gitrepo::run(&config(&missing).syncs[0], false).expect_err("should refuse");
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn user_fixable_failures_are_preconditions_the_daemon_will_not_retry() {
    // The daemon retries transient failures with backoff. These are not transient:
    // nothing will change until the user acts, so they are typed distinctly and
    // reported once instead of three times per trigger.
    let f = fixture();
    let plain = f.b.parent().unwrap().join("plain2");
    fs::create_dir_all(&plain).unwrap();

    for err in [
        // Not a clone.
        gitrepo::run(&config(&plain).syncs[0], false).unwrap_err(),
        // Mid-operation.
        {
            fs::write(f.a.join(".git/MERGE_HEAD"), "deadbeef\n").unwrap();
            let e = gitrepo::run(&config(&f.a).syncs[0], false).unwrap_err();
            fs::remove_file(f.a.join(".git/MERGE_HEAD")).unwrap();
            e
        },
        // No upstream to sync with.
        {
            git(&f.b, &["checkout", "-q", "-b", "no-upstream"]);
            gitrepo::run(&config(&f.b).syncs[0], false).unwrap_err()
        },
    ] {
        assert!(
            matches!(err, conflux_core::Error::Precondition(_)),
            "should be a precondition, not a retryable failure: {err:?}"
        );
        // Preconditions are written for the user, so they carry no `backend error:`
        // style prefix.
        assert!(!err.to_string().starts_with("backend error"), "{err}");
    }
}

#[test]
fn a_pull_only_refresh_fetches_without_committing_or_pushing() {
    let f = fixture();
    let (cfg_a, cfg_b) = (config(&f.a), config(&f.b));

    // B pushes something for A to pick up.
    fs::write(f.b.join("from-b.md"), "b").unwrap();
    sync(&cfg_b);

    // A has uncommitted work in progress.
    fs::write(f.a.join("wip.md"), "not ready").unwrap();

    let report = fetch_only(&cfg_a);
    // B's commit arrived...
    assert_eq!(report.added.len(), 1);
    assert!(f.a.join("from-b.md").exists());
    // ...but A's work in progress was neither committed nor pushed.
    assert_eq!(log(&f.a).len(), 2);
    let bare = Repository::open_bare(&f.bare).unwrap();
    let tree = bare.head().unwrap().peel_to_tree().unwrap();
    assert!(tree.get_name("wip.md").is_none(), "wip must not be pushed");
    assert!(f.a.join("wip.md").exists(), "wip stays in the working tree");
}

#[test]
fn ignored_files_are_never_committed() {
    let f = fixture();
    let cfg = config(&f.a);

    fs::write(f.a.join(".gitignore"), "*.log\nbuild/\n").unwrap();
    fs::write(f.a.join("keep.md"), "keep").unwrap();
    fs::write(f.a.join("noise.log"), "noise").unwrap();
    fs::create_dir_all(f.a.join("build")).unwrap();
    fs::write(f.a.join("build/out.bin"), "binary").unwrap();

    sync(&cfg);

    let bare = Repository::open_bare(&f.bare).unwrap();
    let tree = bare.head().unwrap().peel_to_tree().unwrap();
    assert!(tree.get_name("keep.md").is_some());
    assert!(tree.get_name(".gitignore").is_some());
    assert!(
        tree.get_name("noise.log").is_none(),
        "ignored file committed"
    );
    assert!(tree.get_name("build").is_none(), "ignored dir committed");
}

#[test]
fn commit_msg_command_sets_the_message() {
    let f = fixture();
    let cfg = Config::from_toml_str(&format!(
        "[[sync]]\ngit_repo = \"{}\"\ncommit_msg_command = \"printf 'notes: %s' update\"\n",
        f.a.display()
    ))
    .unwrap();

    fs::write(f.a.join("note.md"), "hello").unwrap();
    sync(&cfg);
    assert_eq!(log(&f.a)[0], "notes: update");
}

#[test]
fn dry_run_reports_both_directions_without_changing_anything() {
    let f = fixture();
    let (cfg_a, cfg_b) = (config(&f.a), config(&f.b));

    // Something to pull...
    fs::write(f.b.join("from-b.md"), "b").unwrap();
    sync(&cfg_b);
    // ...and something to push.
    fs::write(f.a.join("from-a.md"), "a").unwrap();

    let before = log(&f.a);
    let plan = gitrepo::plan(&cfg_a.syncs[0]).expect("plan should succeed");

    let pushes: Vec<_> = plan
        .changes
        .iter()
        .filter(|c| matches!(c.op, PlanOp::Push { .. }))
        .map(|c| c.path.as_str().to_string())
        .collect();
    let pulls: Vec<_> = plan
        .changes
        .iter()
        .filter(|c| matches!(c.op, PlanOp::Pull { .. }))
        .map(|c| c.path.as_str().to_string())
        .collect();
    assert_eq!(pushes, vec!["from-a.md"]);
    assert_eq!(pulls, vec!["from-b.md"]);

    // Nothing was committed, pushed, or checked out.
    assert_eq!(log(&f.a), before);
    assert!(!f.a.join("from-b.md").exists());
    let bare = Repository::open_bare(&f.bare).unwrap();
    let tree = bare.head().unwrap().peel_to_tree().unwrap();
    assert!(tree.get_name("from-a.md").is_none());
}

//! Terminal rendering of daemon responses for the CLI control subcommands.
//!
//! The request itself goes through [`conflux_core::ipc::client`].

use std::path::Path;

use anyhow::Context;
use conflux_core::ipc::{client, Request, Response};

/// Send a request to the daemon, waiting as long as it takes. When nothing
/// listens on `socket`, `legacy` (where a daemon from before the socket moved
/// listens) is tried too.
pub fn send(socket: &Path, legacy: Option<&Path>, request: &Request) -> anyhow::Result<Response> {
    let result = match client::send(socket, request, None) {
        Err(e) if client::is_not_running(&e) => match legacy {
            Some(old) => client::send(old, request, None).map_err(|_| e),
            None => Err(e),
        },
        other => other,
    };
    result.with_context(|| {
        format!(
            "could not talk to the daemon at {} (is it running?)",
            socket.display()
        )
    })
}

/// Render a [`Response`] for the terminal. Returns whether the command succeeded.
pub fn print_response(response: &Response) -> bool {
    match response {
        Response::Status(groups) => {
            if groups.is_empty() {
                println!("no active sync groups");
            }
            for g in groups {
                let when = match g.last_run {
                    Some(secs) => conflux_core::timefmt::local(secs),
                    None => "never".to_string(),
                };
                println!("{} [{}] root={}", g.label, g.trigger, g.root);
                match (&g.last_summary, &g.last_error) {
                    (_, Some(err)) => println!("  last run: ERROR ({when}): {err}"),
                    (Some(s), None) => println!("  last run: {when} — {s}"),
                    (None, None) => println!("  last run: {when}"),
                }
            }
            true
        }
        Response::Synced(outcomes) => {
            let mut ok = true;
            for o in outcomes {
                match (&o.summary, &o.error) {
                    (Some(s), _) => println!("{}: {s}", o.label),
                    (None, Some(err)) => {
                        ok = false;
                        println!("{}: ERROR: {err}", o.label);
                    }
                    (None, None) => println!("{}: done", o.label),
                }
            }
            ok
        }
        Response::Planned(plans) => {
            let mut ok = true;
            for gp in plans {
                if let Some(err) = &gp.error {
                    ok = false;
                    println!("{} (dry run): ERROR: {err}", gp.label);
                    continue;
                }
                // `plan` is always Some when `error` is None.
                let changes = gp.plan.as_ref().map(|p| &p.changes[..]).unwrap_or(&[]);
                if changes.is_empty() {
                    println!("{} (dry run): no changes", gp.label);
                    continue;
                }
                let n = changes.len();
                println!(
                    "{} (dry run): {n} change{}",
                    gp.label,
                    if n == 1 { "" } else { "s" }
                );
                for change in changes {
                    println!(
                        "  {} {:<38} {}",
                        change.op.symbol(),
                        change.op.describe(),
                        change.path
                    );
                }
            }
            ok
        }
        Response::Info(info) => {
            println!(
                "conflux {} (pid {}), profile {}, config {}",
                info.version, info.pid, info.profile, info.config
            );
            true
        }
        Response::Reloaded { ok, message } => {
            println!("{message}");
            *ok
        }
        Response::Error(err) => {
            eprintln!("error: {err}");
            false
        }
    }
}

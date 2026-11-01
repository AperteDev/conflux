//! Terminal rendering of daemon responses for the CLI control subcommands.
//!
//! The request itself goes through [`conflux_core::ipc::client`].

use std::path::Path;

use anyhow::{anyhow, Context};
use conflux_core::ipc::{client, Request, Response};
use conflux_core::paths::{unit_name, DEFAULT_PROFILE};

/// The daemon a control command talks to.
pub struct Target<'a> {
    pub socket: &'a Path,
    /// Where a user daemon from before the socket moved listens, tried when
    /// nothing listens on `socket`.
    pub legacy: Option<&'a Path>,
    pub system: bool,
    pub profile: &'a str,
}

/// Send a request to the daemon, waiting as long as it takes.
pub fn send(target: &Target, request: &Request) -> anyhow::Result<Response> {
    let result = match client::send(target.socket, request, None) {
        Err(e) if client::is_not_running(&e) => match target.legacy {
            Some(old) => client::send(old, request, None).map_err(|_| e),
            None => Err(e),
        },
        other => other,
    };
    match result {
        Err(e) if client::is_not_running(&e) => Err(anyhow!(not_running(target))),
        other => other.with_context(|| {
            format!(
                "could not talk to the daemon at {}",
                target.socket.display()
            )
        }),
    }
}

/// Say that `target`'s daemon is not running, and how to start it.
fn not_running(target: &Target) -> String {
    let default = target.profile == DEFAULT_PROFILE;
    let whose = match (target.system, default) {
        (false, true) => "the conflux daemon".to_string(),
        (false, false) => format!("the conflux daemon of profile {}", target.profile),
        (true, true) => "the system's conflux daemon".to_string(),
        (true, false) => format!("the system's conflux daemon of profile {}", target.profile),
    };
    let unit = unit_name(target.profile);
    let unit = unit.trim_end_matches(".service");
    let start = if target.system {
        format!("`sudo systemctl start {unit}`")
    } else {
        let profile = if default {
            String::new()
        } else {
            format!(" --profile {}", target.profile)
        };
        format!("`systemctl --user start {unit}`, or run `conflux{profile} daemon`")
    };
    format!(
        "{whose} is not running (nothing listens on {})\nstart it with {start}",
        target.socket.display()
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn message(system: bool, profile: &str) -> String {
        let socket = Path::new("/run/conflux.sock");
        not_running(&Target {
            socket,
            legacy: None,
            system,
            profile,
        })
    }

    #[test]
    fn not_running_says_how_to_start_the_daemon() {
        assert_eq!(
            message(false, "default"),
            "the conflux daemon is not running (nothing listens on /run/conflux.sock)\n\
             start it with `systemctl --user start conflux`, or run `conflux daemon`"
        );
        assert_eq!(
            message(false, "desktop"),
            "the conflux daemon of profile desktop is not running \
             (nothing listens on /run/conflux.sock)\n\
             start it with `systemctl --user start conflux@desktop`, \
             or run `conflux --profile desktop daemon`"
        );
        assert!(message(true, "default").ends_with("`sudo systemctl start conflux`"));
        assert!(
            message(true, "backup").starts_with("the system's conflux daemon of profile backup")
        );
        assert!(message(true, "backup").ends_with("`sudo systemctl start conflux@backup`"));
    }
}

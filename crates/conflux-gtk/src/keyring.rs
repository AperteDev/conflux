//! A remote's password in the user's keyring, through `secret-tool`: the app
//! stores it, and the daemon reads it with the `password_command` the app
//! writes. No GTK here; `store` blocks, so it runs off the main thread.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::model::{self, Scope};

/// Whether the app can keep `scope`'s passwords in the keyring: the user's own,
/// outside the Flatpak (whose keyring the host's daemon cannot read), with
/// `secret-tool` installed. The system daemon has no keyring of its own.
pub fn available(scope: Scope) -> bool {
    scope == Scope::User && !model::sandboxed() && on_path("secret-tool")
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The attributes the password of remote `id` is stored under.
fn attributes(id: &str) -> [&str; 4] {
    ["service", "conflux", "remote", id]
}

/// Store `password` as the password of remote `id`, replacing any before it.
pub fn store(id: &str, password: &str) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .arg("store")
        .arg(format!("--label=conflux: {id}"))
        .args(attributes(id))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run secret-tool: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(password.as_bytes())
        .map_err(|e| format!("could not pass the password to secret-tool: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("secret-tool failed: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!("secret-tool failed: {}", err.trim()))
    }
}

/// The `password_command` reading the password of remote `id` back.
pub fn lookup_command(id: &str) -> String {
    let words: Vec<String> = std::iter::once("secret-tool")
        .chain(std::iter::once("lookup"))
        .chain(attributes(id))
        .map(quote)
        .collect();
    words.join(" ")
}

/// `word` as `sh` reads it back: as is when it is plain, else single-quoted.
fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_commands_quote_odd_ids() {
        assert_eq!(
            lookup_command("nextcloud"),
            "secret-tool lookup service conflux remote nextcloud"
        );
        assert_eq!(
            lookup_command("my cloud's"),
            r"secret-tool lookup service conflux remote 'my cloud'\''s'"
        );
        assert_eq!(quote(""), "''");
    }
}

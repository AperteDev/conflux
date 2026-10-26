//! What systemd knows about the daemons' units, asked over D-Bus: the user's
//! manager on the session bus, the system's on the system bus. Reading needs no
//! privileges on either.

use std::collections::HashMap;

use gtk::{gio, glib, prelude::*};

use crate::model::Scope;

const TIMEOUT_MS: i32 = 5000;

/// A unit as systemd sees it.
#[derive(Debug, Clone, Default)]
pub struct Unit {
    /// Whether systemd has a unit file for it (for an instance: the template).
    pub installed: bool,
    /// `active`, `inactive`, `failed`, `activating`, ...
    pub active_state: String,
    /// Whether it starts with the session (user) or at boot (system).
    pub enabled: bool,
}

impl Unit {
    pub fn is_active(&self) -> bool {
        matches!(self.active_state.as_str(), "active" | "reloading")
    }
}

/// The state of each of `names`. Fails when systemd cannot be reached (no
/// systemd, or no session bus); units it does not know come back not installed.
pub async fn units(scope: Scope, names: &[String]) -> Result<HashMap<String, Unit>, glib::Error> {
    let bus = gio::bus_get_future(match scope {
        Scope::User => gio::BusType::Session,
        Scope::System => gio::BusType::System,
    })
    .await?;
    let reply = call(
        &bus,
        "ListUnitsByNames",
        &(names.to_vec(),).to_variant(),
        "(a(ssssssouso))",
    )
    .await?;
    let mut out = HashMap::new();
    let list = reply.child_value(0);
    for i in 0..list.n_children() {
        let unit = list.child_value(i);
        let text = |n| unit.child_value(n).str().unwrap_or_default().to_string();
        let name = text(0);
        let installed = text(2) == "loaded";
        let enabled = if installed {
            call(
                &bus,
                "GetUnitFileState",
                &(name.as_str(),).to_variant(),
                "(s)",
            )
            .await
            .ok()
            .and_then(|v| v.child_value(0).str().map(|s| s == "enabled"))
            .unwrap_or(false)
        } else {
            false
        };
        out.insert(
            name,
            Unit {
                installed,
                active_state: text(3),
                enabled,
            },
        );
    }
    Ok(out)
}

async fn call(
    bus: &gio::DBusConnection,
    method: &str,
    args: &glib::Variant,
    reply: &str,
) -> Result<glib::Variant, glib::Error> {
    bus.call_future(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        method,
        Some(args),
        Some(glib::VariantTy::new(reply).expect("valid D-Bus signature")),
        gio::DBusCallFlags::NONE,
        TIMEOUT_MS,
    )
    .await
}

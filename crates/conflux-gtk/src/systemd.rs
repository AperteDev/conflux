//! The daemons' units as systemd sees them, and their control, over D-Bus: the
//! user's manager on the session bus, the system's on the system bus. Reading
//! needs no privileges; controlling the user's own units needs none either.
//! Lingering, which keeps the user's manager running after logout, is logind's.

use std::collections::HashMap;

use gtk::{gio, glib, prelude::*};

use crate::model::Scope;

const TIMEOUT_MS: i32 = 10_000;

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
        matches!(
            self.active_state.as_str(),
            "active" | "reloading" | "activating"
        )
    }
}

/// What systemd knows about a scope's daemons.
#[derive(Debug, Clone, Default)]
pub struct Systemd {
    pub units: HashMap<String, Unit>,
    /// Whether `conflux@.service` is installed, so profiles can get daemons.
    pub template: bool,
    /// Whether the user's daemons keep running after logout (user scope).
    pub linger: bool,
}

impl Systemd {
    pub fn unit(&self, name: &str) -> Option<&Unit> {
        self.units.get(name)
    }
}

fn bus_type(scope: Scope) -> gio::BusType {
    match scope {
        Scope::User => gio::BusType::Session,
        Scope::System => gio::BusType::System,
    }
}

/// The state of each of `names`. Fails when systemd cannot be reached (no
/// systemd, or no session bus); units it does not know come back not installed.
pub async fn load(scope: Scope, names: &[String]) -> Result<Systemd, glib::Error> {
    let bus = gio::bus_get_future(bus_type(scope)).await?;
    let reply = call(
        &bus,
        "ListUnitsByNames",
        &(names.to_vec(),).to_variant(),
        "(a(ssssssouso))",
    )
    .await?;
    let mut units = HashMap::new();
    let list = reply.child_value(0);
    for i in 0..list.n_children() {
        let unit = list.child_value(i);
        let text = |n| unit.child_value(n).str().unwrap_or_default().to_string();
        let name = text(0);
        let installed = text(2) == "loaded";
        let enabled = installed && file_state(&bus, &name).await.as_deref() == Some("enabled");
        units.insert(
            name,
            Unit {
                installed,
                active_state: text(3),
                enabled,
            },
        );
    }
    Ok(Systemd {
        units,
        template: file_state(&bus, "conflux@.service").await.is_some(),
        linger: scope == Scope::User && lingering().await.unwrap_or(false),
    })
}

/// A unit file's state (`enabled`, `disabled`, `static`, ...), or `None` when
/// there is no such file.
async fn file_state(bus: &gio::DBusConnection, name: &str) -> Option<String> {
    call(bus, "GetUnitFileState", &(name,).to_variant(), "(s)")
        .await
        .ok()
        .and_then(|v| v.child_value(0).str().map(str::to_string))
}

/// What can be done to a unit.
#[derive(Debug, Clone, Copy)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

/// Start, stop or restart `unit`; returns once systemd has queued the job.
pub async fn control(scope: Scope, unit: &str, action: Action) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(bus_type(scope)).await?;
    let method = match action {
        Action::Start => "StartUnit",
        Action::Stop => "StopUnit",
        Action::Restart => "RestartUnit",
    };
    call(&bus, method, &(unit, "replace").to_variant(), "(o)").await?;
    Ok(())
}

/// Make `unit` start at login (user) or boot (system), or not.
pub async fn set_enabled(scope: Scope, unit: &str, enabled: bool) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(bus_type(scope)).await?;
    let files = vec![unit.to_string()];
    if enabled {
        call(
            &bus,
            "EnableUnitFiles",
            &(files, false, false).to_variant(),
            "(ba(sss))",
        )
        .await?;
    } else {
        call(
            &bus,
            "DisableUnitFiles",
            &(files, false).to_variant(),
            "(a(sss))",
        )
        .await?;
    }
    // As `systemctl enable` does, so the manager sees the new links.
    call(&bus, "Reload", &().to_variant(), "()").await?;
    Ok(())
}

/// Whether the user lingers: logind keeps their manager, and so their daemons,
/// running without a session. Asked of logind, which a sandbox can reach too.
async fn lingering() -> Result<bool, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::System).await?;
    // SAFETY: `getuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    let user = bus
        .call_future(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "GetUser",
            Some(&(uid,).to_variant()),
            Some(glib::VariantTy::new("(o)").expect("valid D-Bus signature")),
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
        )
        .await?;
    let path = user.child_value(0);
    let path = path.str().unwrap_or_default();
    let linger = bus
        .call_future(
            Some("org.freedesktop.login1"),
            path,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&("org.freedesktop.login1.User", "Linger").to_variant()),
            Some(glib::VariantTy::new("(v)").expect("valid D-Bus signature")),
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
        )
        .await?;
    Ok(linger
        .child_value(0)
        .as_variant()
        .and_then(|v| v.get::<bool>())
        .unwrap_or(false))
}

/// Turn lingering on or off for the user. logind allows it for oneself by
/// default; where it does not, polkit asks.
pub async fn set_linger(enabled: bool) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::System).await?;
    // SAFETY: `getuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    bus.call_future(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        "SetUserLinger",
        Some(&(uid, enabled, true).to_variant()),
        None,
        gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION,
        TIMEOUT_MS,
    )
    .await?;
    Ok(())
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

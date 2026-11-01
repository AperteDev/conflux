//! Desktop notifications from a user daemon, over the session bus: a sync that
//! failed (once, after its retries, until it works again), and one that left
//! conflict copies. Clicking one opens the Conflux app at that sync, through
//! its `show-sync` action (`org.freedesktop.Application`, the app being D-Bus
//! activatable). Nothing is sent, nor the bus even joined, until there is
//! something to say; without a session bus there is nothing to say it on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use conflux_core::engine::SyncSummary;
use futures_util::StreamExt;
use tokio::sync::OnceCell;
use tracing::debug;
use zbus::zvariant::Value;
use zbus::{Connection, MatchRule, MessageStream};

/// The app, as the notifications name it and as the daemon opens it.
const APP_ID: &str = "dev.aperte.Conflux";
const APP_PATH: &str = "/dev/aperte/Conflux";
const SERVER: &str = "org.freedesktop.Notifications";
const SERVER_PATH: &str = "/org/freedesktop/Notifications";

pub struct Notifier {
    bus: OnceCell<Option<Connection>>,
    shown: Mutex<Shown>,
}

#[derive(Default)]
struct Shown {
    /// The syncs whose failure was notified, by label, with the notification
    /// (`0` once it is gone): they are not notified again until they work.
    failing: HashMap<String, u32>,
    /// The sync each notification opens, by notification.
    opens: HashMap<u32, String>,
    /// The token a click came with, to raise the app's window with.
    tokens: HashMap<u32, String>,
}

/// What a sync's run comes to for its notification.
pub enum Ran<'a> {
    Failed(&'a str),
    Synced(&'a SyncSummary),
}

impl Notifier {
    pub fn new() -> Arc<Self> {
        Arc::new(Notifier {
            bus: OnceCell::new(),
            shown: Mutex::default(),
        })
    }

    /// After a run of the sync with `label` (called `title`): notify a
    /// failure, unless one was notified for it already; withdraw that once it
    /// works; and notify conflict copies. `notify` is the config's switch;
    /// `asked` is a run someone asked for and sees the outcome of, which
    /// notifies nothing, though it still withdraws.
    pub async fn ran(
        self: &Arc<Self>,
        label: &str,
        title: &str,
        ran: Ran<'_>,
        notify: bool,
        asked: bool,
    ) {
        let quiet = !notify || asked;
        match ran {
            Ran::Failed(error) => {
                if quiet || self.shown.lock().unwrap().failing.contains_key(label) {
                    return;
                }
                let summary = format!("{title} could not sync");
                let body = conflux_core::error::reason(error);
                let id = self.send(&summary, &body, label).await.unwrap_or(0);
                self.shown
                    .lock()
                    .unwrap()
                    .failing
                    .insert(label.to_string(), id);
            }
            Ran::Synced(summary) => {
                let gone = self.shown.lock().unwrap().failing.remove(label);
                if let Some(id) = gone.filter(|id| *id != 0) {
                    self.close(id).await;
                }
                if quiet || summary.conflicted == 0 {
                    return;
                }
                let n = summary.conflicted;
                let files = if n == 1 { "file" } else { "files" };
                self.send(
                    &format!("{title}: {n} {files} changed on both sides"),
                    "Conflict copies keep the other versions",
                    label,
                )
                .await;
            }
        }
    }

    /// The session bus, joined the first time it is needed, and listened to
    /// for clicks from then on. `None` when there is none.
    async fn bus(self: &Arc<Self>) -> Option<&Connection> {
        self.bus
            .get_or_init(|| async {
                match Connection::session().await {
                    Ok(bus) => {
                        tokio::spawn(listen(bus.clone(), Arc::downgrade(self)));
                        Some(bus)
                    }
                    Err(e) => {
                        debug!("no session bus for notifications: {e}");
                        None
                    }
                }
            })
            .await
            .as_ref()
    }

    /// Show a notification opening the sync with `label`; its id.
    async fn send(self: &Arc<Self>, summary: &str, body: &str, label: &str) -> Option<u32> {
        let bus = self.bus().await?;
        let hints = HashMap::from([
            ("desktop-entry", Value::from(APP_ID)),
            // Normal.
            ("urgency", Value::U8(1)),
        ]);
        let reply = bus
            .call_method(
                Some(SERVER),
                SERVER_PATH,
                Some(SERVER),
                "Notify",
                &(
                    "Conflux",
                    0u32,
                    APP_ID,
                    summary,
                    body,
                    vec!["default", "Open"],
                    hints,
                    -1i32,
                ),
            )
            .await;
        match reply.map(|r| r.body().deserialize::<u32>()) {
            Ok(Ok(id)) => {
                let mut shown = self.shown.lock().unwrap();
                shown.opens.insert(id, label.to_string());
                Some(id)
            }
            Ok(Err(e)) => {
                debug!("odd answer to a notification: {e}");
                None
            }
            Err(e) => {
                debug!("could not notify: {e}");
                None
            }
        }
    }

    async fn close(self: &Arc<Self>, id: u32) {
        if let Some(bus) = self.bus().await {
            let _ = bus
                .call_method(
                    Some(SERVER),
                    SERVER_PATH,
                    Some(SERVER),
                    "CloseNotification",
                    &id,
                )
                .await;
        }
        self.forget(id);
    }

    fn forget(&self, id: u32) {
        let mut shown = self.shown.lock().unwrap();
        shown.opens.remove(&id);
        shown.tokens.remove(&id);
        for shown_id in shown.failing.values_mut() {
            if *shown_id == id {
                *shown_id = 0;
            }
        }
    }
}

/// Follow what happens to the notifications: clicks open the app at their
/// sync; closed ones are forgotten.
async fn listen(bus: Connection, notifier: Weak<Notifier>) {
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SERVER)
        .and_then(|b| b.path(SERVER_PATH))
        .map(|b| b.build());
    let stream = match rule {
        Ok(rule) => MessageStream::for_match_rule(rule, &bus, None).await,
        Err(e) => Err(e),
    };
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(e) => {
            debug!("cannot follow notifications: {e}");
            return;
        }
    };
    while let Some(message) = stream.next().await {
        // One unreadable message is no reason to stop following.
        let Ok(message) = message else {
            continue;
        };
        let Some(notifier) = notifier.upgrade() else {
            return;
        };
        let header = message.header();
        let body = message.body();
        match header.member().map(|m| m.as_str()) {
            Some("ActivationToken") => {
                if let Ok((id, token)) = body.deserialize::<(u32, String)>() {
                    let mut shown = notifier.shown.lock().unwrap();
                    if shown.opens.contains_key(&id) {
                        shown.tokens.insert(id, token);
                    }
                }
            }
            Some("ActionInvoked") => {
                let Ok((id, action)) = body.deserialize::<(u32, String)>() else {
                    continue;
                };
                let (label, token) = {
                    let mut shown = notifier.shown.lock().unwrap();
                    (shown.opens.get(&id).cloned(), shown.tokens.remove(&id))
                };
                if let Some(label) = label.filter(|_| action == "default") {
                    open_app(&bus, &label, token).await;
                }
            }
            Some("NotificationClosed") => {
                if let Ok((id, _reason)) = body.deserialize::<(u32, u32)>() {
                    notifier.forget(id);
                }
            }
            _ => {}
        }
    }
}

/// Open the app at the sync with `label`, starting it if need be.
async fn open_app(bus: &Connection, label: &str, token: Option<String>) {
    let mut platform: HashMap<&str, Value> = HashMap::new();
    if let Some(token) = token {
        platform.insert("activation-token", Value::from(token));
    }
    let result = bus
        .call_method(
            Some(APP_ID),
            APP_PATH,
            Some("org.freedesktop.Application"),
            "ActivateAction",
            &("show-sync", vec![Value::from(label)], platform),
        )
        .await;
    if let Err(e) = result {
        debug!("could not open the app: {e}");
    }
}

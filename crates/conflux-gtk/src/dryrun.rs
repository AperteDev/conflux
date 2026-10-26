//! The dry-run dialog: what the next sync of a group would change.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use conflux_core::engine::PlanOp;
use conflux_core::ipc::{client, Request, Response, SyncTarget};
use gtk::{gio, glib};

use crate::ui;
use crate::window::Ctx;

/// Ask the daemon on `socket` for a dry run of `label` and show it.
pub fn show(ctx: &Rc<Ctx>, socket: PathBuf, label: String, title: &str) {
    let stack = gtk::Stack::new();
    ui::replace_child(&stack, "status", &ui::loading("Looking for Changes"));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    let dialog = adw::Dialog::builder()
        .title(format!("Dry Run of {title}"))
        .content_width(600)
        .content_height(560)
        .child(&toolbar)
        .build();
    dialog.present(Some(&ctx.window));

    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            client::send(&socket, &Request::Plan(SyncTarget::Group(label)), None)
        })
        .await
        .expect("the dry-run request panicked");
        let plan = match result {
            Ok(Response::Planned(mut plans)) if !plans.is_empty() => {
                let plan = plans.remove(0);
                match (plan.plan, plan.error) {
                    (_, Some(e)) => Err(e),
                    (Some(p), None) => Ok(p),
                    (None, None) => Err("the daemon sent no plan".to_string()),
                }
            }
            Ok(Response::Error(e)) => Err(e),
            Ok(_) => Err("unexpected answer from the daemon".to_string()),
            Err(e) => Err(format!("could not reach the daemon: {e}")),
        };
        match plan {
            Err(e) => {
                let page = adw::StatusPage::builder()
                    .icon_name("dialog-error-symbolic")
                    .title("Dry Run Failed")
                    .description(glib::markup_escape_text(&e))
                    .build();
                ui::replace_child(&stack, "result", &page);
            }
            Ok(plan) if plan.is_empty() => {
                let page = adw::StatusPage::builder()
                    .icon_name("object-select-symbolic")
                    .title("No Changes")
                    .description("Syncing now would change nothing")
                    .build();
                ui::replace_child(&stack, "result", &page);
            }
            Ok(plan) => {
                let n = plan.changes.len();
                let group = adw::PreferencesGroup::builder()
                    .title(format!("{n} change{}", if n == 1 { "" } else { "s" }))
                    .description(
                        "Conflicts are predicted from modification times; a file found the \
                         same on both sides is left alone",
                    )
                    .build();
                for change in &plan.changes {
                    let row = ui::row(&change.path.to_string(), &change.op.describe());
                    row.set_title_selectable(true);
                    let (icon, class) = op_icon(&change.op);
                    row.add_prefix(&ui::state_icon(icon, class));
                    group.add(&row);
                }
                let page = adw::PreferencesPage::new();
                page.add(&group);
                ui::replace_child(&stack, "result", &page);
            }
        }
    });
}

fn op_icon(op: &PlanOp) -> (&'static str, &'static str) {
    match op.symbol() {
        '↑' => ("go-up-symbolic", "accent"),
        '↓' => ("go-down-symbolic", "accent"),
        '✗' => ("user-trash-symbolic", "error"),
        '!' => ("dialog-warning-symbolic", "warning"),
        _ => ("action-unavailable-symbolic", "dim-label"),
    }
}

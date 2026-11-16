//! Keep the desktop's activation token with the notification action that supplied it.
use futures_lite::StreamExt;
use std::collections::HashMap;
use zbus::{Connection, Proxy};

pub(super) fn show_notification(
    notification: &notify_rust::Notification,
    on_default: impl FnOnce(Option<String>),
) -> zbus::Result<()> {
    zbus::block_on(async {
        let connection = Connection::session().await?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        // Subscribe before showing: the user may click as soon as Notify returns.
        let mut signals = proxy.receive_all_signals().await?;
        let hints: HashMap<&str, zbus::wire::Variant<'_>> =
            notification.hints.iter().map(Into::into).collect();
        let id: u32 = proxy
            .call(
                "Notify",
                &(
                    &notification.appname,
                    0u32,
                    &notification.icon,
                    &notification.summary,
                    &notification.body,
                    &notification.actions,
                    hints,
                    i32::from(notification.timeout),
                ),
            )
            .await?;
        let mut token = None;
        while let Some(signal) = signals.next().await {
            match signal.header().member().map(|s| s.as_str()) {
                Some("ActivationToken") => {
                    let (notice, value): (u32, String) = signal.body().deserialize()?;
                    if notice == id {
                        token = Some(value);
                    }
                }
                Some("ActionInvoked") => {
                    let (notice, action): (u32, String) = signal.body().deserialize()?;
                    if notice == id {
                        if action == "default" {
                            on_default(token);
                        }
                        return Ok(());
                    }
                }
                Some("NotificationClosed") => {
                    let (notice, _): (u32, u32) = signal.body().deserialize()?;
                    if notice == id {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(zbus::Error::Failure(
            "notification service disconnected before an action or dismissal".into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Desktop(Arc<AtomicUsize>);

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Desktop {
        async fn notify(
            &self,
            _app: &str,
            _replace: u32,
            _icon: &str,
            summary: &str,
            _body: &str,
            _actions: Vec<String>,
            _hints: HashMap<String, zbus::OwnedVariant>,
            _timeout: i32,
            #[zbus(connection)] connection: &Connection,
            #[zbus(header)] header: zbus::message::Header<'_>,
        ) -> u32 {
            self.0.fetch_add(1, Ordering::SeqCst);
            let destination = header.sender().cloned();
            let path = "/org/freedesktop/Notifications";
            let interface = "org.freedesktop.Notifications";
            if summary == "closed" {
                connection
                    .emit_signal(
                        destination,
                        path,
                        interface,
                        "NotificationClosed",
                        &(7u32, 2u32),
                    )
                    .await
                    .unwrap();
            } else {
                if summary == "token" {
                    for (id, token) in [(8u32, "wrong-notification"), (7, "click-token")] {
                        connection
                            .emit_signal(
                                destination.clone(),
                                path,
                                interface,
                                "ActivationToken",
                                &(id, token),
                            )
                            .await
                            .unwrap();
                    }
                }
                for id in [8u32, 7] {
                    connection
                        .emit_signal(
                            destination.clone(),
                            path,
                            interface,
                            "ActionInvoked",
                            &(id, "default"),
                        )
                        .await
                        .unwrap();
                }
            }
            7
        }
    }

    #[test]
    #[ignore = "requires dbus-run-session; uses an isolated mock desktop, never the user's notification service"]
    fn desktop_actions_keep_tokens_and_ignore_other_notifications() {
        const TEST: &str = "notifications::desktop::tests::desktop_actions_keep_tokens_and_ignore_other_notifications";
        if std::env::var_os("MS_TEAMS_PRIVATE_NOTIFICATION_TEST").is_none() {
            let output = std::process::Command::new("dbus-run-session")
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", TEST])
                .env("MS_TEAMS_PRIVATE_NOTIFICATION_TEST", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let _service = zbus::block_on(
            zbus::connection::Builder::session()
                .name("org.freedesktop.Notifications")
                .serve_at("/org/freedesktop/Notifications", Desktop(calls.clone()))
                .build(),
        )
        .unwrap();
        for (summary, expected) in [
            ("token", Some(Some("click-token".to_string()))),
            ("no-token", Some(None)),
            ("closed", None),
        ] {
            let notification = super::super::message_notification(summary, "test");
            let mut response = None;
            show_notification(&notification, |token| response = Some(token)).unwrap();
            assert_eq!(response, expected);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "exactly one Notify call per message"
        );
    }
}

//! Windows toast notifications mirrored in the notch, via WinRT
//! `UserNotificationListener`. Windows only grants this to apps with package identity
//! (MSIX) after the user allows it in Settings › Privacy › Notifications; unpackaged
//! builds report `Access::Unavailable` and the feature stays off gracefully.

use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus as Status};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Allowed,
    Denied,
    /// Not asked yet (packaged app): call `request_access`.
    Unspecified,
    /// API unusable (no package identity / not supported).
    Unavailable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
    /// Unix seconds.
    pub at: i64,
}

fn listener() -> Option<UserNotificationListener> {
    UserNotificationListener::Current().ok()
}

pub fn access() -> Access {
    let Some(l) = listener() else { return Access::Unavailable };
    match l.GetAccessStatus() {
        Ok(Status::Allowed) => Access::Allowed,
        Ok(Status::Denied) => Access::Denied,
        Ok(_) => Access::Unspecified,
        Err(_) => Access::Unavailable,
    }
}

/// Ask Windows for permission (shows the system prompt for packaged apps).
pub fn request_access() -> Access {
    let Some(l) = listener() else { return Access::Unavailable };
    match l.RequestAccessAsync().ok().and_then(|op| op.get().ok()) {
        Some(Status::Allowed) => Access::Allowed,
        Some(Status::Denied) => Access::Denied,
        Some(_) => Access::Unspecified,
        None => Access::Unavailable,
    }
}

/// Current toasts in the action center (newest first). Blocking — call from a worker.
pub fn toasts() -> Vec<Toast> {
    let Some(l) = listener() else { return Vec::new() };
    let Some(list) = l.GetNotificationsAsync(NotificationKinds::Toast).ok().and_then(|op| op.get().ok()) else {
        return Vec::new();
    };
    let generic = KnownNotificationBindings::ToastGeneric().unwrap_or_default();
    let mut out: Vec<Toast> = list
        .into_iter()
        .filter_map(|n| {
            let id = n.Id().ok()?;
            let app = n.AppInfo().ok().and_then(|a| a.DisplayInfo().ok()).and_then(|d| d.DisplayName().ok()).map(|h| h.to_string()).unwrap_or_default();
            let binding = n.Notification().ok()?.Visual().ok()?.GetBinding(&generic).ok()?;
            let texts: Vec<String> = binding.GetTextElements().ok()?.into_iter().filter_map(|t| t.Text().ok()).map(|h| h.to_string()).collect();
            let at = n.CreationTime().map(|d| d.UniversalTime / 10_000_000 - 11_644_473_600).unwrap_or(0);
            Some(Toast {
                id,
                app,
                title: texts.first().cloned().unwrap_or_default(),
                body: texts.get(1..).map(|r| r.join(" · ")).unwrap_or_default(),
                at,
            })
        })
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at));
    out
}

pub fn dismiss(id: u32) {
    if let Some(l) = listener() {
        let _ = l.RemoveNotification(id);
    }
}

#[cfg(test)]
mod tests {
    /// Read-only: reports what this (unpackaged) build can do.
    #[test]
    fn reports_access() {
        let a = super::access();
        println!("notification listener access: {a:?}");
        if a == super::Access::Allowed {
            println!("{} toasts", super::toasts().len());
        }
    }
}

//! Per-user signal that new ops may have been stored.
//!
//! Only the fact of a change is sent; listeners read the ops themselves from
//! the store, so a slow listener can never miss or reorder any.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::watch;

use crate::store::UserId;

#[derive(Debug, Clone, Default)]
pub struct Notifier {
    channels: Arc<Mutex<HashMap<UserId, watch::Sender<()>>>>,
}

impl Notifier {
    /// A receiver that wakes on the next [`Self::notify`] for `user`, not on
    /// earlier ones.
    pub fn subscribe(&self, user: UserId) -> watch::Receiver<()> {
        self.channels()
            .entry(user)
            .or_insert_with(|| watch::channel(()).0)
            .subscribe()
    }

    pub fn notify(&self, user: UserId) {
        if let Some(sender) = self.channels().get(&user) {
            sender.send_replace(());
        }
    }

    /// The map stays consistent even if a holder panicked, so poisoning is
    /// ignored.
    fn channels(&self) -> MutexGuard<'_, HashMap<UserId, watch::Sender<()>>> {
        self.channels.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::watch;
    use tokio::time::timeout;

    use super::Notifier;
    use crate::store::UserId;

    const USER: UserId = UserId::DEFAULT;
    const OTHER: UserId = UserId(2);

    /// Long enough for a pending wake-up to be observed, short for the suite.
    const QUIET: Duration = Duration::from_millis(50);

    async fn woke(receiver: &mut watch::Receiver<()>) -> bool {
        timeout(QUIET, receiver.changed()).await.is_ok()
    }

    #[tokio::test]
    async fn wakes_subscribers_of_the_user() {
        let notifier = Notifier::default();
        let mut first = notifier.subscribe(USER);
        let mut second = notifier.subscribe(USER);

        notifier.notify(USER);

        assert!(woke(&mut first).await);
        assert!(woke(&mut second).await);
    }

    #[tokio::test]
    async fn does_not_wake_other_users() {
        let notifier = Notifier::default();
        let mut other = notifier.subscribe(OTHER);

        notifier.notify(USER);

        assert!(!woke(&mut other).await);
    }

    #[tokio::test]
    async fn ignores_notifications_before_subscribing() {
        let notifier = Notifier::default();
        notifier.subscribe(USER);
        notifier.notify(USER);

        let mut late = notifier.subscribe(USER);

        assert!(!woke(&mut late).await);
    }

    #[tokio::test]
    async fn notifying_without_subscribers_is_harmless() {
        let notifier = Notifier::default();

        notifier.notify(USER);

        let mut later = notifier.subscribe(USER);
        notifier.notify(USER);
        assert!(woke(&mut later).await);
    }

    #[tokio::test]
    async fn coalesces_notifications_until_seen() {
        let notifier = Notifier::default();
        let mut receiver = notifier.subscribe(USER);

        notifier.notify(USER);
        notifier.notify(USER);

        assert!(woke(&mut receiver).await);
        assert!(!woke(&mut receiver).await);
    }
}

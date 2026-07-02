use core::task::{Poll, Waker};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use miden_note_transport_proto::miden_note_transport::{StreamNotesUpdate, TransportNote};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::database::Database;
use crate::types::NoteTag;

/// Notes (proto) with pagination
pub type TransportNotesPg = (Vec<TransportNote>, u64);

/// Streaming handler
pub struct NoteStreamer {
    manager: NoteStreamerManager,
    rx: mpsc::Receiver<StreamerMessage>,
}

/// Streaming manager
///
/// Periodically queries new notes by note tag stored in the database and feeds them to relevant
/// subscribers.
struct NoteStreamerManager {
    /// Tracked tags
    tags: BTreeMap<NoteTag, TagData>,
    /// Sub wakers
    wakers: BTreeMap<u64, Waker>,
    /// Database
    database: Arc<Database>,
}

/// Internal control message exchanged with the [`NoteStreamer`]
pub(crate) enum StreamerMessage {
    /// New sub
    AddSub(Subface),
    /// Remove sub, tagged with the reason for the lifecycle log
    RemoveSub {
        id: u64,
        tag: NoteTag,
        reason: &'static str,
    },
    /// Update waker for sub
    Waker((u64, Waker)),
    /// Shutdown the streamer
    Shutdown,
}

/// Tag data tracking
pub struct TagData {
    /// Pagination cursor for this tag — the largest `seq` of notes already
    /// forwarded to subscribers. Next fetch uses this to query
    /// `seq > cursor` and pick up only new arrivals.
    cursor: u64,
    subs: BTreeMap<u64, SubEntry>,
}

/// A subscriber's send channel plus when it was registered, so the removal
/// event can report how long the subscription lived.
struct SubEntry {
    tx: mpsc::Sender<TransportNotesPg>,
    created_at: Instant,
}

/// Subscription
pub struct Sub {
    id: u64,
    tag: NoteTag,
    rx: mpsc::Receiver<TransportNotesPg>,
    streamer_tx: mpsc::Sender<StreamerMessage>,
}

/// Subscription interface
pub struct Subface {
    id: u64,
    tag: NoteTag,
    tx: mpsc::Sender<TransportNotesPg>,
}

impl NoteStreamerManager {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            tags: BTreeMap::new(),
            wakers: BTreeMap::new(),
            database,
        }
    }

    pub(super) async fn query_updates(&self) -> crate::Result<Vec<(NoteTag, TransportNotesPg)>> {
        // Update period
        sleep(Duration::from_millis(500)).await;

        let mut updates = vec![];
        for (tag, tag_data) in &self.tags {
            // Use the tuple-returning fetch so a reset (legacy µs cursor, or a
            // cursor stranded above the seq high-water after a DB recreation)
            // flows into the advanced cursor. Basing the advance on the effective
            // cursor — not `tag_data.cursor` — lets a stranded subscription heal
            // instead of re-sending its stranded cursor and re-triggering the
            // reset (and re-delivering the whole backlog) on every tick.
            let (snotes, effective_cursor) =
                self.database.fetch_notes_by_tags(&[*tag], tag_data.cursor).await?;
            let mut cursor = effective_cursor;
            for snote in &snotes {
                // Advance cursor using the DB-assigned monotonic `seq`
                // (matches the pull-side fetch_notes contract). Using
                // `created_at` here is what caused the original race.
                let lcursor: u64 = snote
                    .seq
                    .try_into()
                    .map_err(|_| tonic::Status::internal("Negative seq in stored note"))?;
                cursor = cursor.max(lcursor);
            }

            // Convert to protobuf format
            let pnotes = snotes.into_iter().map(TransportNote::from).collect::<Vec<_>>();
            let notespg = (pnotes, cursor);

            // Push when there are notes to forward, OR when the cursor changed
            // with no notes. The latter is a cursor-only heal: a stranded
            // subscription (cursor above the seq high-water after a DB
            // recreation) whose tag has no notes in the new epoch resets to
            // `effective = 0` every tick but, without this, would never persist
            // that reset — re-firing the reset (and its warn log + metric) on
            // every 500ms tick. Including it lets `update_timestamps` advance the
            // stored cursor once; `forward_updates` skips the empty batch so no
            // empty update reaches subscribers.
            if !notespg.0.is_empty() || cursor != tag_data.cursor {
                updates.push((*tag, notespg));
            }
        }

        Ok(updates)
    }

    pub(super) fn forward_updates(&mut self, tag_notes: Vec<(NoteTag, TransportNotesPg)>) {
        let mut remove_subs = vec![];
        // Forward updates to subs
        for (tag, notes) in tag_notes {
            // Cursor-only heal entries carry no notes; `update_timestamps` has
            // already advanced the stored cursor, and there is nothing to send.
            // The subscriber's waker stays in `self.wakers` (unconsumed) and is
            // woken when notes next arrive for the tag, or dropped on disconnect.
            if notes.0.is_empty() {
                continue;
            }
            if let Some(tag_data) = self.tags.get(&tag) {
                // Wake-up subs with `tag`
                for (sub_id, sub_entry) in &tag_data.subs {
                    if let Some(waker) = self.wakers.remove(sub_id) {
                        if let Ok(()) = sub_entry.tx.try_send(notes.clone()) {
                            waker.wake();
                        } else {
                            remove_subs.push((*sub_id, tag));
                        }
                    }
                }
            }
        }
        // Remove non-responding subs (backpressure)
        for (sub_id, tag) in remove_subs {
            self.remove_sub(sub_id, tag, "backpressure");
        }
    }

    pub(super) fn update_timestamps(&mut self, tag_notes: &[(NoteTag, TransportNotesPg)]) {
        // Update query cursors, to the cursor of the most recent note
        for (tag, notes) in tag_notes {
            if let Some(tag_data) = self.tags.get_mut(tag) {
                tag_data.cursor = notes.1;
            }
        }
    }

    pub fn update_waker(&mut self, sub_id: u64, waker: Waker) {
        self.wakers.insert(sub_id, waker);
    }

    pub fn add_sub(&mut self, sub: Subface) {
        let entry = self.tags.entry(sub.tag).or_insert_with(TagData::new);
        entry.subs.insert(sub.id, SubEntry { tx: sub.tx, created_at: Instant::now() });
        let active = self.tags.values().map(|td| td.subs.len()).sum::<usize>();
        tracing::info!(subscription_id = %sub.id, active_subscriptions = active, "Subscription added");
    }

    /// Removes a subscription and emits the single canonical removal event.
    ///
    /// Only logs when a sub was actually present: a sub evicted for
    /// backpressure is later dropped client-side too, and that second call
    /// must not emit a duplicate event or mislabel the reason.
    pub fn remove_sub(&mut self, sub_id: u64, tag: NoteTag, reason: &'static str) {
        let mut removed_at = None;
        let mut remove_tag = false;
        if let Some(tag_data) = self.tags.get_mut(&tag) {
            removed_at = tag_data.subs.remove(&sub_id).map(|entry| entry.created_at);
            remove_tag = tag_data.subs.is_empty();
        }
        if remove_tag {
            self.tags.remove(&tag);
        }
        if let Some(created_at) = removed_at {
            let active = self.tags.values().map(|td| td.subs.len()).sum::<usize>();
            tracing::info!(
                subscription_id = %sub_id,
                active_subscriptions = active,
                duration_secs = created_at.elapsed().as_secs(),
                reason,
                "Subscription removed"
            );
        }
    }
}

impl NoteStreamer {
    pub(crate) fn new(database: Arc<Database>, rx: mpsc::Receiver<StreamerMessage>) -> Self {
        Self {
            manager: NoteStreamerManager::new(database),
            rx,
        }
    }

    /// Streamer main loop
    pub(crate) async fn stream(self) {
        let mut manager = self.manager;
        let mut rx = self.rx;
        let mut enabled = true;
        while enabled {
            match Self::step(&mut manager, &mut rx).await {
                Ok(true) => (),
                Ok(false) => enabled = false,
                Err(e) => tracing::error!("Streamer error: {e}"),
            }
        }
    }

    /// Streamer loop step
    async fn step(
        manager: &mut NoteStreamerManager,
        rx: &mut mpsc::Receiver<StreamerMessage>,
    ) -> crate::Result<bool> {
        tokio::select! {
            // Periodically query DB for new notes
            res = manager.query_updates() => {
                let tag_notes = res?;
                manager.update_timestamps(&tag_notes);
                manager.forward_updates(tag_notes);
            }
            // Handle streamer control messages
            Some(msg) = rx.recv() => {
                match msg {
                    StreamerMessage::AddSub(sub) => manager.add_sub(sub),
                    StreamerMessage::RemoveSub { id, tag, reason } => {
                        manager.remove_sub(id, tag, reason);
                    },
                    StreamerMessage::Waker((id, waker)) => manager.update_waker(id, waker),
                    StreamerMessage::Shutdown => return Ok(false),
                }
            }
        }
        Ok(true)
    }
}

impl Sub {
    pub(crate) fn new(
        id: u64,
        tag: NoteTag,
        rx: mpsc::Receiver<TransportNotesPg>,
        streamer_tx: mpsc::Sender<StreamerMessage>,
    ) -> Self {
        Self { id, tag, rx, streamer_tx }
    }
}

impl Subface {
    pub fn new(id: u64, tag: NoteTag, tx: mpsc::Sender<TransportNotesPg>) -> Self {
        Self { id, tag, tx }
    }
}

impl TagData {
    pub fn new() -> Self {
        Self { cursor: 0, subs: BTreeMap::new() }
    }
}

impl tonic::codegen::tokio_stream::Stream for Sub {
    type Item = std::result::Result<StreamNotesUpdate, tonic::Status>;

    // Required method
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // Send update notes to client
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(pgnotes)) => {
                let (notes, cursor) = pgnotes;
                let updates = StreamNotesUpdate { notes, cursor };
                return Poll::Ready(Some(Ok(updates)));
            },
            Poll::Ready(None) => return Poll::Ready(None),
            _ => (),
        }

        // Update streamer' stored waker
        if let Err(e) =
            self.streamer_tx.try_send(StreamerMessage::Waker((self.id, cx.waker().clone())))
        {
            tracing::error!("Streaming waker tx failure: {e}");
            return Poll::Ready(None);
        }

        Poll::Pending
    }
}

impl Drop for Sub {
    fn drop(&mut self) {
        // Hand removal to the manager, which emits the single lifecycle event.
        // If the sub was already evicted (e.g. backpressure) this is a no-op
        // and logs nothing.
        if let Err(e) = self.streamer_tx.try_send(StreamerMessage::RemoveSub {
            id: self.id,
            tag: self.tag,
            reason: "client_disconnect",
        }) {
            tracing::error!(subscription_id = %self.id, error = %e, "Streamer remove sub control message sending error");
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::database::{Database, DatabaseConfig};
    use crate::metrics::Metrics;
    use crate::test_utils::{TAG_LOCAL_ANY, test_note_header_with_tag};
    use crate::types::StoredNote;

    /// A streaming subscription stranded above the seq high-water (its cursor
    /// carried over a DB recreation) whose tag has NO notes in the new epoch must
    /// heal its stored cursor to 0 exactly once — not re-fire the reset (warn +
    /// metric) on every 500ms tick — and must NOT push an empty update, which
    /// would deliver `cursor: 0` to the client and regress its position. The
    /// `forward_updates` empty-skip guard is load-bearing for that last property:
    /// with a waker registered, removing the guard would deliver the empty batch
    /// and fail this test.
    #[tokio::test]
    async fn test_streaming_heals_stranded_cursor_without_spurious_push() {
        let db = Arc::new(
            Database::connect(DatabaseConfig::default(), Metrics::default().db)
                .await
                .unwrap(),
        );

        // A note under a DIFFERENT tag: the shared seq high-water becomes 1,
        // while the subscribed tag has no notes in this epoch.
        db.store_note(&StoredNote {
            header: test_note_header_with_tag(0xc000_0001),
            details: vec![1],
            created_at: Utc::now(),
            seq: 0,
            after_block_num: None,
        })
        .await
        .unwrap();

        let mut mgr = NoteStreamerManager::new(db);

        // Register a subscriber on TAG_LOCAL_ANY with a stranded shared cursor,
        // plus a waker so `forward_updates` WOULD deliver if the empty-skip guard
        // were removed — making the "no spurious push" assertion meaningful.
        let tag: NoteTag = TAG_LOCAL_ANY.into();
        let (tx, mut rx) = mpsc::channel::<TransportNotesPg>(4);
        let mut subs = BTreeMap::new();
        subs.insert(1u64, SubEntry { tx, created_at: Instant::now() });
        mgr.tags.insert(tag, TagData { cursor: 10_000, subs });
        mgr.update_waker(1, Waker::noop().clone());

        // Tick 1: cursor (10_000) is above the high-water (1) → reset →
        // cursor-only heal entry (empty notes, cursor 0).
        let updates = mgr.query_updates().await.unwrap();
        assert_eq!(updates.len(), 1, "stranded empty tag must emit one cursor-only heal");
        assert!(updates[0].1.0.is_empty(), "heal carries no notes");
        assert_eq!(updates[0].1.1, 0, "heal cursor is the reset value 0");

        mgr.update_timestamps(&updates);
        mgr.forward_updates(updates);

        assert_eq!(mgr.tags.get(&tag).unwrap().cursor, 0, "stored cursor healed to 0 exactly once");
        assert!(rx.try_recv().is_err(), "no empty update may reach the subscriber");

        // Tick 2: caught-up at 0 — the `effective > 0` guard skips the high-water
        // check entirely, so no reset and no push. The storm is gone.
        assert!(
            mgr.query_updates().await.unwrap().is_empty(),
            "steady state after heal produces no push (no storm)",
        );
    }

    /// A stranded subscription whose tag DOES have notes in the new epoch heals
    /// and delivers those notes in a single tick: the reset re-scans from 0, the
    /// recovered notes are forwarded to the subscriber, and the stored cursor
    /// advances to their max seq (not 0, not the stranded value). Covers the
    /// heal + non-empty `forward_updates` + waker-consumption path.
    #[tokio::test]
    async fn test_streaming_heals_stranded_cursor_and_delivers_notes() {
        let db = Arc::new(
            Database::connect(DatabaseConfig::default(), Metrics::default().db)
                .await
                .unwrap(),
        );

        // Two notes on the SUBSCRIBED tag: the high-water becomes 2.
        for details in [vec![1u8], vec![2u8]] {
            db.store_note(&StoredNote {
                header: test_note_header_with_tag(TAG_LOCAL_ANY),
                details,
                created_at: Utc::now(),
                seq: 0,
                after_block_num: None,
            })
            .await
            .unwrap();
        }

        let mut mgr = NoteStreamerManager::new(db);
        let tag: NoteTag = TAG_LOCAL_ANY.into();
        let (tx, mut rx) = mpsc::channel::<TransportNotesPg>(4);
        let mut subs = BTreeMap::new();
        subs.insert(1u64, SubEntry { tx, created_at: Instant::now() });
        mgr.tags.insert(tag, TagData { cursor: 10_000, subs });
        mgr.update_waker(1, Waker::noop().clone());

        let updates = mgr.query_updates().await.unwrap();
        mgr.update_timestamps(&updates);
        mgr.forward_updates(updates);

        // The subscriber received the recovered notes (heal + delivery)...
        let (notes, cursor) = rx.try_recv().expect("healed notes must be delivered");
        assert_eq!(notes.len(), 2, "both new-epoch notes recovered");
        assert_eq!(cursor, 2, "delivered cursor is the max recovered seq");
        // ...and the stored cursor advanced to the max seq (not 0, not stranded).
        assert_eq!(mgr.tags.get(&tag).unwrap().cursor, 2, "stored cursor healed to max seq");
    }
}

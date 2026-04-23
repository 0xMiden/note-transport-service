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
    /// Remove sub
    RemoveSub((u64, NoteTag)),
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
    subs: BTreeMap<u64, mpsc::Sender<TransportNotesPg>>,
}

/// Subscription
pub struct Sub {
    id: u64,
    tag: NoteTag,
    rx: mpsc::Receiver<TransportNotesPg>,
    streamer_tx: mpsc::Sender<StreamerMessage>,
    created_at: Instant,
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
            let snotes = self.database.fetch_notes(*tag, tag_data.cursor).await?;
            let mut cursor = tag_data.cursor;
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

            if !notespg.0.is_empty() {
                updates.push((*tag, notespg));
            }
        }

        Ok(updates)
    }

    pub(super) fn forward_updates(&mut self, tag_notes: Vec<(NoteTag, TransportNotesPg)>) {
        let mut remove_subs = vec![];
        // Forward updates to subs
        for (tag, notes) in tag_notes {
            if let Some(tag_data) = self.tags.get(&tag) {
                // Wake-up subs with `tag`
                for (sub_id, sub_tx) in &tag_data.subs {
                    if let Some(waker) = self.wakers.remove(sub_id) {
                        if let Ok(()) = sub_tx.try_send(notes.clone()) {
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
            tracing::warn!(subscription_id = %sub_id, tag = tag.as_u32(), reason = "backpressure", "Dropping subscription");
            self.remove_sub(sub_id, tag);
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
        entry.subs.insert(sub.id, sub.tx);
        let active = self.tags.values().map(|td| td.subs.len()).sum::<usize>();
        tracing::info!(subscription_id = %sub.id, tag = sub.tag.as_u32(), active_subscriptions = active, "Subscription added");
    }

    pub fn remove_sub(&mut self, sub_id: u64, tag: NoteTag) {
        let mut remove_tag = false;
        if let Some(tag_data) = self.tags.get_mut(&tag) {
            tag_data.subs.remove(&sub_id);
            if tag_data.subs.is_empty() {
                remove_tag = true;
            }
        }
        if remove_tag {
            self.tags.remove(&tag);
        }
        let active = self.tags.values().map(|td| td.subs.len()).sum::<usize>();
        tracing::info!(subscription_id = %sub_id, tag = tag.as_u32(), active_subscriptions = active, "Subscription removed");
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
                    StreamerMessage::RemoveSub((id, tag)) => manager.remove_sub(id, tag),
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
        Self {
            id,
            tag,
            rx,
            streamer_tx,
            created_at: Instant::now(),
        }
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
        let duration_secs = self.created_at.elapsed().as_secs();
        tracing::info!(
            subscription_id = %self.id,
            tag = self.tag.as_u32(),
            duration_secs = duration_secs,
            reason = "client_disconnect",
            "Subscription dropped"
        );
        if let Err(e) = self.streamer_tx.try_send(StreamerMessage::RemoveSub((self.id, self.tag))) {
            tracing::error!(subscription_id = %self.id, tag = self.tag.as_u32(), "Streamer remove sub control message sending error: {e}");
        }
    }
}

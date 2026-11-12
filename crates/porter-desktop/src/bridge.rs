//! The engine side of the app: a tokio runtime created once, and per project window a
//! `Bridge` that runs stages and store queries on it and sends results back over a
//! channel. The window drains that channel on a GPUI task; nothing here touches GPUI.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

use porter_engine::destinations::payload::schema_check;
use porter_engine::destinations::payload::ts::read_payload_config;
use porter_engine::events::Event;
use porter_engine::mapping::edit::MappingDoc;
use porter_engine::mapping::summary::TypeSummary;
use porter_engine::report::{Coverage, read_coverage};
use porter_engine::session::{
    IssueQuery, MediaRow, Overview, Page, RunOutcome, RunPlan, STAGES, Session,
};
use porter_model::{ItemId, ItemKind, Resolution, SourceRef};
use tokio::runtime::Runtime;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio_util::sync::CancellationToken;

use crate::thumbs::{self, Cache};
use crate::vm::apply::{PreviewInfo, preview_info};
use crate::vm::content::{NodeKey, Query, Reply};
use crate::vm::pipeline::{KindCount, issues_by_kind};
use crate::vm::runs::History;
use crate::vm::{mapping, media, urls};

pub enum UiEvent {
    Engine(Event),
    /// The engine's event channel overflowed and this many events never arrived.
    Lagged(u64),
    Data(DataResult),
    RunDone(RunOutcome),
}

/// Store and file queries, run off the UI thread. Errors are kept verbatim.
pub enum DataResult {
    Overview(Result<Overview, String>),
    /// Open issues per kind.
    IssueKinds(Result<Vec<KindCount>, String>),
    /// What analyse found per connector; None before analyse has run.
    Coverage(Result<Option<Coverage>, String>),
    /// Runs per stage, newest first.
    History(Result<History, String>),
    /// The last dry run, for the apply sheet: to open it, or (`confirm`) the recheck
    /// when Apply was pressed.
    Preview {
        confirm: bool,
        /// `destination.url` as porter.toml says now.
        url: String,
        preview: Option<PreviewInfo>,
    },
    /// The content section's reads, tagged with the generation that asked.
    Content {
        generation: u64,
        reply: Reply,
    },
    Media {
        generation: u64,
        reply: media::Reply,
    },
    Urls {
        generation: u64,
        reply: urls::Reply,
    },
    /// A thumbnail landed in the cache: redraw.
    ThumbReady,
    /// porter.toml opened, or an edit saved (or refused), with the schema check's issues.
    Mapping(mapping::Reply),
}

/// The app's one tokio runtime (multi-thread), created on first use.
pub fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("porter-engine")
            .build()
            .expect("could not start the tokio runtime")
    })
}

pub struct Bridge {
    session: Arc<Session>,
    tx: Sender<UiEvent>,
    /// Run outcomes go through the event forwarder (see `new`).
    done: tokio::sync::mpsc::UnboundedSender<RunOutcome>,
    /// Cancels the run in flight, if any.
    cancel: Option<CancellationToken>,
}

impl Bridge {
    pub fn new(session: Arc<Session>) -> (Bridge, Receiver<UiEvent>) {
        let (tx, rx) = channel();
        let (done, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<RunOutcome>();
        let mut events = session.subscribe();
        let fwd = tx.clone();
        // One forwarder for engine events and run outcomes, so `RunDone` can never
        // overtake the events of the run it ends.
        runtime().spawn(async move {
            loop {
                let e = tokio::select! {
                    biased;
                    r = events.recv() => match r {
                        Ok(e) => UiEvent::Engine(e),
                        Err(RecvError::Lagged(n)) => UiEvent::Lagged(n),
                        Err(RecvError::Closed) => break,
                    },
                    Some(o) = done_rx.recv() => {
                        // The run has ended, so all its events are already queued.
                        loop {
                            let e = match events.try_recv() {
                                Ok(e) => UiEvent::Engine(e),
                                Err(TryRecvError::Lagged(n)) => UiEvent::Lagged(n),
                                Err(_) => break,
                            };
                            let _ = fwd.send(e);
                        }
                        UiEvent::RunDone(o)
                    }
                };
                // The window is gone.
                if fwd.send(e).is_err() {
                    break;
                }
            }
        });
        (
            Bridge {
                session,
                tx,
                done,
                cancel: None,
            },
            rx,
        )
    }

    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Starts `plan`. Always ends with one `RunDone`, also when the session refuses it.
    pub fn run(&mut self, plan: RunPlan) {
        let mine = CancellationToken::new();
        self.cancel = Some(mine.clone());
        let (session, done) = (self.session.clone(), self.done.clone());
        // Session::run reads porter.toml, so even starting happens off the UI thread.
        runtime().spawn(async move {
            let outcome = match session.run(plan) {
                Ok(h) => {
                    let token = h.cancel_token();
                    let mut wait = std::pin::pin!(h.wait());
                    tokio::select! {
                        o = &mut wait => o,
                        _ = mine.cancelled() => {
                            token.cancel();
                            wait.await
                        }
                    }
                }
                Err(e) => RunOutcome {
                    code: 1,
                    messages: vec![format!("{e:#}")],
                    ..Default::default()
                },
            };
            let _ = done.send(outcome);
        });
    }

    /// Asks the run in flight to stop at its next safe point (it ends resumable).
    pub fn cancel(&self) {
        if let Some(c) = &self.cancel {
            c.cancel();
        }
    }

    /// Re-reads what the window shows: overview, issues by kind, coverage, run history.
    pub fn refresh(&self) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            let send = |d| {
                let _ = tx.send(UiEvent::Data(d));
            };
            send(DataResult::Overview(
                session.overview().await.map_err(|e| format!("{e:#}")),
            ));
            // ponytail: loads every open issue to group them, as overview() does to count
            // them; a GROUP BY in the store when projects get huge.
            let open = IssueQuery {
                resolution: Some(Resolution::Open),
                ..Default::default()
            };
            let all = Page {
                offset: 0,
                limit: u64::MAX,
            };
            send(DataResult::IssueKinds(
                session
                    .issues(open, all)
                    .await
                    .map(|p| issues_by_kind(&p.rows))
                    .map_err(|e| format!("{e:#}")),
            ));
            let project = session.project().clone();
            let (coverage, history) = blocking(move || {
                let coverage = read_coverage(&project).map_err(|e| format!("{e:#}"));
                let history = STAGES
                    .iter()
                    .map(|&s| {
                        let mut runs = project.store().runs(s)?;
                        runs.reverse();
                        Ok((s, runs))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()
                    .map_err(|e| format!("{e:#}"));
                (coverage, history)
            })
            .await
            .unwrap_or_else(|e| (Err(e.clone()), Err(e)));
            send(DataResult::Coverage(coverage));
            send(DataResult::History(history));
        });
    }

    /// Reads the last dry run and whether it is still fresh.
    pub fn preview(&self, confirm: bool) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            // Re-read porter.toml so the sheet names the destination the load would use.
            let (url, preview) = blocking(move || {
                session.reload_config();
                (
                    session.config().destination.url.clone(),
                    preview_info(&session),
                )
            })
            .await
            .unwrap_or_default();
            let _ = tx.send(UiEvent::Data(DataResult::Preview {
                confirm,
                url,
                preview,
            }));
        });
    }
}

impl Bridge {
    /// Runs one content-section read; the reply comes back as `DataResult::Content`.
    pub fn content(&self, generation: u64, q: Query) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            let err = |e: anyhow::Error| format!("{e:#}");
            let reply = match q {
                Query::Types => Reply::Types(session.doc_types().await.map_err(err)),
                Query::Children { key, page } => {
                    let result = match &key {
                        NodeKey::Type(t) => session.children(None, Some(t), page).await,
                        NodeKey::Doc(id) => session.children(Some(*id), None, page).await,
                    };
                    Reply::Children {
                        key,
                        offset: page.offset,
                        result: result.map_err(err),
                    }
                }
                Query::Doc(id) => Reply::Doc(id, session.item(id).await.map(Box::new).map_err(err)),
                Query::Labels(ids) => Reply::Labels(read_labels(&session, ids).await),
            };
            let _ = tx.send(UiEvent::Data(DataResult::Content { generation, reply }));
        });
    }
}

impl Bridge {
    /// Runs one media-section read; the reply comes back as `DataResult::Media`.
    pub fn media(&self, generation: u64, q: media::Query) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            let reply = match q {
                media::Query::Page(page) => media::Reply::Page {
                    offset: page.offset,
                    result: session.media(page).await.map_err(|e| format!("{e:#}")),
                },
                media::Query::Detail(id) => media::Reply::Detail(
                    id,
                    session
                        .item(id)
                        .await
                        .map(Box::new)
                        .map_err(|e| format!("{e:#}")),
                ),
                media::Query::Labels(ids) => media::Reply::Labels(read_labels(&session, ids).await),
            };
            let _ = tx.send(UiEvent::Data(DataResult::Media { generation, reply }));
        });
    }

    /// Runs one URL-table read; the reply comes back as `DataResult::Urls`.
    pub fn urls(&self, generation: u64, q: urls::Query) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            let result = session
                .routes(q.filter, q.page)
                .await
                .map_err(|e| format!("{e:#}"));
            let reply = urls::Reply {
                offset: q.page.offset,
                result,
            };
            let _ = tx.send(UiEvent::Data(DataResult::Urls { generation, reply }));
        });
    }

    /// Decodes `row`'s thumbnail into `cache` unless it is there or on its way.
    pub fn thumb(&self, cache: &Arc<Cache>, row: &MediaRow) {
        if !cache.begin(&row.hash) {
            return;
        }
        // At most two decodes at once, so a fast scroll queues instead of starting
        // hundreds of threads and holding hundreds of decoded frames.
        // ponytail: scrolled-off requests still decode; a cancel flag per request if it shows.
        static DECODING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
        let (session, tx, cache, row) = (
            self.session.clone(),
            self.tx.clone(),
            cache.clone(),
            row.clone(),
        );
        runtime().spawn(async move {
            let _permit = DECODING.acquire().await;
            let (c, r) = (cache.clone(), row.clone());
            let done = blocking(move || {
                let t = match session.project().store().media_path(r.id) {
                    Ok(path) => thumbs::decode(&path, &r.mime, &r.filename),
                    Err(e) => thumbs::Thumb::Placeholder(format!("can't preview ({e:#})")),
                };
                c.put(&r.hash, t);
            })
            .await;
            if let Err(e) = done {
                cache.put(
                    &row.hash,
                    thumbs::Thumb::Placeholder(format!("can't preview ({e})")),
                );
            }
            let _ = tx.send(UiEvent::Data(DataResult::ThumbReady));
        });
    }
}

impl Bridge {
    /// Opens porter.toml for the mapping editor (dropping unsaved edits) and checks it.
    pub fn mapping_open(&self) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            let reply = blocking(move || {
                let path = session.project().dir().join("porter.toml");
                mapping::Reply::Loaded(
                    MappingDoc::open(&path)
                        .map_err(|e| format!("{e:#}"))
                        .and_then(|doc| mapping_check(&session, doc)),
                )
            })
            .await
            .unwrap_or_else(|e| mapping::Reply::Loaded(Err(e)));
            let _ = tx.send(UiEvent::Data(DataResult::Mapping(reply)));
        });
    }

    /// Saves an edited document; once saved, runs the schema check again.
    pub fn mapping_save(&self, mut doc: MappingDoc) {
        let (session, tx) = (self.session.clone(), self.tx.clone());
        runtime().spawn(async move {
            let reply = blocking(move || {
                let refused = doc.save();
                if !refused.is_empty() {
                    return mapping::Reply::Refused(refused);
                }
                session.reload_config();
                mapping::Reply::Loaded(mapping_check(&session, doc))
            })
            .await
            .unwrap_or_else(|e| mapping::Reply::Refused(vec![e]));
            let _ = tx.send(UiEvent::Data(DataResult::Mapping(reply)));
        });
    }
}

/// `porter schema check` without the run: offline, nothing written to the store. The
/// destination schema is read only when `destination.config_dir` is set.
// ponytail: re-reads the store summary and the Payload config on every save; cache them
// keyed by store run / file mtimes if large projects make saves slow.
fn mapping_check(session: &Session, doc: MappingDoc) -> Result<Box<mapping::Loaded>, String> {
    let err = |e: anyhow::Error| format!("{e:#}");
    let config = doc.config().map_err(err)?;
    let project = session.project();
    let summary = TypeSummary::from_store(project.store()).map_err(err)?;
    let dest = config
        .destination
        .config_dir
        .as_ref()
        .map(|d| read_payload_config(&project.dir().join(d)))
        .transpose()
        .map_err(err);
    let issues = match &dest {
        Ok(Some(d)) => schema_check::check(&summary, &config.mapping, d),
        _ => vec![],
    };
    Ok(Box::new(mapping::Loaded {
        doc,
        summary,
        dest,
        issues,
    }))
}

/// Titles of documents and filenames of media for `ids`; None where there is no live item.
async fn read_labels(
    session: &Arc<Session>,
    ids: Vec<ItemId>,
) -> Result<Vec<(ItemId, Option<String>)>, String> {
    let project = session.project().clone();
    blocking(move || {
        ids.into_iter()
            .map(|id| {
                let found = project.store().item_source(id)?;
                Ok((id, found.map(|(kind, src, data)| label(kind, &src, &data))))
            })
            .collect::<anyhow::Result<Vec<_>>>()
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .and_then(|r| r)
}

/// A media item's filename, else a document's title, else its source id.
fn label(kind: ItemKind, src: &SourceRef, data: &serde_json::Value) -> String {
    let key = if kind == ItemKind::Media {
        "filename"
    } else {
        "title"
    };
    match data.get(key).and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => format!("(untitled: {})", src.source_id),
    }
}

/// Runs blocking file/SQLite work on tokio's blocking pool; a panic there comes back as
/// an error message.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// A closed window stops its run: the engine ends it at a safe point, resumable.
impl Drop for Bridge {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Keeps only the newest `Progress` per stage in a drained batch; everything else, and
/// the order, is untouched. Progress is a gauge, so older readings are worthless.
pub fn coalesce(batch: Vec<UiEvent>) -> Vec<UiEvent> {
    let mut seen = Vec::new();
    let mut out: Vec<UiEvent> = batch
        .into_iter()
        .rev()
        .filter(|e| match e {
            UiEvent::Engine(Event::Progress { stage, .. }) => {
                let newer = seen.contains(stage);
                seen.push(*stage);
                !newer
            }
            _ => true,
        })
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_engine::events::Summary;
    use porter_model::Stage;

    fn progress(stage: Stage, done: u64) -> UiEvent {
        UiEvent::Engine(Event::Progress {
            stage,
            done,
            total: Some(100),
            label: "items".into(),
        })
    }

    fn done_of(e: &UiEvent) -> Option<(Stage, u64)> {
        match e {
            UiEvent::Engine(Event::Progress { stage, done, .. }) => Some((*stage, *done)),
            _ => None,
        }
    }

    #[test]
    fn coalesces_progress() {
        let finished = UiEvent::Engine(Event::StageFinished {
            stage: Stage::Extract,
            run: porter_engine::store::RunId(uuid::Uuid::nil()),
            summary: Summary::default(),
        });
        let batch = vec![
            progress(Stage::Extract, 1),
            UiEvent::Lagged(3),
            progress(Stage::Extract, 2),
            progress(Stage::Analyse, 7),
            progress(Stage::Extract, 3),
            finished,
        ];
        let out = coalesce(batch);
        let prog: Vec<_> = out.iter().filter_map(done_of).collect();
        assert_eq!(prog, [(Stage::Analyse, 7), (Stage::Extract, 3)]);
        // Non-progress events all survive, in order, and the last reading stays before
        // the stage's end.
        assert_eq!(out.len(), 4);
        assert!(matches!(out[0], UiEvent::Lagged(3)));
        assert!(matches!(
            out[3],
            UiEvent::Engine(Event::StageFinished { .. })
        ));
    }

    /// The real engine on the app runtime: a run started from a plain (non-tokio) thread
    /// reports its events and ends with exactly one `RunDone`.
    #[test]
    fn run_reports_events_and_done() {
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let session = Arc::new(Session::open(d.path()).unwrap());
        let (mut bridge, rx) = Bridge::new(session);
        bridge.run(RunPlan::Stage(
            Stage::Normalise,
            porter_engine::session::StageOpts {
                refresh: false,
                apply: false,
                schema_mode: porter_engine::stage::SchemaMode::Check,
            },
        ));
        let mut started = false;
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                UiEvent::Engine(Event::StageStarted { .. }) => started = true,
                UiEvent::RunDone(o) => {
                    assert!(
                        started,
                        "RunDone before StageStarted: {:?} {:?}",
                        o.messages, o.per_stage
                    );
                    break;
                }
                _ => {}
            }
        }
        bridge.refresh();
        let got = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert!(matches!(got, UiEvent::Data(DataResult::Overview(Ok(_)))));
    }

    /// The mapping editor's save path on the real engine, off the UI thread: open, save
    /// an edit (re-checked), and a refusal when porter.toml changed on disk.
    #[test]
    fn mapping_open_save_refuse() {
        use crate::vm::mapping::{Mapping, NO_SCHEMA, Picker};
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let session = Arc::new(Session::open(d.path()).unwrap());
        let (bridge, rx) = Bridge::new(session);
        let next = || loop {
            if let UiEvent::Data(DataResult::Mapping(r)) =
                rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap()
            {
                return r;
            }
        };
        let mut m = Mapping::default();
        assert!(m.open());
        bridge.mapping_open();
        m.land(next());
        assert_eq!(m.error(), None);
        assert_eq!(m.collection_picker(), Picker::FreeText(NO_SCHEMA.into()));
        let doc = m.edit(|d| d.set_type_to("post", Some("posts"))).unwrap();
        bridge.mapping_save(doc);
        m.land(next());
        assert!(m.messages().is_empty(), "{:?}", m.messages());
        assert_eq!(m.type_rows()[0].to.as_deref(), Some("posts"));
        let file = d.path().join("porter.toml");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("to = \"posts\""));
        std::fs::write(&file, format!("{text}# hand edit\n")).unwrap();
        let doc = m.edit(|d| d.set_path("post", Some("/{slug}/"))).unwrap();
        bridge.mapping_save(doc);
        m.land(next());
        assert!(m.needs_reload());
        assert_eq!(m.type_rows()[0].path, None, "refused edit undone");
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .ends_with("# hand edit\n")
        );
    }
}

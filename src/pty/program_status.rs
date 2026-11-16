//! OSC 7501 program status (rev 0.4,
//! <https://www.superlogical.com/rex/docs/build/program-status>): records
//! programs report about themselves, one per id.

use crate::process::Pid;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Answer to `OSC 7501 ; ?`, without the introducer and terminator.
pub const QUERY_REPLY: &[u8] =
    b"7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth";

const MAX_SEQUENCE: usize = 4096;
/// `ESC ] 7501 ;` and `ESC \`, the longer terminator.
const FRAMING: usize = 9;
const MAX_KEY: usize = 16;
const MAX_APP: usize = 32;
const MAX_ID: usize = 128;
const MAX_SEGMENT: usize = 32;
const MAX_DEPTH: usize = 8;
const MAX_RECORDS: usize = 256;

/// Padded or not.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "ProgramState"))]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "ProgramKind"))]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "ProgramRecord"))]
pub struct Record {
    /// Empty for the root record.
    pub id: String,
    pub state: State,
    pub kind: Option<Kind>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
    /// The foreground process group when reported: the attached process.
    pub group: Option<Pid>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Report {
    Set(Record),
    /// Removes the id and its subtree; `None` removes every record.
    Clear(Option<String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Report(Report),
    /// OSC 133 A: a new shell prompt.
    Prompt,
    /// RIS.
    Reset,
}

/// What the pane's records say overall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub state: State,
    pub kind: Option<Kind>,
    pub app: Option<String>,
    /// The record's message, else its title.
    pub text: Option<String>,
}

/// Parses the data after `7501;`. `None` for the query and for reports the
/// spec says to ignore.
pub fn parse(data: &[u8]) -> Option<Report> {
    if data.len() + FRAMING > MAX_SEQUENCE {
        return None;
    }
    let data = std::str::from_utf8(data).ok()?;
    let (mut state, mut id, mut kind, mut progress, mut app, mut title, mut msg) =
        (None, None, None, None, None, None, None);
    for pair in data.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || key.len() > MAX_KEY || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
        {
            continue;
        }
        match key {
            "state" => state = Some(value),
            "id" => id = Some(value),
            "kind" => kind = Some(value),
            "progress" => progress = Some(value),
            "app" => app = Some(value),
            "title" => title = Some(value),
            "msg" => msg = Some(value),
            _ => {}
        }
    }
    let id = match id {
        Some(id) => {
            valid_id(id).then_some(())?;
            id
        }
        None => "",
    };
    let state = match state? {
        "idle" => State::Idle,
        "working" => State::Working,
        "done" => State::Done,
        "blocked" => State::Blocked,
        "error" => State::Error,
        "clear" => return Some(Report::Clear((!id.is_empty()).then(|| id.to_string()))),
        _ => return None,
    };
    if app.is_some_and(|app| app.len() > MAX_APP) {
        return None;
    }
    let kind = kind
        .filter(|_| state == State::Blocked)
        .and_then(|kind| match kind {
            "permission" => Some(Kind::Permission),
            "question" => Some(Kind::Question),
            "auth" => Some(Kind::Auth),
            _ => None,
        });
    let progress = progress
        .filter(|_| matches!(state, State::Working | State::Blocked))
        .and_then(|p| p.parse::<u8>().ok())
        .filter(|p| *p <= 100);
    Some(Report::Set(Record {
        id: id.to_string(),
        state,
        kind,
        progress,
        app: app.filter(|app| !app.is_empty()).map(str::to_string),
        title: decode(title, 256, 192)?,
        msg: decode(msg, 2732, 2048)?,
        group: None,
    }))
}

fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID
        && id.split('/').count() <= MAX_DEPTH
        && id.split('/').all(|segment| {
            !segment.is_empty()
                && segment.len() <= MAX_SEGMENT
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        })
}

/// Base64 text, padded or not. `None` rejects the report; `Some(None)` is no text.
fn decode(value: Option<&str>, encoded: usize, decoded: usize) -> Option<Option<String>> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Some(None);
    };
    if value.len() > encoded {
        return None;
    }
    let bytes = BASE64.decode(value).ok()?;
    if bytes.len() > decoded {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    // Invisible formatting could disguise the text.
    let text: String = text.chars().filter(|c| !is_invisible(*c)).collect();
    let text = text.trim();
    Some((!text.is_empty()).then(|| text.to_string()))
}

fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}')
}

/// A pane's records, least recently updated first.
#[derive(Default)]
pub struct ProgramStatus {
    records: Vec<Record>,
    revision: u64,
    synced: u64,
    /// A report set a record since the last `take_change`.
    reported: bool,
}

/// Records changed since the last `take_change`.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub summary: Option<Summary>,
    /// A program reported, rather than records only ending.
    pub reported: bool,
}

impl ProgramStatus {
    /// Records handed over by an upgrade; the first `take_change` reports them.
    pub fn restore(records: Vec<Record>) -> Self {
        Self {
            revision: u64::from(!records.is_empty()),
            reported: !records.is_empty(),
            records,
            synced: 0,
        }
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn apply(&mut self, event: Event) -> bool {
        let before = self.records.len();
        let changed = match event {
            // Even an unchanged record is a report: it makes the record the
            // most recent, and the program's status outranks others again.
            Event::Report(Report::Set(record)) => {
                if let Some(index) = self.records.iter().position(|r| r.id == record.id) {
                    self.records.remove(index);
                } else if self.records.len() >= MAX_RECORDS {
                    self.records.remove(0);
                }
                self.records.push(record);
                self.reported = true;
                true
            }
            Event::Report(Report::Clear(None)) | Event::Reset => {
                self.records.clear();
                before > 0
            }
            Event::Report(Report::Clear(Some(id))) => {
                self.records.retain(|r| !in_subtree(&r.id, &id));
                self.records.len() != before
            }
            Event::Prompt => {
                self.records.retain(|r| !transient(r.state));
                self.records.len() != before
            }
        };
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// Drops transient records whose process group is gone.
    pub fn drop_exited(&mut self, alive: impl Fn(Pid) -> bool) -> bool {
        let before = self.records.len();
        self.records.retain(|r| {
            !transient(r.state) || r.group.is_none_or(|group| group <= 0 || alive(group))
        });
        let changed = self.records.len() != before;
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// Drops done and error records: their results were seen, or nothing
    /// shows them. Not a change.
    pub fn forget_results(&mut self) {
        self.records.retain(|r| transient(r.state));
    }

    /// Changed since the last `take_change`.
    pub fn pending(&self) -> bool {
        self.synced != self.revision
    }

    pub fn take_change(&mut self) -> Option<Change> {
        if self.synced == self.revision {
            return None;
        }
        self.synced = self.revision;
        Some(Change {
            summary: self.summary(),
            reported: std::mem::take(&mut self.reported),
        })
    }

    /// The record most in need of attention: blocked, then working, failed,
    /// done, and idle; among equals, the most recently updated.
    pub fn summary(&self) -> Option<Summary> {
        let rank = |state| match state {
            State::Blocked => 4,
            State::Working => 3,
            State::Error => 2,
            State::Done => 1,
            State::Idle => 0,
        };
        let (_, record) = self
            .records
            .iter()
            .enumerate()
            .max_by_key(|(order, r)| (rank(r.state), *order))?;
        // `app` is inherited from the nearest ancestor.
        let app = record.app.clone().or_else(|| {
            let mut id = record.id.as_str();
            while !id.is_empty() {
                id = id.rsplit_once('/').map_or("", |(parent, _)| parent);
                let parent = self.records.iter().find(|r| r.id == id);
                if let Some(app) = parent.and_then(|r| r.app.clone()) {
                    return Some(app);
                }
            }
            None
        });
        Some(Summary {
            state: record.state,
            kind: record.kind,
            app,
            text: record.msg.clone().or_else(|| record.title.clone()),
        })
    }
}

/// Dropped when the program exits or a prompt starts; done and error stay.
pub fn transient(state: State) -> bool {
    matches!(state, State::Idle | State::Working | State::Blocked)
}

fn in_subtree(id: &str, root: &str) -> bool {
    id.strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    fn set(data: &str) -> Record {
        match parse(data.as_bytes()) {
            Some(Report::Set(record)) => record,
            other => panic!("{data}: {other:?}"),
        }
    }

    #[test]
    fn parses_reports_as_the_spec_says() {
        let record = set(&format!(
            " state = blocked : kind=permission:progress=40:app=claude-code:msg={}:bogus:x=1:Upper=1",
            b64("Run rm?")
        ));
        assert_eq!(record.state, State::Blocked);
        assert_eq!(record.kind, Some(Kind::Permission));
        assert_eq!(record.progress, Some(40));
        assert_eq!(record.app.as_deref(), Some("claude-code"));
        assert_eq!(record.msg.as_deref(), Some("Run rm?"));
        assert_eq!(record.id, "");
        // The last value wins; kind and progress only go with some states.
        let record = set("state=idle:state=done:kind=question:progress=5:id=a/b-1");
        assert_eq!(
            (
                record.state,
                record.kind,
                record.progress,
                record.id.as_str()
            ),
            (State::Done, None, None, "a/b-1")
        );
        assert_eq!(set("state=working:progress=101").progress, None);
        assert_eq!(
            parse(b"state=clear:id=a"),
            Some(Report::Clear(Some("a".into())))
        );
        assert_eq!(parse(b"state=clear"), Some(Report::Clear(None)));
    }

    #[test]
    fn rejects_what_the_spec_ignores() {
        let control = format!("state=done:msg={}", b64("a\x1b[2Jb"));
        let long_app = format!("state=done:app={}", "a".repeat(33));
        let long_msg = format!("state=done:msg={}", b64(&"a".repeat(2049)));
        let long_segment = format!("state=done:id={}", "a".repeat(33));
        let deep = format!("state=done:id={}", ["a"; 9].join("/"));
        let huge = format!("state=done:x={}", "a".repeat(MAX_SEQUENCE));
        for data in [
            "?",
            "msg=aGk=",
            "state=paused",
            "state=done:msg=a",
            "state=done:id=a//b",
            "state=done:id=",
            &long_segment,
            &control,
            &long_app,
            &long_msg,
            &deep,
            &huge,
        ] {
            assert_eq!(parse(data.as_bytes()), None, "{data}");
        }
        assert_eq!(set("state=done:msg=aGk").msg.as_deref(), Some("hi"));
        // The cap counts the longer terminator.
        let at_cap = format!("state=done:x={}", "a".repeat(MAX_SEQUENCE - FRAMING - 13));
        assert!(parse(at_cap.as_bytes()).is_some());
        assert_eq!(parse(format!("{at_cap}a").as_bytes()), None);
        // Invisible formatting is removed.
        let record = set(&format!("state=done:title={}", b64("a\u{202e}b\u{200b}")));
        assert_eq!(record.title.as_deref(), Some("ab"));
    }

    fn report(store: &mut ProgramStatus, data: &str, group: Option<Pid>) -> bool {
        let mut record = set(data);
        record.group = group;
        store.apply(Event::Report(Report::Set(record)))
    }

    #[test]
    fn records_follow_their_lifetime() {
        let mut store = ProgramStatus::default();
        assert!(report(&mut store, "state=working:app=deploy", Some(10)));
        assert!(report(
            &mut store,
            "state=blocked:id=eu:kind=auth",
            Some(10)
        ));
        assert!(report(&mut store, "state=done:id=us", Some(10)));
        assert!(report(&mut store, "state=done:id=us", Some(10)));
        let summary = store.summary().unwrap();
        assert_eq!(summary.state, State::Blocked);
        assert_eq!(summary.app.as_deref(), Some("deploy"));
        let change = Change {
            summary: Some(summary),
            reported: true,
        };
        assert_eq!(store.take_change(), Some(change));
        assert_eq!(store.take_change(), None);

        // The process exits: done and error stay.
        assert!(!store.drop_exited(|_| true));
        assert!(store.drop_exited(|_| false));
        let change = store.take_change().unwrap();
        assert_eq!(change.summary.unwrap().state, State::Done);
        assert!(!change.reported);
        assert!(report(&mut store, "state=working:id=us/x", Some(11)));
        assert!(store.apply(Event::Prompt));
        assert_eq!(store.records().len(), 1);
        // Seen results go, without a change.
        report(&mut store, "state=working:id=w", Some(11));
        store.take_change();
        store.forget_results();
        let ids: Vec<_> = store.records().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["w"]);
        assert!(!store.pending());
        assert!(store.apply(Event::Reset));
        let change = store.take_change().unwrap();
        assert_eq!(change.summary, None);
        assert!(!change.reported);

        // Clearing takes the subtree, not siblings that share a prefix.
        for id in ["a", "a/b", "a/b/c", "ab"] {
            report(&mut store, &format!("state=idle:id={id}"), None);
        }
        assert!(store.apply(Event::Report(Report::Clear(Some("a/b".into())))));
        let ids: Vec<_> = store.records().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "ab"]);
        // Groups that can't be checked keep their records.
        report(&mut store, "state=working:id=g", Some(-4));
        assert!(!store.drop_exited(|_| false));
    }

    #[test]
    fn the_least_recently_updated_record_makes_room() {
        let mut store = ProgramStatus::default();
        for n in 0..=MAX_RECORDS {
            report(&mut store, &format!("state=working:id=r{n}"), None);
        }
        assert_eq!(store.records().len(), MAX_RECORDS);
        assert_eq!(store.records()[0].id, "r1");
        // Ties go to the newest.
        report(&mut store, "state=working:id=r5:title=TmV3", None);
        assert_eq!(store.summary().unwrap().text.as_deref(), Some("New"));
    }

    #[test]
    fn repeated_reports_and_ancestors_change_the_summary() {
        let text = |change: Option<Change>| change.and_then(|c| c.summary?.text);
        let mut store = ProgramStatus::default();
        report(&mut store, "state=blocked:id=a:msg=QQ", None);
        report(&mut store, "state=blocked:id=b:msg=Qg", None);
        assert_eq!(text(store.take_change()).as_deref(), Some("B"));
        // An identical report makes `a` the most recent.
        assert!(report(&mut store, "state=blocked:id=a:msg=QQ", None));
        assert_eq!(text(store.take_change()).as_deref(), Some("A"));
        // And it is reported again, to take over from other sources.
        assert!(report(&mut store, "state=blocked:id=a:msg=QQ", None));
        assert!(store.take_change().unwrap().reported);
        assert_eq!(store.take_change(), None);

        // A child inherits its parent's current app.
        report(&mut store, "state=working:app=build", None);
        report(&mut store, "state=blocked:id=a:msg=QQ", None);
        report(&mut store, "state=working:app=deploy", None);
        let app = store.take_change().and_then(|c| c.summary?.app);
        assert_eq!(app.as_deref(), Some("deploy"));
    }
}

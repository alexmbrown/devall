//! The to-do data model (schema version 2): tasks, lists, sections, labels, saved filters and
//! settings, all in one personal file in the app data folder.

use chrono::{NaiveDate, NaiveTime, Weekday};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Current on-disk schema version of `todos/todos.json`.
pub const SCHEMA_VERSION: u32 = 2;
/// Id of the Inbox list (global, not tied to a project).
pub const INBOX: &str = "inbox";

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// Todoist-style priority: `1` is the most urgent (p1), `4` is the default (no priority).
/// Serialized as a number; also reads `"p1"`…`"p4"` and the v1 words (`high`/`normal`/`low`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    P1 = 1,
    P2 = 2,
    P3 = 3,
    #[default]
    P4 = 4,
}

impl Priority {
    pub fn from_number(n: u64) -> Option<Self> {
        Some(match n {
            1 => Self::P1,
            2 => Self::P2,
            3 => Self::P3,
            4 => Self::P4,
            _ => return None,
        })
    }

    /// `p1`…`p4` (case-insensitive), `!!1`…`!!4`, or a bare digit.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        let digits = s.strip_prefix('p').or_else(|| s.strip_prefix("!!")).unwrap_or(&s);
        digits.parse::<u64>().ok().and_then(Self::from_number)
    }

    pub fn number(self) -> u8 {
        self as u8
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "p{}", self.number())
    }
}

impl Serialize for Priority {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.number())
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            N(u64),
            S(String),
        }
        let bad = || serde::de::Error::custom("priority must be 1-4 or p1-p4");
        match Raw::deserialize(d)? {
            Raw::N(n) => Priority::from_number(n).ok_or_else(bad),
            Raw::S(s) => match s.to_ascii_lowercase().as_str() {
                "high" => Ok(Priority::P1),
                "normal" | "low" | "" => Ok(Priority::P4),
                other => Priority::parse(other).ok_or_else(bad),
            },
        }
    }
}

/// A weekday in recurrence rules, serialized as `mon`…`sun`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl From<Weekday> for Day {
    fn from(w: Weekday) -> Self {
        match w {
            Weekday::Mon => Day::Mon,
            Weekday::Tue => Day::Tue,
            Weekday::Wed => Day::Wed,
            Weekday::Thu => Day::Thu,
            Weekday::Fri => Day::Fri,
            Weekday::Sat => Day::Sat,
            Weekday::Sun => Day::Sun,
        }
    }
}

impl From<Day> for Weekday {
    fn from(d: Day) -> Self {
        match d {
            Day::Mon => Weekday::Mon,
            Day::Tue => Weekday::Tue,
            Day::Wed => Weekday::Wed,
            Day::Thu => Weekday::Thu,
            Day::Fri => Weekday::Fri,
            Day::Sat => Weekday::Sat,
            Day::Sun => Weekday::Sun,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Freq {
    Day,
    Week,
    Month,
    Year,
}

fn one() -> u32 {
    1
}
fn is_one(n: &u32) -> bool {
    *n == 1
}

/// A recurrence rule. Occurrences are computed from the task's current due date (the anchor),
/// so moving a recurring task moves the series, as in Todoist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recurrence {
    pub freq: Freq,
    /// Every N days/weeks/months/years.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub interval: u32,
    /// Weekly: the weekdays (empty = the due date's weekday). Monthly with `weekOfMonth`: one day.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<Day>,
    /// Monthly/yearly: days of the month, `-1` = the last day. Days past the month's end clamp to
    /// its last day (`every 31st` is Feb 28 in February).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub month_days: Vec<i8>,
    /// Monthly: `1`…`4` or `-1` (last) together with one weekday — "every 2nd tue", "every last fri".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_of_month: Option<i8>,
    /// Yearly: month 1-12 (empty = the due date's month).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    /// Todoist's `every!`: the next date counts from the completion date, not the due date.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_completion: bool,
    /// Last possible occurrence (`until`/`ending`).
    #[serde(default, with = "opt_ymd", skip_serializing_if = "Option::is_none")]
    pub until: Option<NaiveDate>,
}

impl Recurrence {
    pub fn new(freq: Freq) -> Self {
        Self { freq, interval: 1, weekdays: Vec::new(), month_days: Vec::new(), week_of_month: None, month: None, from_completion: false, until: None }
    }
}

/// When a task is due. `date`/`time` are wall-clock values in `timezone`, or in the machine's local
/// time when `timezone` is empty ("floating", the default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Due {
    #[serde(with = "ymd")]
    pub date: NaiveDate,
    #[serde(default, with = "opt_hm", skip_serializing_if = "Option::is_none")]
    pub time: Option<NaiveTime>,
    /// IANA zone (`Europe/Paris`) for a fixed-time task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    /// The natural-language text the rule was typed as (`every mon 9am`), for recurring dates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub string: String,
}

impl Due {
    pub fn on(date: NaiveDate) -> Self {
        Self { date, time: None, timezone: None, recurrence: None, string: String::new() }
    }

    pub fn is_recurring(&self) -> bool {
        self.recurrence.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub text: String,
    /// Unix ms.
    pub at: i64,
}

/// A file attached to a task. The bytes are a copy kept on this device in
/// `<app data>/todos/attachments/<task id>/<name>` (never in a project); see [`super::attachments`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    /// The stored file name (sanitized, unique among the task's attachments).
    pub name: String,
    pub mime: String,
    /// Bytes.
    pub size: u64,
    /// Unix ms.
    pub added_at: i64,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }
}

/// One completion of a recurring task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    /// Unix ms.
    pub at: i64,
    /// The occurrence that was completed.
    #[serde(with = "ymd")]
    pub due: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    /// Title (one line, inline Markdown).
    pub content: String,
    /// Markdown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub list_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    /// Parent task (sub-tasks share their parent's list and section).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Position among siblings (same list, section and parent).
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<Due>,
    /// Hard deadline (date only), independent of the planned `due` date.
    #[serde(default, with = "opt_ymd", skip_serializing_if = "Option::is_none")]
    pub deadline: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Things this task is about: `http/users/list.req.yaml`, `notes/design/auth.md`, URLs…
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
    /// Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms.
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// Files attached to the task (stored in the app data folder, see [`Attachment`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Completions of a recurring task (newest last, capped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Completion>,
    /// Sub-tasks folded in the list.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    /// Board column (workflow status) id, see [`TodoSettings::columns`]. `None` (and unknown ids)
    /// = the first column. Completed tasks are always in the done column; for them this is the
    /// column they come back to when reopened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Position in its board column (manual order across lists; ties fall back to list order).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub status_order: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ListKind {
    /// The global Inbox.
    Inbox,
    /// The personal list of a /dev/all project (id = project id).
    Project,
    /// A list the user created ("project" in Todoist terms).
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub kind: ListKind,
    /// Palette name (`red`, `orange`, `yellow`, `green`, `teal`, `blue`, `purple`, `pink`, `gray`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub order: i64,
    /// Project lists: the project folder at the last visit (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: String,
    pub list_id: String,
    pub name: String,
    #[serde(default)]
    pub order: i64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFilter {
    pub id: String,
    pub name: String,
    /// Filter language, e.g. `today | overdue`, `p1 & @bug`, `#Work & !@later`.
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub order: i64,
}

fn yes() -> bool {
    true
}

/// A board column: one workflow status (`To do`, `In progress`, `Done`…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusColumn {
    /// Stable id (tasks refer to it); empty when the UI adds a column (one is generated).
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Palette name, like list colours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Work-in-progress limit (open cards); shown as `n/limit` on the board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wip_limit: Option<u32>,
    /// The done column (exactly one): moving a task into it completes the task.
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
}

impl StatusColumn {
    pub fn new(id: &str, name: &str, color: Option<&str>, done: bool) -> Self {
        Self { id: id.into(), name: name.into(), color: color.map(str::to_string), wip_limit: None, done }
    }
}

/// `To do`, `In progress`, `Done`.
pub fn default_columns() -> Vec<StatusColumn> {
    vec![StatusColumn::new("todo", "To do", None, false), StatusColumn::new("in-progress", "In progress", Some("blue"), false), StatusColumn::new("done", "Done", Some("green"), true)]
}

/// Reminder preferences (reminders fire only while the desktop app is running) and the board's
/// status columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoSettings {
    /// Desktop notification when a task with a time is due.
    #[serde(default = "yes")]
    pub reminders: bool,
    /// An extra reminder this many minutes before the due time (0 = none).
    #[serde(default)]
    pub remind_before_minutes: u32,
    /// `HH:MM`: also remind about tasks due that day without a time (empty = off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_day_reminder: Option<String>,
    /// Board columns (workflow statuses), in order: exactly one is the done column; the first
    /// other column is where new and unstatused tasks land. Changed only through
    /// `TodoOp::SaveColumns` (which reassigns tasks), never by `SaveSettings`.
    #[serde(default = "default_columns")]
    pub columns: Vec<StatusColumn>,
}

impl Default for TodoSettings {
    fn default() -> Self {
        Self { reminders: true, remind_before_minutes: 0, all_day_reminder: None, columns: default_columns() }
    }
}

impl TodoSettings {
    /// Where new and unstatused tasks land: the first column that isn't the done column.
    pub fn first_column(&self) -> Option<&StatusColumn> {
        self.columns.iter().find(|c| !c.done)
    }

    pub fn done_column(&self) -> Option<&StatusColumn> {
        self.columns.iter().find(|c| c.done)
    }

    /// The column a task is in: the done column when completed, else its status (unknown or
    /// missing → the first column).
    pub fn status_of(&self, t: &Task) -> Option<&StatusColumn> {
        if t.done {
            return self.done_column();
        }
        t.status.as_deref().and_then(|s| self.columns.iter().find(|c| c.id == s && !c.done)).or_else(|| self.first_column())
    }

    /// A column by id, else by name (case-insensitive).
    pub fn column(&self, key: &str) -> Option<&StatusColumn> {
        let k = key.trim();
        self.columns.iter().find(|c| c.id == k).or_else(|| self.column_named(k))
    }

    /// A column by name (case-insensitive, surrounding spaces ignored).
    pub fn column_named(&self, name: &str) -> Option<&StatusColumn> {
        let n = name.trim().to_lowercase();
        self.columns.iter().find(|c| c.name.to_lowercase() == n)
    }

    /// The column names, for error messages (`To do, In progress, Done`).
    pub fn column_names(&self) -> String {
        self.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}

/// The whole personal to-do file (`<app data>/todos/todos.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoData {
    pub version: u32,
    #[serde(default)]
    pub lists: Vec<TaskList>,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub filters: Vec<SavedFilter>,
    #[serde(default)]
    pub settings: TodoSettings,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

impl Default for TodoData {
    fn default() -> Self {
        Self { version: SCHEMA_VERSION, lists: Vec::new(), sections: Vec::new(), labels: Vec::new(), filters: Vec::new(), settings: TodoSettings::default(), tasks: Vec::new() }
    }
}

pub(crate) mod ymd {
    use chrono::NaiveDate;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &NaiveDate, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.format("%Y-%m-%d").to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NaiveDate, D::Error> {
        let s = String::deserialize(d)?;
        NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| serde::de::Error::custom(format!("expected a YYYY-MM-DD date, got `{s}`")))
    }
}

pub(crate) mod opt_ymd {
    use chrono::NaiveDate;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Option<NaiveDate>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => super::ymd::serialize(d, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<NaiveDate>, D::Error> {
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(s) if s.trim().is_empty() => Ok(None),
            Some(s) => NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map(Some).map_err(|_| serde::de::Error::custom(format!("expected a YYYY-MM-DD date, got `{s}`"))),
        }
    }
}

pub(crate) mod opt_hm {
    use chrono::NaiveTime;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<NaiveTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&t.format("%H:%M").to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<NaiveTime>, D::Error> {
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(s) if s.trim().is_empty() => Ok(None),
            Some(s) => NaiveTime::parse_from_str(s.trim(), "%H:%M")
                .or_else(|_| NaiveTime::parse_from_str(s.trim(), "%H:%M:%S"))
                .map(Some)
                .map_err(|_| serde::de::Error::custom(format!("expected an HH:MM time, got `{s}`"))),
        }
    }
}

/// Distinguishes a missing field (`None`) from an explicit `null` (`Some(None)`) in patches.
pub(crate) fn tri<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_serde() {
        assert_eq!(serde_json::to_string(&Priority::P1).unwrap(), "1");
        for (raw, p) in [("1", Priority::P1), ("\"p2\"", Priority::P2), ("\"P3\"", Priority::P3), ("4", Priority::P4), ("\"high\"", Priority::P1), ("\"normal\"", Priority::P4)] {
            assert_eq!(serde_json::from_str::<Priority>(raw).unwrap(), p, "{raw}");
        }
        assert!(serde_json::from_str::<Priority>("5").is_err());
        assert!(serde_json::from_str::<Priority>("\"urgent\"").is_err());
        assert_eq!(Priority::parse("!!2"), Some(Priority::P2));
        assert!(Priority::P1 < Priority::P4);
    }

    #[test]
    fn due_serde_round_trip() {
        let mut r = Recurrence::new(Freq::Week);
        r.weekdays = vec![Day::Mon, Day::Wed];
        r.until = NaiveDate::from_ymd_opt(2027, 1, 1);
        let due = Due { date: NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(), time: NaiveTime::from_hms_opt(9, 0, 0), timezone: None, recurrence: Some(r), string: "every mon, wed 9am".into() };
        let json = serde_json::to_string(&due).unwrap();
        assert_eq!(json, r#"{"date":"2026-10-26","time":"09:00","recurrence":{"freq":"week","weekdays":["mon","wed"],"until":"2027-01-01"},"string":"every mon, wed 9am"}"#);
        assert_eq!(serde_json::from_str::<Due>(&json).unwrap(), due);
        assert!(serde_json::from_str::<Due>(r#"{"date":"26/10/2026"}"#).is_err());
        let d: Due = serde_json::from_str(r#"{"date":"2026-10-26","time":"17:30:00"}"#).unwrap();
        assert_eq!(d.time, NaiveTime::from_hms_opt(17, 30, 0));
    }
}

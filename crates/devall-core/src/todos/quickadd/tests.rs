use chrono::{NaiveDate, NaiveTime};

use super::*;
use crate::todos::model::{Freq, ListKind};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

struct Fixture {
    lists: Vec<TaskList>,
    sections: Vec<Section>,
    labels: Vec<Label>,
}

fn fixture() -> Fixture {
    let list = |id: &str, name: &str, kind| TaskList { id: id.into(), name: name.into(), kind, color: None, order: 0, project_root: None };
    let section = |id: &str, list_id: &str, name: &str| Section { id: id.into(), list_id: list_id.into(), name: name.into(), order: 0, collapsed: false };
    Fixture {
        lists: vec![list("inbox", "Inbox", ListKind::Inbox), list("L1", "Work", ListKind::User), list("L2", "Side project", ListKind::User), list("L3", "Side", ListKind::User), list("P1", "Shop API", ListKind::Project)],
        sections: vec![section("S1", "L1", "Backend"), section("S2", "L1", "Frontend"), section("S3", "P1", "Bugs"), section("S4", "P1", "Tech debt")],
        labels: vec![Label { name: "api".into(), color: None }, Label { name: "Urgent".into(), color: Some("red".into()) }],
    }
}

fn parse_with(text: &str, default_list: Option<&str>, ignore: &[&str]) -> QuickAdd {
    let f = fixture();
    let ignore: Vec<String> = ignore.iter().map(|s| s.to_string()).collect();
    let ctx = ParseCtx {
        dates: DateCtx::new(d(2026, 10, 23), NaiveTime::from_hms_opt(14, 30, 0).unwrap()),
        lists: &f.lists,
        sections: &f.sections,
        labels: &f.labels,
        default_list,
        ignore: &ignore,
    };
    parse(text, &ctx)
}

fn p(text: &str) -> QuickAdd {
    parse_with(text, Some("inbox"), &[])
}

#[test]
fn everything_at_once() {
    let q = p("Ship refunds tomorrow 5pm #Work /Backend @api @urgent p1 {fri}");
    assert_eq!(q.content, "Ship refunds");
    let due = q.due.unwrap();
    assert_eq!((due.date, due.time), (d(2026, 10, 24), NaiveTime::from_hms_opt(17, 0, 0)));
    assert_eq!(q.list_id.as_deref(), Some("L1"));
    assert_eq!(q.section_id.as_deref(), Some("S1"));
    assert_eq!(q.labels, ["api", "Urgent"]);
    assert!(q.new_labels.is_empty());
    assert_eq!(q.priority, Some(Priority::P1));
    assert_eq!(q.deadline, Some(d(2026, 10, 30)));
    let kinds: Vec<TokenKind> = q.tokens.iter().map(|t| t.kind).collect();
    assert_eq!(kinds, [TokenKind::Date, TokenKind::List, TokenKind::Section, TokenKind::Label, TokenKind::Label, TokenKind::Priority, TokenKind::Deadline]);
    assert_eq!(q.tokens[0].text, "tomorrow 5pm");
    assert_eq!(q.tokens[0].value, "2026-10-24");
    assert_eq!(q.tokens[1].text, "#Work");
    assert_eq!(q.tokens[6].text, "{fri}");
}

#[test]
fn token_offsets_are_utf16() {
    let text = "🚀 Launch tomorrow";
    let q = p(text);
    let t = &q.tokens[0];
    assert_eq!((t.start, t.end), (10, 18));
    let utf16: Vec<u16> = text.encode_utf16().collect();
    assert_eq!(String::from_utf16(&utf16[t.start..t.end]).unwrap(), "tomorrow");
    assert_eq!(q.content, "🚀 Launch");
}

#[test]
fn lists_and_sections() {
    // Longest list name wins; quoting works; unknown names stay text.
    let q = p("#Side project fix login");
    assert_eq!((q.list_id.as_deref(), q.content.as_str()), (Some("L2"), "fix login"));
    let q = p("#Side fix login");
    assert_eq!(q.list_id.as_deref(), Some("L3"));
    let q = p("fix #\"side project\" login");
    assert_eq!((q.list_id.as_deref(), q.content.as_str()), (Some("L2"), "fix login"));
    let q = p("#Unknown thing");
    assert_eq!((q.list_id, q.content.as_str()), (None, "#Unknown thing"));
    let q = p("Fix issue #123 now");
    assert_eq!((q.list_id, q.content.as_str()), (None, "Fix issue #123 now"));
    let q = p("#inbox tidy up");
    assert_eq!(q.list_id.as_deref(), Some("inbox"));
    // Sections are looked up in the target list.
    let q = parse_with("/Bugs crash on save", Some("P1"), &[]);
    assert_eq!((q.section_id.as_deref(), q.content.as_str()), (Some("S3"), "crash on save"));
    let q = parse_with("/tech debt remove v1 api", Some("P1"), &[]);
    assert_eq!((q.section_id.as_deref(), q.content.as_str()), (Some("S4"), "remove v1 api"));
    let q = parse_with("/Bugs x #Work", Some("P1"), &[]);
    assert_eq!((q.list_id.as_deref(), q.section_id, q.content.as_str()), (Some("L1"), None, "/Bugs x"));
    let q = p("#Work /frontend polish");
    assert_eq!(q.section_id.as_deref(), Some("S2"));
    // Slashes inside words are not sections.
    let q = parse_with("Update the http/Bugs endpoint", Some("P1"), &[]);
    assert_eq!((q.section_id, q.content.as_str()), (None, "Update the http/Bugs endpoint"));
}

#[test]
fn labels_and_priorities() {
    let q = p("Email bob@example.com");
    assert!(q.labels.is_empty());
    assert_eq!(q.content, "Email bob@example.com");
    let q = p("@new-label task @API.");
    assert_eq!(q.labels, ["new-label", "api"]);
    assert_eq!(q.new_labels, ["new-label"]);
    assert!(q.tokens[0].new && !q.tokens[1].new);
    assert_eq!(q.content, "task.");
    let q = p("p1 p3 task");
    assert_eq!((q.priority, q.content.as_str()), (Some(Priority::P3), "task"));
    let q = p("Task P2");
    assert_eq!(q.priority, Some(Priority::P2));
    let q = p("Task !!1");
    assert_eq!(q.priority, Some(Priority::P1));
    let q = p("Task p5 pizza p12");
    assert_eq!((q.priority, q.content.as_str()), (None, "Task p5 pizza p12"));
}

#[test]
fn dates_in_text() {
    let q = p("Call mom tomorrow.");
    assert_eq!((q.content.as_str(), q.due.unwrap().date), ("Call mom.", d(2026, 10, 24)));
    let q = p("Pay rent every month on the 1st");
    let due = q.due.unwrap();
    assert_eq!(q.content, "Pay rent");
    assert_eq!((due.date, due.recurrence.as_ref().unwrap().freq, due.string.as_str()), (d(2026, 11, 1), Freq::Month, "every month on the 1st"));
    let q = p("Standup every weekday at 9:30am #Work");
    assert_eq!((q.content.as_str(), q.list_id.as_deref()), ("Standup", Some("L1")));
    assert_eq!(q.due.unwrap().time, NaiveTime::from_hms_opt(9, 30, 0));
    let q = p("every! 3 days water plants");
    assert!(q.due.unwrap().recurrence.unwrap().from_completion);
    assert_eq!(q.content, "water plants");
    let q = p("Review PR in 2 hours");
    assert_eq!(q.due.unwrap().time, NaiveTime::from_hms_opt(16, 30, 0));
    let q = p("Deploy {jan 30} by fri");
    assert_eq!((q.content.as_str(), q.deadline, q.due.unwrap().date), ("Deploy", Some(d(2027, 1, 30)), d(2026, 10, 30)));
    let q = p("Plan meeting no date");
    assert!(q.no_date && q.due.is_none());
    assert_eq!(q.content, "Plan meeting");
    // Not dates.
    for text in ["Fix 2 bugs", "Prepare morning standup", "Write today's report", "Fix bug in parser", "Meet at 5", "Read chapter 3"] {
        let q = p(text);
        assert!(q.due.is_none(), "{text}");
        assert_eq!(q.content, text);
    }
    // Only the first date expression is used.
    let q = p("Move standup from mon to fri");
    assert_eq!(q.due.unwrap().date, d(2026, 10, 26));
    // A date can't span a sigil.
    let q = p("tomorrow @api 5pm");
    assert_eq!(q.due.unwrap().time, None);
}

#[test]
fn ignored_tokens_stay_in_the_text() {
    let q = p("Buy sun cream");
    assert_eq!((q.content.as_str(), q.due.map(|d| d.date)), ("Buy cream", Some(d(2026, 10, 25))));
    let q = parse_with("Buy sun cream", Some("inbox"), &["SUN"]);
    assert_eq!((q.content.as_str(), q.due), ("Buy sun cream", None));
    let q = parse_with("Fix #Work thing p1", Some("inbox"), &["#work", "p1"]);
    assert_eq!((q.content.as_str(), q.list_id, q.priority), ("Fix #Work thing p1", None, None));
}

#[test]
fn empty_and_whitespace() {
    let q = p("");
    assert_eq!(q, QuickAdd::default());
    let q = p("   tomorrow   ");
    assert_eq!(q.content, "");
    assert!(q.due.is_some());
    let q = p("Buy milk, tomorrow");
    assert_eq!(q.content, "Buy milk");
}

use chrono::{NaiveDate, NaiveTime};

use super::*;
use crate::todos::model::{Due, Recurrence, Freq};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}
fn ctx() -> DateCtx {
    DateCtx::new(d(2026, 10, 23), NaiveTime::from_hms_opt(14, 30, 0).unwrap())
}

fn task(id: &str, content: &str) -> Task {
    Task {
        id: id.into(),
        content: content.into(),
        description: String::new(),
        list_id: "inbox".into(),
        section_id: None,
        parent_id: None,
        order: 0,
        priority: Priority::P4,
        due: None,
        deadline: None,
        labels: vec![],
        links: vec![],
        done: false,
        completed_at: None,
        created_at: 1_790_000_000_000,
        updated_at: 0,
        comments: vec![],
        history: vec![],
        collapsed: false,
    }
}

fn tasks() -> Vec<Task> {
    let mut a = task("a", "Pay invoice");
    a.due = Some(Due::on(d(2026, 10, 23)));
    a.priority = Priority::P1;
    a.labels = vec!["bug".into()];
    a.list_id = "work".into();
    let mut b = task("b", "Old thing");
    b.due = Some(Due::on(d(2026, 10, 20)));
    b.labels = vec!["later".into(), "review-me".into()];
    b.list_id = "work".into();
    b.section_id = Some("s1".into());
    let mut c = task("c", "Someday");
    c.description = "Check the INVOICE totals".into();
    let mut e = task("e", "Standup");
    let mut due = Due::on(d(2026, 10, 26));
    due.recurrence = Some(Recurrence::new(Freq::Week));
    due.time = NaiveTime::from_hms_opt(9, 0, 0);
    e.due = Some(due);
    e.priority = Priority::P2;
    e.deadline = Some(d(2026, 10, 30));
    e.list_id = "side".into();
    let mut f = task("f", "Subtask");
    f.parent_id = Some("a".into());
    f.due = Some(Due::on(d(2027, 1, 15)));
    f.created_at = 1_700_000_000_000;
    vec![a, b, c, e, f]
}

fn run(q: &str) -> Vec<Vec<String>> {
    let query = parse(q, &ctx()).unwrap_or_else(|e| panic!("{q}: {e}"));
    let ts = tasks();
    let list_name = |id: &str| Some(match id { "work" => "Work", "side" => "Side project", _ => "Inbox" }.to_string());
    let section_name = |id: &str| (id == "s1").then(|| "Backend".to_string());
    let due_date = |t: &Task| t.due.as_ref().map(|d| d.date);
    let created = |t: &Task| chrono::DateTime::from_timestamp_millis(t.created_at).unwrap().date_naive();
    let cx = EvalCtx { today: d(2026, 10, 23), list_name: &list_name, section_name: &section_name, due_date: &due_date, created: &created };
    query.groups.iter().map(|(_, e)| ts.iter().filter(|t| e.matches(t, &cx)).map(|t| t.id.clone()).collect()).collect()
}

fn one(q: &str) -> Vec<String> {
    run(q).remove(0)
}

#[test]
fn terms() {
    assert_eq!(one("today | overdue"), ["a", "b"]);
    assert_eq!(one("today|od"), ["a", "b"]);
    assert_eq!(one("p1 & @bug"), ["a"]);
    assert_eq!(one("#Work & !@later"), ["a"]);
    assert_eq!(one("#work"), ["a", "b"]);
    assert_eq!(one("#side*"), ["e"]);
    assert_eq!(one("#\"Side project\""), ["e"]);
    assert_eq!(one("due before: next week"), ["a", "b"]);
    assert_eq!(one("due after: today"), ["e", "f"]);
    assert_eq!(one("due: mon"), ["e"]);
    assert_eq!(one("jan 15"), ["f"]);
    assert_eq!(one("no date"), ["c"]);
    assert_eq!(one("!no date"), ["a", "b", "e", "f"]);
    assert_eq!(one("search: invoice"), ["a", "c"]);
    assert_eq!(one("@review*"), ["b"]);
    assert_eq!(one("@*me"), ["b"]);
    assert_eq!(one("(p1 | p2) & 7 days"), ["a", "e"]);
    assert_eq!(one("next 3 days"), ["a"]);
    assert_eq!(one("4 days"), ["a", "e"]);
    assert_eq!(one("/backend"), ["b"]);
    assert_eq!(one("no section & #work"), ["a"]);
    assert_eq!(one("recurring"), ["e"]);
    assert_eq!(one("no time & recurring"), Vec::<String>::new());
    assert_eq!(one("subtask"), ["f"]);
    assert_eq!(one("no labels"), ["c", "e", "f"]);
    assert_eq!(one("no priority & !subtask"), ["b", "c"]);
    assert_eq!(one("deadline before: nov 1"), ["e"]);
    assert_eq!(one("no deadline & p2"), Vec::<String>::new());
    assert_eq!(one("created before: 2025-01-01"), ["f"]);
    assert_eq!(one("created after: 2025-01-01 & no date"), ["c"]);
    assert_eq!(one("all"), ["a", "b", "c", "e", "f"]);
    assert_eq!(one("tomorrow"), Vec::<String>::new());
    assert_eq!(one("!(today | overdue) & !no date"), ["e", "f"]);
    assert_eq!(one("p1 | p2 & @bug"), ["a"]);
}

#[test]
fn groups_and_escapes() {
    let q = parse("overdue, today , p2", &ctx()).unwrap();
    let titles: Vec<&str> = q.groups.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["overdue", "today", "p2"]);
    assert_eq!(run("overdue, today, p2"), [vec!["b"], vec!["a"], vec!["e"]]);
    let q = parse(r"search: a\, b \& c", &ctx()).unwrap();
    assert_eq!(q.groups[0].1, Expr::Term(Term::Search("a, b & c".into())));
    // Commas inside parentheses don't split groups.
    assert!(parse("(today, p1)", &ctx()).is_err());
}

#[test]
fn errors() {
    for q in ["", "  ", "p1 &", "(today", "today)", "foo", "today | | p1", "search:", "due before: blah", "p5", "unknown: x", "today p1"] {
        assert!(parse(q, &ctx()).is_err(), "{q} should fail");
    }
    assert!(parse("foo", &ctx()).unwrap_err().contains("unknown filter `foo`"));
}

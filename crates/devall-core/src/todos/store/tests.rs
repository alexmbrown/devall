use chrono::{NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Europe::Paris;
use serde_json::json;

use super::*;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}
fn t(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap()
}
/// Friday 2026-10-23 14:30 in Paris.
fn now() -> Now {
    Now { ms: Paris.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap().timestamp_millis(), ctx: DateCtx::new(d(2026, 10, 23), t(14, 30)) }
}
fn later(days: i64) -> Now {
    let n = now();
    Now { ms: n.ms + days * 86_400_000, ctx: DateCtx::new(n.ctx.today + chrono::Duration::days(days), n.ctx.now) }
}
fn project() -> ProjectRef {
    ProjectRef::new("01PROJECT", "Shop API", "/work/shop")
}

fn store() -> TodoData {
    let mut s = TodoData::new_store();
    s.ensure_lists(Some(&project()));
    s
}

fn add(s: &mut TodoData, text: &str, list: &str) -> String {
    s.apply(TodoOp::Add { task: NewTask::new(text).in_list(list) }, Some(&project()), &now()).unwrap().created.unwrap()
}

fn apply(s: &mut TodoData, op: serde_json::Value) -> Applied {
    let op: TodoOp = serde_json::from_value(op).unwrap();
    s.apply(op, Some(&project()), &now()).unwrap()
}

#[test]
fn fresh_store_has_inbox_project_list_and_example_filters() {
    let s = store();
    let ids: Vec<&str> = s.lists.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, [INBOX, "01PROJECT"]);
    assert_eq!(s.lists[1].kind, ListKind::Project);
    assert_eq!(s.lists[1].project_root.as_deref(), Some("/work/shop"));
    assert_eq!(s.filters.len(), 2);
    // Renamed projects update their list.
    let mut s = s;
    assert!(s.ensure_lists(Some(&ProjectRef::new("01PROJECT", "Shop", "/work/shop"))));
    assert_eq!(s.lists[1].name, "Shop");
    assert!(!s.ensure_lists(Some(&ProjectRef::new("01PROJECT", "Shop", "/work/shop"))));
}

#[test]
fn quick_add_creates_labels_and_resolves_lists() {
    let mut s = store();
    let work = apply(&mut s, json!({"type": "saveList", "name": "Work", "color": "Blue"})).created.unwrap();
    assert_eq!(s.list(&work).unwrap().color.as_deref(), Some("blue"));
    let backend = apply(&mut s, json!({"type": "saveSection", "listId": work, "name": "Backend"})).created.unwrap();
    let id = add(&mut s, "Ship refunds tomorrow 5pm #Work /Backend @api p1 {fri}", INBOX);
    let task = s.task(&id).unwrap();
    assert_eq!(task.content, "Ship refunds");
    assert_eq!((task.list_id.as_str(), task.section_id.as_deref()), (work.as_str(), Some(backend.as_str())));
    assert_eq!(task.priority, Priority::P1);
    assert_eq!(task.due.as_ref().map(|d| (d.date, d.time)), Some((d(2026, 10, 24), Some(t(17, 0)))));
    assert_eq!(task.deadline, Some(d(2026, 10, 30)));
    assert_eq!(task.labels, ["api"]);
    assert!(s.labels.iter().any(|l| l.name == "api"));
    // Defaults from the caller when the text has none; `parse: false` keeps the text verbatim.
    let id = apply(&mut s, json!({"type": "add", "task": {"text": "Call #Work tomorrow", "parse": false, "listId": "01PROJECT", "priority": 2, "dueString": "next mon", "labels": ["@Api"], "links": ["http/a.req.yaml", "http/a.req.yaml"]}})).created.unwrap();
    let task = s.task(&id).unwrap();
    assert_eq!((task.content.as_str(), task.list_id.as_str(), task.priority), ("Call #Work tomorrow", "01PROJECT", Priority::P2));
    assert_eq!(task.due.as_ref().unwrap().date, d(2026, 10, 26));
    assert_eq!(task.labels, ["api"]);
    assert_eq!(task.links, ["http/a.req.yaml"]);
    // No list given: the open project's list, else the Inbox.
    let id = s.apply(TodoOp::Add { task: NewTask::new("About this request") }, Some(&project()), &now()).unwrap().created.unwrap();
    assert_eq!(s.task(&id).unwrap().list_id, "01PROJECT");
    let id = s.apply(TodoOp::Add { task: NewTask::new("Loose") }, None, &now()).unwrap().created.unwrap();
    assert_eq!(s.task(&id).unwrap().list_id, INBOX);
    // Errors.
    assert!(s.apply(TodoOp::Add { task: NewTask::new("  tomorrow ") }, None, &now()).is_err());
    assert!(s.apply(TodoOp::Add { task: NewTask::new("x").in_list("nope") }, None, &now()).is_err());
}

#[test]
fn subtasks_order_and_insertion() {
    let mut s = store();
    let a = add(&mut s, "A", INBOX);
    let b = add(&mut s, "B", INBOX);
    let c = apply(&mut s, json!({"type": "add", "task": {"text": "C", "listId": INBOX, "afterId": a}})).created.unwrap();
    let order: Vec<&str> = s.ordered(INBOX, false).iter().map(|(_, t)| t.content.as_str()).collect();
    assert_eq!(order, ["A", "C", "B"]);
    let a1 = apply(&mut s, json!({"type": "add", "task": {"text": "A1 #Shop API", "parentId": a}})).created.unwrap();
    let a1x = apply(&mut s, json!({"type": "add", "task": {"text": "A1x", "parentId": a1}})).created.unwrap();
    let t = s.task(&a1).unwrap();
    assert_eq!((t.list_id.as_str(), t.parent_id.as_deref()), (INBOX, Some(a.as_str())));
    let tree: Vec<(usize, &str)> = s.ordered(INBOX, false).iter().map(|(d, t)| (*d, t.content.as_str())).collect();
    assert_eq!(tree, [(0, "A"), (1, "A1"), (2, "A1x"), (0, "C"), (0, "B")]);
    assert_eq!(s.subtree(&a), [a.clone(), a1.clone(), a1x.clone()]);
    let _ = (b, c);
}

#[test]
fn completing_and_reopening() {
    let mut s = store();
    let p = add(&mut s, "Parent", INBOX);
    let kid = apply(&mut s, json!({"type": "add", "task": {"text": "Kid", "parentId": p}})).created.unwrap();
    let out = apply(&mut s, json!({"type": "complete", "ids": [p], "done": true}));
    assert_eq!(out.count, 1);
    assert!(s.task(&p).unwrap().done && s.task(&kid).unwrap().done);
    assert_eq!(s.task(&kid).unwrap().completed_at, Some(now().ms));
    // Reopening a sub-task reopens its parent.
    apply(&mut s, json!({"type": "complete", "ids": [kid], "done": false}));
    assert!(!s.task(&p).unwrap().done && !s.task(&kid).unwrap().done);
}

#[test]
fn recurring_tasks_advance_and_undo() {
    let mut s = store();
    let id = add(&mut s, "Standup every weekday 9am", INBOX);
    assert_eq!(s.task(&id).unwrap().due.as_ref().unwrap().date, d(2026, 10, 26)); // 9am already passed today
    let kid = apply(&mut s, json!({"type": "add", "task": {"text": "notes", "parentId": id}})).created.unwrap();
    apply(&mut s, json!({"type": "complete", "ids": [kid], "done": true}));
    let out = s.apply(TodoOp::Complete { ids: vec![id.clone()], done: true }, None, &later(3)).unwrap();
    assert_eq!(out.rescheduled, [Rescheduled { id: id.clone(), date: d(2026, 10, 27) }]);
    let task = s.task(&id).unwrap();
    assert!(!task.done);
    assert_eq!(task.due.as_ref().unwrap().time, Some(t(9, 0)));
    assert_eq!(task.history, [Completion { at: later(3).ms, due: d(2026, 10, 26) }]);
    assert!(!s.task(&kid).unwrap().done, "sub-tasks come back for the next occurrence");
    // Un-completing an open recurring task undoes the last completion.
    apply(&mut s, json!({"type": "complete", "ids": [id], "done": false}));
    let task = s.task(&id).unwrap();
    assert_eq!((task.due.as_ref().unwrap().date, task.history.len()), (d(2026, 10, 26), 0));
    // A series that ended completes for good.
    let last = add(&mut s, "Course every day until oct 24", INBOX);
    s.apply(TodoOp::Complete { ids: vec![last.clone()], done: true }, None, &now()).unwrap();
    assert_eq!(s.task(&last).unwrap().due.as_ref().unwrap().date, d(2026, 10, 24));
    s.apply(TodoOp::Complete { ids: vec![last.clone()], done: true }, None, &later(1)).unwrap();
    assert!(s.task(&last).unwrap().done);
}

#[test]
fn updates() {
    let mut s = store();
    let id = add(&mut s, "Review every mon 10am @old", INBOX);
    apply(&mut s, json!({"type": "update", "ids": [id], "patch": {"dueDate": "2026-11-04", "priority": 3, "addLabels": ["@new"], "removeLabels": ["OLD"], "deadline": "nov 30", "description": "## Notes\n", "addLinks": ["notes/a.md"]}}));
    let task = s.task(&id).unwrap();
    let due = task.due.as_ref().unwrap();
    assert_eq!((due.date, due.time, due.is_recurring()), (d(2026, 11, 4), Some(t(10, 0)), true), "dueDate keeps time and rule");
    assert_eq!((task.priority, task.labels.clone(), task.deadline), (Priority::P3, vec!["new".to_string()], Some(d(2026, 11, 30))));
    assert_eq!((task.description.as_str(), task.links.clone()), ("## Notes", vec!["notes/a.md".to_string()]));
    apply(&mut s, json!({"type": "update", "ids": [id], "patch": {"dueString": "tomorrow"}}));
    assert!(!s.task(&id).unwrap().due.as_ref().unwrap().is_recurring());
    apply(&mut s, json!({"type": "update", "ids": [id], "patch": {"due": null, "deadline": null}}));
    assert!(s.task(&id).unwrap().due.is_none() && s.task(&id).unwrap().deadline.is_none());
    apply(&mut s, json!({"type": "update", "ids": [id], "patch": {"due": {"date": "2026-12-01", "time": "08:15", "timezone": "Europe/Paris"}}}));
    assert_eq!(s.task(&id).unwrap().due.as_ref().unwrap().timezone.as_deref(), Some("Europe/Paris"));
    let bad: TodoOp = serde_json::from_value(json!({"type": "update", "ids": [id], "patch": {"dueString": "blah"}})).unwrap();
    assert!(s.apply(bad, None, &now()).is_err());
    // Bulk: many ids at once.
    let b = add(&mut s, "B", INBOX);
    let out = apply(&mut s, json!({"type": "update", "ids": [id, b], "patch": {"dueDate": "2026-10-23"}}));
    assert_eq!(out.count, 2);
    assert_eq!(s.task(&b).unwrap().due.as_ref().unwrap().date, d(2026, 10, 23));
}

#[test]
fn inline_edit_parses_quick_add() {
    let mut s = store();
    let work = apply(&mut s, json!({"type": "saveList", "name": "Work"})).created.unwrap();
    let id = add(&mut s, "Draft spec @docs", INBOX);
    apply(&mut s, json!({"type": "edit", "id": id, "text": "Draft API spec fri p2 @review #Work"}));
    let task = s.task(&id).unwrap();
    assert_eq!((task.content.as_str(), task.priority, task.list_id.as_str()), ("Draft API spec", Priority::P2, work.as_str()));
    assert_eq!(task.labels, ["docs", "review"]);
    assert_eq!(task.due.as_ref().unwrap().date, d(2026, 10, 30));
    apply(&mut s, json!({"type": "edit", "id": id, "text": "Draft API spec no date", "ignore": []}));
    assert!(s.task(&id).unwrap().due.is_none());
    apply(&mut s, json!({"type": "edit", "id": id, "text": "tomorrow p4"}));
    assert_eq!((s.task(&id).unwrap().content.as_str(), s.task(&id).unwrap().priority), ("Draft API spec", Priority::P4));
    apply(&mut s, json!({"type": "edit", "id": id, "text": "Buy sun cream", "ignore": ["sun"]}));
    assert_eq!(s.task(&id).unwrap().content, "Buy sun cream");
}

#[test]
fn moving_indenting_and_cycles() {
    let mut s = store();
    let a = add(&mut s, "A", INBOX);
    let b = add(&mut s, "B", INBOX);
    let c = add(&mut s, "C", INBOX);
    let b1 = apply(&mut s, json!({"type": "add", "task": {"text": "B1", "parentId": b}})).created.unwrap();
    // Reorder: C first.
    apply(&mut s, json!({"type": "move", "ids": [c], "listId": INBOX, "index": 0}));
    let order: Vec<&str> = s.ordered(INBOX, false).iter().filter(|(d, _)| *d == 0).map(|(_, t)| t.content.as_str()).collect();
    assert_eq!(order, ["C", "A", "B"]);
    // Indent B under A (B1 follows).
    apply(&mut s, json!({"type": "move", "ids": [b], "listId": INBOX, "parentId": a}));
    let tree: Vec<(usize, &str)> = s.ordered(INBOX, false).iter().map(|(d, t)| (*d, t.content.as_str())).collect();
    assert_eq!(tree, [(0, "C"), (0, "A"), (1, "B"), (2, "B1")]);
    // Can't move into own subtree.
    let cyc: TodoOp = serde_json::from_value(json!({"type": "move", "ids": [a], "listId": INBOX, "parentId": b1})).unwrap();
    assert!(s.apply(cyc, None, &now()).is_err());
    // Move a subtree to the project list's section.
    let sec = apply(&mut s, json!({"type": "saveSection", "listId": "01PROJECT", "name": "Bugs"})).created.unwrap();
    apply(&mut s, json!({"type": "move", "ids": [a], "listId": "01PROJECT", "sectionId": sec}));
    for id in [&a, &b, &b1] {
        let t = s.task(id).unwrap();
        assert_eq!((t.list_id.as_str(), t.section_id.as_deref()), ("01PROJECT", Some(sec.as_str())));
    }
    assert_eq!(s.task(&a).unwrap().parent_id, None);
    assert_eq!(s.task(&b).unwrap().parent_id.as_deref(), Some(a.as_str()));
    // A section of another list is dropped.
    apply(&mut s, json!({"type": "move", "ids": [c], "listId": INBOX, "sectionId": sec}));
    assert_eq!(s.task(&c).unwrap().section_id, None);
}

/// Drag & drop sends the position among the *open* siblings (what the UI shows). Completed tasks
/// stay in the file with their old positions, so counting them used to land moves in the wrong
/// place — moving a task down past one with a completed task above it did nothing.
#[test]
fn move_index_counts_open_siblings_only() {
    let mut s = store();
    let a = add(&mut s, "A", INBOX);
    let x = add(&mut s, "X", INBOX);
    let b = add(&mut s, "B", INBOX);
    let y = add(&mut s, "Y", INBOX);
    let c = add(&mut s, "C", INBOX);
    apply(&mut s, json!({"type": "complete", "ids": [x, y], "done": true}));
    let open = |s: &TodoData| s.ordered(INBOX, false).iter().map(|(_, t)| t.content.clone()).collect::<Vec<_>>();
    // A after B: index 1 among the open siblings without A ([B, C]).
    apply(&mut s, json!({"type": "move", "ids": [a], "listId": INBOX, "index": 1}));
    assert_eq!(open(&s), ["B", "A", "C"]);
    // C to the top, then B to the end.
    apply(&mut s, json!({"type": "move", "ids": [c], "listId": INBOX, "index": 0}));
    assert_eq!(open(&s), ["C", "B", "A"]);
    apply(&mut s, json!({"type": "move", "ids": [b], "listId": INBOX, "index": 2}));
    assert_eq!(open(&s), ["C", "A", "B"]);
    // Completed tasks follow the open ones, in their old order.
    let all: Vec<String> = s.ordered(INBOX, true).iter().map(|(_, t)| t.content.clone()).collect();
    assert_eq!(all, ["C", "A", "B", "X", "Y"]);
    // "Add after" counts open siblings too.
    apply(&mut s, json!({"type": "add", "task": {"text": "D", "listId": INBOX, "afterId": c}}));
    assert_eq!(open(&s), ["C", "D", "A", "B"]);
    // Reopened tasks come back after the open ones.
    apply(&mut s, json!({"type": "complete", "ids": [x], "done": false}));
    assert_eq!(open(&s), ["C", "D", "A", "B", "X"]);
}

#[test]
fn delete_and_restore() {
    let mut s = store();
    let a = add(&mut s, "A @x", INBOX);
    let a1 = apply(&mut s, json!({"type": "add", "task": {"text": "A1", "parentId": a}})).created.unwrap();
    let saved: Vec<Task> = s.subtree(&a).iter().map(|id| s.task(id).unwrap().clone()).collect();
    assert_eq!(apply(&mut s, json!({"type": "delete", "ids": [a]})).count, 2);
    assert!(s.task(&a1).is_none());
    apply(&mut s, json!({"type": "restore", "tasks": saved}));
    assert_eq!(s.task(&a1).unwrap().parent_id.as_deref(), Some(a.as_str()));
    // Restoring into a list that's gone lands in the Inbox; a missing parent makes it top-level.
    let mut orphan = s.task(&a1).unwrap().clone();
    orphan.id = "ORPHAN".into();
    orphan.parent_id = Some("GONE".into());
    orphan.list_id = "GONE".into();
    apply(&mut s, json!({"type": "restore", "tasks": [orphan]}));
    let o = s.task("ORPHAN").unwrap();
    assert_eq!((o.list_id.as_str(), o.parent_id.as_deref()), (INBOX, None));
}

#[test]
fn lists_sections_labels_filters() {
    let mut s = store();
    let work = apply(&mut s, json!({"type": "saveList", "name": "Work"})).created.unwrap();
    apply(&mut s, json!({"type": "saveList", "id": work, "name": "Job", "color": "green"}));
    assert_eq!(s.list(&work).unwrap().name, "Job");
    // Project/Inbox names aren't editable, colours are.
    apply(&mut s, json!({"type": "saveList", "id": INBOX, "name": "Other", "color": "red"}));
    assert_eq!((s.list(INBOX).unwrap().name.as_str(), s.list(INBOX).unwrap().color.as_deref()), ("Inbox", Some("red")));
    let bad: TodoOp = serde_json::from_value(json!({"type": "deleteList", "id": INBOX})).unwrap();
    assert!(s.apply(bad, None, &now()).is_err());

    let sec = apply(&mut s, json!({"type": "saveSection", "listId": work, "name": "Later"})).created.unwrap();
    let sec2 = apply(&mut s, json!({"type": "saveSection", "listId": work, "name": "Now"})).created.unwrap();
    apply(&mut s, json!({"type": "reorderSections", "ids": [sec2, sec]}));
    assert_eq!(s.sections.iter().find(|x| x.id == sec2).unwrap().order, 0);
    apply(&mut s, json!({"type": "saveSection", "id": sec, "listId": work, "name": "", "collapsed": true}));
    assert!(s.sections.iter().find(|x| x.id == sec).unwrap().collapsed);
    let x = apply(&mut s, json!({"type": "add", "task": {"text": "x @Bug", "listId": work, "sectionId": sec}})).created.unwrap();
    assert_eq!(apply(&mut s, json!({"type": "deleteSection", "id": sec})).count, 1);
    assert!(s.task(&x).is_none());

    let y = add(&mut s, "y @bug", INBOX);
    apply(&mut s, json!({"type": "saveLabel", "name": "BUG", "newName": "defect", "color": "red"}));
    assert_eq!(s.task(&y).unwrap().labels, ["defect"]);
    assert_eq!(s.labels.iter().find(|l| l.name == "defect").unwrap().color.as_deref(), Some("red"));
    apply(&mut s, json!({"type": "deleteLabel", "name": "Defect"}));
    assert!(s.task(&y).unwrap().labels.is_empty());

    let bad: TodoOp = serde_json::from_value(json!({"type": "saveFilter", "name": "x", "query": "p1 &"})).unwrap();
    assert!(s.apply(bad, None, &now()).is_err());
    let f = apply(&mut s, json!({"type": "saveFilter", "name": "", "query": "#job | @x"})).created.unwrap();
    assert_eq!(s.filters.iter().find(|x| x.id == f).unwrap().name, "#job | @x");

    let gone = add(&mut s, "z", &work);
    assert_eq!(apply(&mut s, json!({"type": "deleteList", "id": work})).count, 1);
    assert!(s.task(&gone).is_none() && s.list(&work).is_none());
}

#[test]
fn comments() {
    let mut s = store();
    let id = add(&mut s, "A", INBOX);
    let c = apply(&mut s, json!({"type": "addComment", "id": id, "text": "Looked at logs\n"})).created.unwrap();
    assert_eq!(s.task(&id).unwrap().comments[0].text, "Looked at logs");
    apply(&mut s, json!({"type": "deleteComment", "id": id, "commentId": c}));
    assert!(s.task(&id).unwrap().comments.is_empty());
    let empty: TodoOp = serde_json::from_value(json!({"type": "addComment", "id": id, "text": " "})).unwrap();
    assert!(s.apply(empty, None, &now()).is_err());
}

#[test]
fn snapshot_views_and_history() {
    let mut s = store();
    let a = add(&mut s, "Ping the team 18:00", INBOX);
    let p = add(&mut s, "Parent", INBOX);
    let kid = apply(&mut s, json!({"type": "add", "task": {"text": "kid", "parentId": p}})).created.unwrap();
    let old = add(&mut s, "Old done", INBOX);
    let r = add(&mut s, "Water plants every day", INBOX);
    s.apply(TodoOp::Complete { ids: vec![old.clone()], done: true }, None, &later(-10)).unwrap();
    apply(&mut s, json!({"type": "complete", "ids": [kid], "done": true}));
    apply(&mut s, json!({"type": "complete", "ids": [r], "done": true}));
    let snap = s.snapshot_in(Some(&project()), &now(), &Paris);
    let ids: Vec<&str> = snap.tasks.iter().map(|t| t.task.id.as_str()).collect();
    assert!(ids.contains(&kid.as_str()), "done sub-task of an open parent");
    assert!(!ids.contains(&old.as_str()), "old completed tasks stay out of the snapshot");
    assert_eq!(snap.completed_count, 3);
    assert_eq!(snap.project_list.as_deref(), Some("01PROJECT"));
    assert_eq!(snap.today, "2026-10-23");
    let at = snap.tasks.iter().find(|t| t.task.id == a).unwrap();
    assert_eq!(at.due_local.as_deref(), Some("2026-10-23"));
    assert_eq!(at.due_at, Some(Paris.with_ymd_and_hms(2026, 10, 23, 18, 0, 0).unwrap().timestamp_millis()));
    let f = snap.filters.iter().find(|f| f.filter.query == "overdue | 7 days").unwrap();
    assert_eq!(f.count, 2, "{:?}", f.groups);
    // The JSON the UI gets: task fields flattened, camelCase.
    let v = serde_json::to_value(&snap).unwrap();
    assert!(v["tasks"][0]["content"].is_string() && v["tasks"][0]["listId"].is_string());
    assert_eq!(v["filters"][0]["name"], "Priority 1");

    let done = s.completed(None, None, 10);
    assert_eq!(done.len(), 3);
    assert!(done[0].at >= done[1].at);
    assert!(done.iter().any(|e| e.recurring && e.task.task.id == r && e.occurrence == Some(d(2026, 10, 23))));
    assert_eq!(s.completed(None, Some(now().ms), 10).len(), 1);
    assert_eq!(s.completed(Some("01PROJECT"), None, 10).len(), 0);
    assert_eq!(apply(&mut s, json!({"type": "clearCompleted"})).count, 2);
}

#[test]
fn filters_in_scope() {
    let mut s = store();
    add(&mut s, "a p1 today", INBOX);
    let b = add(&mut s, "b p1 tomorrow", "01PROJECT");
    let g = s.run_filter_in("p1", Some(&["01PROJECT".to_string()]), &now(), &Paris).unwrap();
    assert_eq!(g[0].ids, std::slice::from_ref(&b));
    let g = s.run_filter_in("tomorrow, today", None, &now(), &Paris).unwrap();
    assert_eq!((g[0].title.as_str(), g[0].ids.len(), g[1].ids.len()), ("tomorrow", 1, 1));
    assert!(s.run_filter_in("p1 &", None, &now(), &Paris).is_err());
}

#[test]
fn reminders() {
    let mut s = store();
    let a = add(&mut s, "Call 15:00", INBOX);
    add(&mut s, "Allday today", INBOX);
    add(&mut s, "Tomorrow 15:00 thing tomorrow 15:00", INBOX);
    let at = |h: u32, m: u32| Paris.with_ymd_and_hms(2026, 10, 23, h, m, 0).unwrap().timestamp_millis();
    let r = s.due_reminders(at(14, 59), at(15, 0), &Paris);
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].task_id.as_str(), r[0].title.as_str()), (a.as_str(), "Call"));
    assert!(r[0].body.starts_with("Due now (15:00)"));
    // Window is (from, to]: nothing twice.
    assert!(s.due_reminders(at(15, 0), at(15, 1), &Paris).is_empty());
    // Offset reminder + all-day reminder.
    apply(&mut s, json!({"type": "saveSettings", "settings": {"reminders": true, "remindBeforeMinutes": 10, "allDayReminder": "09:00"}}));
    let r = s.due_reminders(at(14, 45), at(14, 51), &Paris);
    assert_eq!(r.len(), 1);
    assert!(r[0].body.starts_with("Due in 10 min"));
    let r = s.due_reminders(at(8, 59), at(9, 0), &Paris);
    assert_eq!(r.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Allday"]);
    // Waking from sleep only looks back 15 minutes.
    assert!(s.due_reminders(at(0, 0), at(16, 0), &Paris).is_empty());
    assert_eq!(s.due_reminders(at(0, 0), at(15, 10), &Paris).len(), 1);
    // Completed tasks and disabled reminders are silent.
    apply(&mut s, json!({"type": "complete", "ids": [a], "done": true}));
    assert!(s.due_reminders(at(14, 59), at(15, 0), &Paris).is_empty());
    let bad: TodoOp = serde_json::from_value(json!({"type": "saveSettings", "settings": {"reminders": true, "allDayReminder": "9am"}})).unwrap();
    assert!(s.apply(bad, None, &now()).is_err());
    apply(&mut s, json!({"type": "saveSettings", "settings": {"reminders": false}}));
    assert!(s.due_reminders(at(8, 59), at(9, 0), &Paris).is_empty());
}

#[test]
fn short_ids() {
    let mut s = store();
    let a = add(&mut s, "A", INBOX);
    assert_eq!(s.find_id(&a).unwrap(), a);
    assert_eq!(s.find_id(&a[a.len() - 6..].to_lowercase()).unwrap(), a);
    assert!(s.find_id("ZZZZZZZZ").is_err());
    assert!(s.find_id("abc").is_err());
}

#[test]
fn file_store_and_v1_migration() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("todos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("_global.json"),
        r#"{"version":1,"projectName":"Global","items":[{"id":"G1","text":"Renew domain","done":false,"priority":"high","due":"2026-11-01","createdAt":1}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("01PROJECT.json"),
        r#"{"version":1,"projectName":"Shop API","items":[
            {"id":"T1","text":"Write auth tests","priority":"normal","links":["http/auth/login.req.yaml"],"notes":"see notes","tags":["auth"],"createdAt":5},
            {"id":"T2","text":"Ask about limits","done":true,"doneAt":9,"priority":"low","createdAt":6}]}"#,
    )
    .unwrap();
    std::fs::write(dir.join("broken.json"), "{ not json").unwrap();

    // Reading migrates in memory without touching the files.
    let data = TodoStore::read(base.path()).unwrap();
    assert!(dir.join("_global.json").exists() && !dir.join(FILE).exists());
    let g1 = data.task("G1").unwrap();
    assert_eq!((g1.list_id.as_str(), g1.priority, g1.due.as_ref().unwrap().date), (INBOX, Priority::P1, d(2026, 11, 1)));
    let t1 = data.task("T1").unwrap();
    assert_eq!((t1.list_id.as_str(), t1.description.as_str(), t1.labels.clone(), t1.links.len()), ("01PROJECT", "see notes", vec!["auth".to_string()], 1));
    let t2 = data.task("T2").unwrap();
    assert_eq!((t2.done, t2.completed_at, t2.priority, t2.labels.clone(), t2.order), (true, Some(9), Priority::P4, vec!["low".to_string()], 1));
    assert_eq!(data.list("01PROJECT").unwrap().name, "Shop API");
    assert_eq!(data.filters.len(), 2);

    // The first save writes v2 and moves the v1 files to a backup folder (the broken one stays).
    let mut st = TodoStore::open(base.path()).unwrap();
    st.add_text(Some(&project()), "New one").unwrap();
    assert!(dir.join(FILE).exists());
    assert!(dir.join("v1-backup").join("_global.json").exists() && dir.join("v1-backup").join("01PROJECT.json").exists());
    assert!(!dir.join("_global.json").exists() && dir.join("broken.json").exists());
    let again = TodoStore::read(base.path()).unwrap();
    assert_eq!(again.tasks.len(), 4);
    assert_eq!(again.version, SCHEMA_VERSION);

    // A file from a newer version is refused rather than overwritten.
    std::fs::write(dir.join(FILE), r#"{"version":99}"#).unwrap();
    assert!(TodoStore::open(base.path()).is_err());
}

#[test]
fn files_without_attachments_still_load() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("todos");
    std::fs::create_dir_all(&dir).unwrap();
    // A v2 file written before attachments existed.
    std::fs::write(dir.join(FILE), r#"{"version":2,"lists":[{"id":"inbox","name":"Inbox","kind":"inbox"}],"tasks":[{"id":"T1","content":"Old","listId":"inbox","createdAt":1}]}"#).unwrap();
    let data = TodoStore::read(base.path()).unwrap();
    assert!(data.task("T1").unwrap().attachments.is_empty());
    // Tasks without attachments don't write the field.
    let json = serde_json::to_value(data.task("T1").unwrap()).unwrap();
    assert!(json.get("attachments").is_none());
}

#[test]
fn attachments_survive_undo_and_go_with_permanent_deletes() {
    let base = tempfile::tempdir().unwrap();
    let mut st = TodoStore::open(base.path()).unwrap();
    let p = Some(project());
    let task = st.add_text(p.as_ref(), "With files").unwrap();
    let shot = st.attach_bytes(&task.id, "C:\\tmp\\shot.png", Some("image/png"), b"img", p.as_ref(), &now()).unwrap();
    let src = base.path().join("spec.pdf");
    std::fs::write(&src, b"%PDF-1").unwrap();
    let spec = st.attach_file(&task.id, &src, p.as_ref(), &now()).unwrap();
    assert_eq!((shot.name.as_str(), shot.mime.as_str()), ("shot.png", "image/png"));
    let path = st.data.attachment_path(base.path(), &task.id, &spec.id).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1");
    // Saved, and in the snapshot the UI gets.
    let again = TodoStore::read(base.path()).unwrap();
    assert_eq!(again.task(&task.id).unwrap().attachments.len(), 2);
    let snap = serde_json::to_value(again.snapshot_in(p.as_ref(), &now(), &Paris)).unwrap();
    assert_eq!(snap["tasks"][0]["attachments"][0]["name"], "shot.png");
    // Unknown task: nothing is written.
    assert!(st.attach_bytes("NOPE", "x.txt", None, b"x", p.as_ref(), &now()).is_err());
    assert!(!attachments::root(base.path()).join("NOPE").exists());

    // Removing an attachment and undoing it (restore) keeps the file usable.
    let before = st.data.task(&task.id).unwrap().clone();
    st.apply(TodoOp::RemoveAttachment { id: task.id.clone(), attachment_id: shot.id.clone() }, p.as_ref(), &now()).unwrap();
    assert_eq!(st.data.task(&task.id).unwrap().attachments.len(), 1);
    assert!(st.data.attachment_path(base.path(), &task.id, &shot.id).is_err());
    // Deleting the task keeps its folder for undo; restoring brings both files back.
    st.apply(TodoOp::Delete { ids: vec![task.id.clone()] }, p.as_ref(), &now()).unwrap();
    assert!(path.exists());
    st.apply(TodoOp::Restore { tasks: vec![before] }, p.as_ref(), &now()).unwrap();
    let shot_path = st.data.attachment_path(base.path(), &task.id, &shot.id).unwrap();
    assert_eq!(std::fs::read(shot_path).unwrap(), b"img");
    // Startup GC leaves files that are referenced.
    assert_eq!(TodoStore::gc_attachments(base.path(), later(30).ms).unwrap(), 0);
    assert!(path.exists());

    // Clearing completed tasks removes them for good — and their files.
    st.apply(TodoOp::Complete { ids: vec![task.id.clone()], done: true }, p.as_ref(), &now()).unwrap();
    st.apply(TodoOp::ClearCompleted { list_id: None }, p.as_ref(), &now()).unwrap();
    assert!(!attachments::root(base.path()).join(&task.id).exists());
    // The IPC can't smuggle attachment records in.
    assert!(serde_json::from_value::<TodoOp>(json!({"type": "addAttachment", "id": "T", "attachment": {"id": "A", "name": "x", "mime": "x/y", "size": 1, "addedAt": 0}})).is_err());
}

#[test]
fn empty_base_creates_a_fresh_store_on_first_save() {
    let base = tempfile::tempdir().unwrap();
    let data = TodoStore::read(base.path()).unwrap();
    assert_eq!(data.lists.len(), 1);
    assert!(!base.path().join("todos").exists());
    let mut st = TodoStore::open(base.path()).unwrap();
    let t = st.add_text(None, "Inbox thing p2").unwrap();
    assert_eq!((t.list_id.as_str(), t.priority), (INBOX, Priority::P2));
    assert!(st.path().exists());
}

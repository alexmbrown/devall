use super::*;
use crate::todos::model::{Day, Freq};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}
fn t(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap()
}
/// Friday 2026-10-23, 14:30.
fn ctx() -> DateCtx {
    DateCtx::new(d(2026, 10, 23), t(14, 30))
}
fn due(s: &str) -> Due {
    parse_due(s, &ctx()).unwrap_or_else(|e| panic!("{s}: {e}")).unwrap_or_else(|| panic!("{s}: no date"))
}
fn due_at(s: &str, c: DateCtx) -> Due {
    parse_due(s, &c).unwrap_or_else(|e| panic!("{s}: {e}")).unwrap()
}

#[test]
fn single_dates() {
    let cases: &[(&str, NaiveDate, Option<NaiveTime>)] = &[
        ("today", d(2026, 10, 23), None),
        ("tod", d(2026, 10, 23), None),
        ("Today", d(2026, 10, 23), None),
        ("tomorrow", d(2026, 10, 24), None),
        ("tom", d(2026, 10, 24), None),
        ("tmrw", d(2026, 10, 24), None),
        ("yesterday", d(2026, 10, 22), None),
        ("tonight", d(2026, 10, 23), Some(t(20, 0))),
        ("this evening", d(2026, 10, 23), Some(t(18, 0))),
        ("tomorrow morning", d(2026, 10, 24), Some(t(9, 0))),
        ("tomorrow 5pm", d(2026, 10, 24), Some(t(17, 0))),
        ("tomorrow at 5:30pm", d(2026, 10, 24), Some(t(17, 30))),
        ("tomorrow at 5:30 p.m.", d(2026, 10, 24), Some(t(17, 30))),
        ("5pm tomorrow", d(2026, 10, 24), Some(t(17, 0))),
        ("at 17:00 tomorrow", d(2026, 10, 24), Some(t(17, 0))),
        ("tomorrow 17h", d(2026, 10, 24), Some(t(17, 0))),
        // A time alone: today when still ahead, else tomorrow.
        ("5pm", d(2026, 10, 23), Some(t(17, 0))),
        ("5 pm", d(2026, 10, 23), Some(t(17, 0))),
        ("9am", d(2026, 10, 24), Some(t(9, 0))),
        ("noon", d(2026, 10, 24), Some(t(12, 0))),
        ("14:30", d(2026, 10, 24), Some(t(14, 30))),
        ("14:31", d(2026, 10, 23), Some(t(14, 31))),
        ("12am", d(2026, 10, 24), Some(t(0, 0))),
        ("tomorrow 12pm", d(2026, 10, 24), Some(t(12, 0))),
        // Weekdays: the next one after today (today is a Friday); `this` may be today.
        ("fri", d(2026, 10, 30), None),
        ("friday", d(2026, 10, 30), None),
        ("next fri", d(2026, 10, 30), None),
        ("this fri", d(2026, 10, 23), None),
        ("mon", d(2026, 10, 26), None),
        ("next monday", d(2026, 10, 26), None),
        ("thu", d(2026, 10, 29), None),
        ("sat", d(2026, 10, 24), None),
        ("mon 9am", d(2026, 10, 26), Some(t(9, 0))),
        ("on monday at noon", d(2026, 10, 26), Some(t(12, 0))),
        ("next week", d(2026, 10, 26), None),
        ("next month", d(2026, 11, 1), None),
        ("next year", d(2027, 1, 1), None),
        ("weekend", d(2026, 10, 24), None),
        ("this weekend", d(2026, 10, 24), None),
        ("next weekend", d(2026, 10, 31), None),
        ("end of month", d(2026, 10, 31), None),
        ("eom", d(2026, 10, 31), None),
        ("end of week", d(2026, 10, 23), None),
        ("end of the year", d(2026, 12, 31), None),
        // Relative.
        ("in 3 days", d(2026, 10, 26), None),
        ("in three days", d(2026, 10, 26), None),
        ("in a week", d(2026, 10, 30), None),
        ("in 2 weeks", d(2026, 11, 6), None),
        ("in 2w", d(2026, 11, 6), None),
        ("in 1 month", d(2026, 11, 23), None),
        ("in a year", d(2027, 10, 23), None),
        ("in 2 hours", d(2026, 10, 23), Some(t(16, 30))),
        ("in 30 min", d(2026, 10, 23), Some(t(15, 0))),
        ("in 10 hours", d(2026, 10, 24), Some(t(0, 30))),
        // Calendar dates: past month-days without a year mean next year.
        ("jan 15", d(2027, 1, 15), None),
        ("January 15th", d(2027, 1, 15), None),
        ("15 jan", d(2027, 1, 15), None),
        ("15th of january", d(2027, 1, 15), None),
        ("dec 25", d(2026, 12, 25), None),
        ("oct 23", d(2026, 10, 23), None),
        ("oct 22", d(2027, 10, 22), None),
        ("january 15th, 2028", d(2028, 1, 15), None),
        ("jan 15 2028 at 10:30", d(2028, 1, 15), Some(t(10, 30))),
        ("feb 29", d(2028, 2, 29), None),
        ("2026-12-01", d(2026, 12, 1), None),
        ("2026/12/01 9:15am", d(2026, 12, 1), Some(t(9, 15))),
        // Ordinals: the next such day of a month.
        ("15th", d(2026, 11, 15), None),
        ("the 30th", d(2026, 10, 30), None),
        ("on the 31st", d(2026, 10, 31), None),
        ("23rd", d(2026, 10, 23), None),
    ];
    for (s, date, time) in cases {
        let got = due(s);
        assert_eq!((got.date, got.time), (*date, *time), "{s}");
        assert!(got.recurrence.is_none() && got.string.is_empty(), "{s}");
    }
}

#[test]
fn month_ends_and_leap_years() {
    let jan31 = DateCtx::new(d(2026, 1, 31), t(8, 0));
    assert_eq!(due_at("in 1 month", jan31).date, d(2026, 2, 28));
    assert_eq!(due_at("next month", jan31).date, d(2026, 2, 1));
    assert_eq!(due_at("eom", DateCtx::new(d(2028, 2, 3), t(8, 0))).date, d(2028, 2, 29));
    // 31st in a 30-day month skips to the next month that has one.
    assert_eq!(due_at("31st", DateCtx::new(d(2026, 11, 5), t(8, 0))).date, d(2026, 12, 31));
    assert_eq!(due_at("30th", DateCtx::new(d(2027, 2, 1), t(8, 0))).date, d(2027, 3, 30));
    // Year rollover.
    assert_eq!(due_at("tomorrow", DateCtx::new(d(2026, 12, 31), t(8, 0))).date, d(2027, 1, 1));
    assert_eq!(due_at("next week", DateCtx::new(d(2026, 12, 28), t(8, 0))).date, d(2027, 1, 4));
    assert_eq!(due_at("in 2 hours", DateCtx::new(d(2026, 12, 31), t(23, 0))).date, d(2027, 1, 1));
}

#[test]
fn invalid_dates_are_errors() {
    for s in ["blah", "tomorrow blah", "feb 30", "13pm", "25:00", "at 5", "every", "every blah", "jan 32", "2026-13-01"] {
        assert!(parse_due(s, &ctx()).is_err(), "{s} should not parse");
    }
    assert!(parse_due("every 2 hours", &ctx()).unwrap_err().contains("hourly"));
    assert_eq!(parse_due("", &ctx()).unwrap(), None);
    assert_eq!(parse_due("no date", &ctx()).unwrap(), None);
}

fn rule(s: &str) -> (NaiveDate, Option<NaiveTime>, Recurrence) {
    let due = due(s);
    assert_eq!(due.string, s.trim(), "string kept for {s}");
    (due.date, due.time, due.recurrence.unwrap_or_else(|| panic!("{s}: not recurring")))
}

#[test]
fn recurring_rules() {
    let (date, time, r) = rule("every day");
    assert_eq!((date, time, r.freq, r.interval), (d(2026, 10, 23), None, Freq::Day, 1));
    // A time already past today starts tomorrow.
    let (date, time, _) = rule("every day at 9am");
    assert_eq!((date, time), (d(2026, 10, 24), Some(t(9, 0))));
    let (date, time, _) = rule("every day 5pm");
    assert_eq!((date, time), (d(2026, 10, 23), Some(t(17, 0))));
    let (date, time, r) = rule("9am every day");
    assert_eq!((date, time, r.freq), (d(2026, 10, 24), Some(t(9, 0)), Freq::Day));
    let (date, time, r) = rule("every morning");
    assert_eq!((date, time, r.freq), (d(2026, 10, 24), Some(t(9, 0)), Freq::Day));
    let (date, _, r) = rule("daily");
    assert_eq!((date, r.freq), (d(2026, 10, 23), Freq::Day));
    let (_, _, r) = rule("every 3 days");
    assert_eq!((r.freq, r.interval), (Freq::Day, 3));
    let (_, _, r) = rule("every other day");
    assert_eq!(r.interval, 2);

    let (date, _, r) = rule("every weekday");
    assert_eq!((date, r.weekdays.len()), (d(2026, 10, 23), 5));
    let (date, _, r) = rule("every workday");
    assert_eq!((date, r.weekdays.len()), (d(2026, 10, 23), 5));
    let (date, _, r) = rule("every weekend");
    assert_eq!((date, r.weekdays.clone()), (d(2026, 10, 24), vec![Day::Sat, Day::Sun]));
    let (date, _, r) = rule("every mon, wed");
    assert_eq!((date, r.freq, r.weekdays.clone()), (d(2026, 10, 26), Freq::Week, vec![Day::Mon, Day::Wed]));
    let (date, _, r) = rule("every mon,wed,fri");
    assert_eq!((date, r.weekdays.len()), (d(2026, 10, 23), 3));
    let (date, _, r) = rule("every tuesday and thursday at 10am");
    assert_eq!((date, r.weekdays.clone()), (d(2026, 10, 27), vec![Day::Tue, Day::Thu]));
    let (date, _, r) = rule("every other mon");
    assert_eq!((date, r.interval, r.weekdays.clone()), (d(2026, 10, 26), 2, vec![Day::Mon]));
    let (date, _, r) = rule("every 2 weeks");
    assert_eq!((date, r.freq, r.interval, r.weekdays.is_empty()), (d(2026, 10, 23), Freq::Week, 2, true));
    let (date, _, r) = rule("every 2 weeks on mon");
    assert_eq!((date, r.weekdays.clone()), (d(2026, 10, 26), vec![Day::Mon]));
    let (_, _, r) = rule("biweekly");
    assert_eq!((r.freq, r.interval), (Freq::Week, 2));

    let (date, _, r) = rule("every month on the 1st");
    assert_eq!((date, r.freq, r.month_days.clone()), (d(2026, 11, 1), Freq::Month, vec![1]));
    let (date, _, r) = rule("every 1st");
    assert_eq!((date, r.month_days.clone()), (d(2026, 11, 1), vec![1]));
    let (date, _, r) = rule("every 1st and 15th");
    assert_eq!((date, r.month_days.clone()), (d(2026, 11, 1), vec![1, 15]));
    let (date, _, r) = rule("every 15th, 1st");
    assert_eq!((date, r.month_days.clone()), (d(2026, 11, 1), vec![1, 15]));
    let (date, _, r) = rule("every last day");
    assert_eq!((date, r.month_days.clone()), (d(2026, 10, 31), vec![-1]));
    let (date, _, _) = rule("every last day of the month");
    assert_eq!(date, d(2026, 10, 31));
    let (date, _, r) = rule("every month on the last day");
    assert_eq!((date, r.month_days.clone()), (d(2026, 10, 31), vec![-1]));
    let (date, _, r) = rule("every month");
    assert_eq!((date, r.freq, r.is_plain()), (d(2026, 10, 23), Freq::Month, true));
    let (_, _, r) = rule("every 3 months");
    assert_eq!((r.freq, r.interval), (Freq::Month, 3));
    let (date, _, r) = rule("every first mon");
    assert_eq!((date, r.week_of_month, r.weekdays.clone()), (d(2026, 11, 2), Some(1), vec![Day::Mon]));
    let (date, _, r) = rule("every last fri");
    assert_eq!((date, r.week_of_month), (d(2026, 10, 30), Some(-1)));
    let (date, _, _) = rule("every 2nd tue of the month");
    assert_eq!(date, d(2026, 11, 10));
    let (date, _, _) = rule("every month on the 3rd wed");
    assert_eq!(date, d(2026, 11, 18));

    let (date, _, r) = rule("every year");
    assert_eq!((date, r.freq), (d(2026, 10, 23), Freq::Year));
    let (date, _, r) = rule("every jan 15");
    assert_eq!((date, r.freq, r.month, r.month_days.clone()), (d(2027, 1, 15), Freq::Year, Some(1), vec![15]));
    let (date, _, _) = rule("every 25 dec");
    assert_eq!(date, d(2026, 12, 25));
    let (date, _, _) = rule("every year on nov 1");
    assert_eq!(date, d(2026, 11, 1));

    let (_, _, r) = rule("every! 3 days");
    assert!(r.from_completion);
    let (date, time, r) = rule("every mon 9am starting nov 2");
    assert_eq!((date, time, r.weekdays.clone()), (d(2026, 11, 2), Some(t(9, 0)), vec![Day::Mon]));
    let (date, _, r) = rule("every day starting tomorrow until nov 1");
    assert_eq!((date, r.until), (d(2026, 10, 24), Some(d(2026, 11, 1))));
}

fn rec(s: &str, date: NaiveDate) -> Due {
    let mut due = due(s);
    due.date = date;
    due
}

#[test]
fn advancing_recurring_tasks() {
    let today = d(2026, 10, 23);
    // From the due date, skipping occurrences already past.
    assert_eq!(next_occurrence(&rec("every day", d(2026, 10, 20)), today), Some(d(2026, 10, 24)));
    assert_eq!(next_occurrence(&rec("every day", today), today), Some(d(2026, 10, 24)));
    // Completing early doesn't skip the next occurrence.
    assert_eq!(next_occurrence(&rec("every day", d(2026, 10, 25)), today), Some(d(2026, 10, 26)));
    // Keeps the schedule: 1, 4, 7…
    assert_eq!(next_occurrence(&rec("every 3 days", d(2026, 10, 1)), d(2026, 10, 5)), Some(d(2026, 10, 7)));
    // `every!` counts from the completion date.
    assert_eq!(next_occurrence(&rec("every! 3 days", d(2026, 10, 1)), d(2026, 10, 5)), Some(d(2026, 10, 8)));
    assert_eq!(next_occurrence(&rec("every! month", d(2026, 1, 10)), d(2026, 1, 31)), Some(d(2026, 2, 28)));
    assert_eq!(next_occurrence(&rec("every! mon", d(2026, 10, 19)), today), Some(d(2026, 10, 26)));
    assert_eq!(next_occurrence(&rec("every mon, wed", d(2026, 10, 26)), d(2026, 10, 26)), Some(d(2026, 10, 28)));
    assert_eq!(next_occurrence(&rec("every mon, wed", d(2026, 10, 28)), d(2026, 10, 28)), Some(d(2026, 11, 2)));
    assert_eq!(next_occurrence(&rec("every weekday", today), today), Some(d(2026, 10, 26)));
    assert_eq!(next_occurrence(&rec("every weekend", d(2026, 10, 24)), d(2026, 10, 24)), Some(d(2026, 10, 25)));
    // Interval phase is kept across weeks.
    assert_eq!(next_occurrence(&rec("every 2 weeks", today), today), Some(d(2026, 11, 6)));
    assert_eq!(next_occurrence(&rec("every 2 weeks", d(2026, 10, 9)), today), Some(d(2026, 11, 6)));
    assert_eq!(next_occurrence(&rec("every 2 weeks", d(2026, 10, 9)), d(2026, 10, 22)), Some(d(2026, 10, 23)));
    assert_eq!(next_occurrence(&rec("every other mon", d(2026, 10, 26)), d(2026, 10, 26)), Some(d(2026, 11, 9)));
    // Month ends: explicit days clamp without drifting; a plain monthly rule follows the due date.
    assert_eq!(next_occurrence(&rec("every 31st", d(2026, 1, 31)), d(2026, 1, 31)), Some(d(2026, 2, 28)));
    assert_eq!(next_occurrence(&rec("every 31st", d(2026, 2, 28)), d(2026, 2, 28)), Some(d(2026, 3, 31)));
    assert_eq!(next_occurrence(&rec("every 30th", d(2028, 1, 30)), d(2028, 1, 30)), Some(d(2028, 2, 29)));
    assert_eq!(next_occurrence(&rec("every last day", d(2026, 1, 31)), d(2026, 1, 31)), Some(d(2026, 2, 28)));
    assert_eq!(next_occurrence(&rec("every last day", d(2026, 2, 28)), d(2026, 2, 28)), Some(d(2026, 3, 31)));
    assert_eq!(next_occurrence(&rec("every month", d(2026, 1, 31)), d(2026, 1, 31)), Some(d(2026, 2, 28)));
    assert_eq!(next_occurrence(&rec("every month", d(2026, 3, 15)), d(2026, 3, 15)), Some(d(2026, 4, 15)));
    assert_eq!(next_occurrence(&rec("every 1st and 15th", d(2026, 11, 1)), d(2026, 11, 1)), Some(d(2026, 11, 15)));
    assert_eq!(next_occurrence(&rec("every 3 months", today), today), Some(d(2027, 1, 23)));
    assert_eq!(next_occurrence(&rec("every first mon", d(2026, 11, 2)), d(2026, 11, 2)), Some(d(2026, 12, 7)));
    assert_eq!(next_occurrence(&rec("every 2nd tue", d(2026, 11, 10)), d(2026, 11, 10)), Some(d(2026, 12, 8)));
    assert_eq!(next_occurrence(&rec("every last fri", d(2026, 10, 30)), d(2026, 10, 30)), Some(d(2026, 11, 27)));
    // Years and leap days.
    assert_eq!(next_occurrence(&rec("every year", d(2028, 2, 29)), d(2028, 2, 29)), Some(d(2029, 2, 28)));
    assert_eq!(next_occurrence(&rec("every jan 15", d(2027, 1, 15)), d(2027, 1, 15)), Some(d(2028, 1, 15)));
    assert_eq!(next_occurrence(&rec("every 2 years", d(2026, 5, 1)), d(2026, 5, 1)), Some(d(2028, 5, 1)));
    // The series ends.
    assert_eq!(next_occurrence(&rec("every day until oct 25", d(2026, 10, 25)), d(2026, 10, 25)), None);
    assert_eq!(next_occurrence(&rec("every day until oct 25", d(2026, 10, 24)), d(2026, 10, 24)), Some(d(2026, 10, 25)));
    // Not recurring.
    assert_eq!(next_occurrence(&Due::on(today), today), None);
}

#[test]
fn deadlines_parse_dates_only() {
    assert_eq!(parse_date("jan 30", &ctx()).unwrap(), Some(d(2027, 1, 30)));
    assert_eq!(parse_date("fri", &ctx()).unwrap(), Some(d(2026, 10, 30)));
    assert_eq!(parse_date("by next week", &ctx()).unwrap(), Some(d(2026, 10, 26)));
    assert_eq!(parse_date("2026-11-30", &ctx()).unwrap(), Some(d(2026, 11, 30)));
    assert_eq!(parse_date("", &ctx()).unwrap(), None);
    assert!(parse_date("tomorrow 5pm", &ctx()).is_err());
    assert!(parse_date("every day", &ctx()).is_err());
}

#[test]
fn dst_and_time_zones() {
    use chrono_tz::{America::New_York, Europe::Paris};
    // Spring forward: 02:30 doesn't exist in New York on 2026-03-08 → 03:30 EDT.
    let gap = resolve_local(&New_York, d(2026, 3, 8).and_time(t(2, 30))).unwrap();
    assert_eq!(gap.with_timezone(&Utc).naive_utc(), d(2026, 3, 8).and_time(t(7, 30)));
    // Fall back: 01:30 happens twice on 2026-11-01 → the earlier (EDT).
    let amb = resolve_local(&New_York, d(2026, 11, 1).and_time(t(1, 30))).unwrap();
    assert_eq!(amb.with_timezone(&Utc).naive_utc(), d(2026, 11, 1).and_time(t(5, 30)));
    // A floating 9am stays 9am local across the DST change (23 h apart in UTC).
    let mut a = due("every day 9am");
    a.date = d(2026, 3, 7);
    let mut b = a.clone();
    b.date = d(2026, 3, 8);
    let (ia, ib) = (due_instant(&a, &New_York).unwrap(), due_instant(&b, &New_York).unwrap());
    assert_eq!((ib - ia).num_hours(), 23);
    // Fixed-zone tasks: Paris leaves summer time on 2026-10-25.
    let mut p = Due::on(d(2026, 10, 24));
    p.time = Some(t(9, 0));
    p.timezone = Some("Europe/Paris".into());
    assert_eq!(due_instant(&p, &New_York).unwrap().naive_utc(), d(2026, 10, 24).and_time(t(7, 0)));
    p.date = d(2026, 10, 25);
    assert_eq!(due_instant(&p, &New_York).unwrap().naive_utc(), d(2026, 10, 25).and_time(t(8, 0)));
    // The local calendar day can differ from the stored one.
    let mut ny = Due::on(d(2026, 10, 23));
    ny.time = Some(t(23, 30));
    ny.timezone = Some("America/New_York".into());
    assert_eq!(local_due_date(&ny, &Paris), d(2026, 10, 24));
    assert_eq!(local_due_date(&ny, &New_York), d(2026, 10, 23));
    // No time → no instant; unknown zones fall back to local time.
    assert!(due_instant(&Due::on(d(2026, 1, 1)), &Paris).is_none());
    ny.timezone = Some("Mars/Olympus".into());
    assert_eq!(local_due_date(&ny, &Paris), d(2026, 10, 23));
}

#[test]
fn tokenizer_spans() {
    let text = "Call mom tomorrow. (mon,wed) today's every!";
    let toks = tokenize(text);
    let norms: Vec<&str> = toks.iter().map(|t| t.norm.as_str()).collect();
    assert_eq!(norms, ["call", "mom", "tomorrow", "mon", ",", "wed", "today's", "every!"]);
    assert_eq!(&text[toks[2].start..toks[2].end], "tomorrow");
    assert_eq!(&text[toks[3].start..toks[3].end], "mon");
    assert_eq!(&text[toks[5].start..toks[5].end], "wed");
    // Multi-byte text keeps byte spans.
    let text = "Café ünïcode tomorrow";
    let toks = tokenize(text);
    assert_eq!(&text[toks[2].start..toks[2].end], "tomorrow");
}

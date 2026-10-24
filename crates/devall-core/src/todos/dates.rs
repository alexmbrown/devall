//! Natural-language due dates and recurrence rules, Todoist style: `today`, `tomorrow 5pm`,
//! `next fri`, `in 3 days`, `jan 15`, `15th`, `every weekday`, `every mon, wed`, `every 2 weeks`,
//! `every month on the 1st`, `every last day`, `every! 3 days`…
//!
//! Everything is relative to a [`DateCtx`] (today + the current time) so it is pure and testable.
//! Conventions (documented in `docs/FEATURES.md`):
//! - a bare weekday (`fri`, `next fri`) is the next such day *after* today; `this fri` may be today;
//! - `next week` = next Monday, `next month` = the 1st, `weekend` = Saturday;
//! - a time alone (`5pm`) is today if still ahead, otherwise tomorrow;
//! - month-days without a year that already passed this year mean next year;
//! - `morning` 09:00, `noon` 12:00, `afternoon` 13:00, `evening` 18:00, `tonight`/`night` 20:00;
//! - monthly days past a month's end clamp to its last day.

use chrono::{DateTime, Datelike, Days, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike, Utc, Weekday};

use super::model::{Day, Due, Freq, Recurrence};

/// "Now" for parsing: the local date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateCtx {
    pub today: NaiveDate,
    pub now: NaiveTime,
}

impl DateCtx {
    pub fn new(today: NaiveDate, now: NaiveTime) -> Self {
        Self { today, now }
    }

    /// The machine's local date and time.
    pub fn local_now() -> Self {
        let n = chrono::Local::now().naive_local();
        Self { today: n.date(), now: NaiveTime::from_hms_opt(n.time().hour(), n.time().minute(), 0).unwrap_or(n.time()) }
    }
}


/// A parsed date expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSpec {
    pub date: NaiveDate,
    pub time: Option<NaiveTime>,
    pub recurrence: Option<Recurrence>,
}

impl DueSpec {
    /// Turns the spec into a stored due date; `string` is kept for recurring rules.
    pub fn into_due(self, string: &str) -> Due {
        let string = if self.recurrence.is_some() { string.trim().to_string() } else { String::new() };
        Due { date: self.date, time: self.time, timezone: None, recurrence: self.recurrence, string }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Matched {
    Due(DueSpec),
    /// `no date` — explicitly clears the due date.
    NoDate,
}

/// A word of free text: lowercased with trailing punctuation trimmed; `start..end` is its byte span
/// in the source (without the trimmed punctuation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tok {
    pub norm: String,
    pub start: usize,
    pub end: usize,
}

/// Splits text into words (commas are words of their own).
pub(crate) fn tokenize(text: &str) -> Vec<Tok> {
    fn push(out: &mut Vec<Tok>, text: &str, start: usize, end: usize) {
        let raw = &text[start..end];
        let lead = raw.len() - raw.trim_start_matches(['(', '"', '\u{201c}']).len();
        let mut s = &raw[lead..];
        if s.to_lowercase() != "every!" {
            s = s.trim_end_matches(['.', ';', '?', '!', ':', ')', '"', '\u{201d}']);
        }
        if !s.is_empty() {
            out.push(Tok { norm: s.to_lowercase(), start: start + lead, end: start + lead + s.len() });
        }
    }
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() || c == ',' {
            if let Some(s) = start.take() {
                push(&mut out, text, s, i);
            }
            if c == ',' {
                out.push(Tok { norm: ",".into(), start: i, end: i + 1 });
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        push(&mut out, text, s, text.len());
    }
    out
}

fn weekday(w: &str) -> Option<Weekday> {
    Some(match w {
        "mon" | "monday" | "mondays" => Weekday::Mon,
        "tue" | "tues" | "tuesday" | "tuesdays" => Weekday::Tue,
        "wed" | "weds" | "wednesday" | "wednesdays" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" | "thursdays" => Weekday::Thu,
        "fri" | "friday" | "fridays" => Weekday::Fri,
        "sat" | "saturday" | "saturdays" => Weekday::Sat,
        "sun" | "sunday" | "sundays" => Weekday::Sun,
        _ => return None,
    })
}

fn month_name(w: &str) -> Option<u32> {
    Some(match w {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

fn small_number(w: &str) -> Option<u32> {
    if let Ok(n) = w.parse::<u32>() {
        return (1..=999).contains(&n).then_some(n);
    }
    Some(match w {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        _ => return None,
    })
}

/// `1st`…`31st`.
fn ordinal(w: &str) -> Option<u32> {
    let digits = w.strip_suffix("st").or_else(|| w.strip_suffix("nd")).or_else(|| w.strip_suffix("rd")).or_else(|| w.strip_suffix("th"))?;
    let n: u32 = digits.parse().ok()?;
    (1..=31).contains(&n).then_some(n)
}

/// `1`…`31` or an ordinal.
fn day_number(w: &str) -> Option<u32> {
    ordinal(w).or_else(|| w.parse::<u32>().ok().filter(|n| (1..=31).contains(n) && w.len() <= 2))
}

fn nth_word(w: &str) -> Option<i8> {
    Some(match w {
        "first" | "1st" => 1,
        "second" | "2nd" => 2,
        "third" | "3rd" => 3,
        "fourth" | "4th" => 4,
        "fifth" | "5th" => 5,
        "last" => -1,
        _ => return None,
    })
}

fn part_of_day(w: &str) -> Option<NaiveTime> {
    let h = match w {
        "morning" => 9,
        "noon" | "midday" => 12,
        "afternoon" => 13,
        "evening" => 18,
        "night" | "tonight" => 20,
        _ => return None,
    };
    NaiveTime::from_hms_opt(h, 0, 0)
}

fn hm(h: u32, m: u32) -> Option<NaiveTime> {
    NaiveTime::from_hms_opt(h, m, 0)
}

/// `5pm`, `5:30pm`, `17:00`, `5:30` (24 h), `noon`; with a following `am`/`pm` word.
fn time_at(t: &[Tok], i: usize) -> Option<(NaiveTime, usize)> {
    let w = t.get(i)?.norm.as_str();
    if w == "noon" || w == "midday" {
        return Some((hm(12, 0)?, i + 1));
    }
    let (num, suffix) = match w.find(|c: char| c.is_ascii_alphabetic()) {
        Some(p) => (&w[..p], &w[p..]),
        None => (w, ""),
    };
    let suffix = suffix.replace('.', "");
    let (h, m, colon) = match num.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?, true),
        Some(_) => return None,
        None => (num.parse::<u32>().ok()?, 0, false),
    };
    if num.is_empty() || num.len() > 5 {
        return None;
    }
    let (meridiem, next) = match suffix.as_str() {
        "am" | "a" => (Some(false), i + 1),
        "pm" | "p" => (Some(true), i + 1),
        "" => match t.get(i + 1).map(|x| x.norm.replace('.', "")) {
            Some(s) if s == "am" => (Some(false), i + 2),
            Some(s) if s == "pm" => (Some(true), i + 2),
            _ => (None, i + 1),
        },
        "h" if !colon => (None, i + 1),
        _ => return None,
    };
    let time = match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&h) || m > 59 {
                return None;
            }
            hm(if pm { if h == 12 { 12 } else { h + 12 } } else if h == 12 { 0 } else { h }, m)?
        }
        // Without am/pm only `17:00`-style (or `17h`) counts as a time: a bare number is too ambiguous.
        None if colon || suffix == "h" => hm(h, m)?,
        None => return None,
    };
    Some((time, next))
}

/// A time with an optional leading `at`/`@`.
fn at_time(t: &[Tok], i: usize) -> Option<(NaiveTime, usize)> {
    match t.get(i).map(|x| x.norm.as_str()) {
        Some("at" | "@") => time_at(t, i + 1),
        _ => time_at(t, i),
    }
}

fn last_day(y: i32, m: u32) -> u32 {
    let first_next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, m + 1, 1) };
    first_next.and_then(|d| d.pred_opt()).map(|d| d.day()).unwrap_or(28)
}

/// Day `d` of a month (`-1` = last day); days past the month's end clamp to its last day.
fn clamp_day(y: i32, m: u32, d: i32) -> Option<NaiveDate> {
    let last = last_day(y, m) as i32;
    let day = if d < 0 { (last + 1 + d).max(1) } else { d.min(last) };
    NaiveDate::from_ymd_opt(y, m, day as u32)
}

/// The `n`th weekday of a month (`-1` = last), if the month has one.
fn nth_weekday(y: i32, m: u32, n: i8, wd: Weekday) -> Option<NaiveDate> {
    if n < 0 {
        let mut d = NaiveDate::from_ymd_opt(y, m, last_day(y, m))?;
        while d.weekday() != wd {
            d = d.pred_opt()?;
        }
        return Some(d);
    }
    let first = NaiveDate::from_ymd_opt(y, m, 1)?;
    let offset = (7 + wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64) % 7;
    let d = first + Duration::days(offset + 7 * (n as i64 - 1));
    (d.month() == m).then_some(d)
}

/// The next `wd` strictly after `from` (or on `from` when `inclusive`).
fn next_weekday(from: NaiveDate, wd: Weekday, inclusive: bool) -> NaiveDate {
    let diff = (7 + wd.num_days_from_monday() as i64 - from.weekday().num_days_from_monday() as i64) % 7;
    let diff = if diff == 0 && !inclusive { 7 } else { diff };
    from + Duration::days(diff)
}

fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

fn add_months(d: NaiveDate, n: u32) -> NaiveDate {
    d.checked_add_months(Months::new(n)).unwrap_or(d)
}

/// The next date with this month-day on/after `from` (next year if it already passed).
fn upcoming_month_day(from: NaiveDate, m: u32, d: u32) -> Option<NaiveDate> {
    let this = NaiveDate::from_ymd_opt(from.year(), m, d);
    match this {
        Some(x) if x >= from => Some(x),
        _ => (1..=8).find_map(|k| NaiveDate::from_ymd_opt(from.year() + k, m, d)),
    }
}

/// The next date with this day of the month on/after `from`, skipping months that lack it.
fn upcoming_day_of_month(from: NaiveDate, d: u32) -> Option<NaiveDate> {
    (0..14).find_map(|k| {
        let base = add_months(NaiveDate::from_ymd_opt(from.year(), from.month(), 1)?, k);
        NaiveDate::from_ymd_opt(base.year(), base.month(), d).filter(|x| *x >= from)
    })
}

struct Parser<'a> {
    t: &'a [Tok],
    ctx: &'a DateCtx,
}

impl<'a> Parser<'a> {
    fn w(&self, i: usize) -> &str {
        self.t.get(i).map(|x| x.norm.as_str()).unwrap_or("")
    }

    /// A 4-digit year (optionally after a comma).
    fn year(&self, i: usize) -> Option<(i32, usize)> {
        let j = if self.w(i) == "," { i + 1 } else { i };
        let w = self.w(j);
        let y: i32 = w.parse().ok().filter(|_| w.len() == 4)?;
        (1970..=2999).contains(&y).then_some((y, j + 1))
    }

    /// `in 3 days`, `in a week`, `in 2h`, `in 30 min`.
    fn relative(&self, i: usize) -> Option<(NaiveDate, Option<NaiveTime>, usize)> {
        if self.w(i) != "in" {
            return None;
        }
        let (n, unit, next) = match small_number(self.w(i + 1)) {
            Some(n) => (n, self.w(i + 2).to_string(), i + 3),
            None => {
                let w = self.w(i + 1);
                let p = w.find(|c: char| !c.is_ascii_digit())?;
                (w[..p].parse().ok()?, w[p..].to_string(), i + 2)
            }
        };
        let today = self.ctx.today;
        let now = today.and_time(self.ctx.now);
        Some(match unit.as_str() {
            "day" | "days" | "d" => (today + Duration::days(n as i64), None, next),
            "week" | "weeks" | "w" | "wk" | "wks" => (today + Duration::days(7 * n as i64), None, next),
            "month" | "months" | "mo" | "mos" => (add_months(today, n), None, next),
            "year" | "years" | "y" | "yr" | "yrs" => (add_months(today, 12 * n), None, next),
            "hour" | "hours" | "h" | "hr" | "hrs" => {
                let t = now + Duration::hours(n as i64);
                (t.date(), Some(t.time()), next)
            }
            "minute" | "minutes" | "min" | "mins" | "m" => {
                let t = now + Duration::minutes(n as i64);
                (t.date(), Some(t.time()), next)
            }
            _ => return None,
        })
    }

    /// A calendar date (and an implied time for `tonight`, `this evening`).
    fn date_part(&self, i: usize) -> Option<(NaiveDate, Option<NaiveTime>, usize)> {
        let today = self.ctx.today;
        let w = self.w(i);
        let one = |d: NaiveDate| Some((d, None, i + 1));
        match w {
            "" => return None,
            "today" | "tod" | "tdy" => return one(today),
            "tonight" => return Some((today, part_of_day("tonight"), i + 1)),
            "tomorrow" | "tom" | "tmr" | "tmrw" | "tmw" | "tomorow" => return one(today + Duration::days(1)),
            "yesterday" => return one(today - Duration::days(1)),
            "eom" => return one(clamp_day(today.year(), today.month(), -1)?),
            "eow" => return one(next_weekday(today, Weekday::Fri, true)),
            "weekend" => return one(self.weekend(false)),
            "this" => {
                let n = self.w(i + 1);
                if let Some(wd) = weekday(n) {
                    return Some((next_weekday(today, wd, true), None, i + 2));
                }
                if n == "weekend" {
                    return Some((self.weekend(false), None, i + 2));
                }
                if let Some(t) = part_of_day(n).filter(|_| n != "noon" && n != "midday") {
                    return Some((today, Some(t), i + 2));
                }
                return None;
            }
            "next" => {
                let n = self.w(i + 1);
                if let Some(wd) = weekday(n) {
                    return Some((next_weekday(today, wd, false), None, i + 2));
                }
                return match n {
                    "week" => Some((next_weekday(today, Weekday::Mon, false), None, i + 2)),
                    "weekend" => Some((self.weekend(true), None, i + 2)),
                    "month" => Some((add_months(NaiveDate::from_ymd_opt(today.year(), today.month(), 1)?, 1), None, i + 2)),
                    "year" => Some((NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)?, None, i + 2)),
                    _ => None,
                };
            }
            "end" if self.w(i + 1) == "of" => {
                let k = if self.w(i + 2) == "the" { i + 3 } else { i + 2 };
                return match self.w(k) {
                    "month" => Some((clamp_day(today.year(), today.month(), -1)?, None, k + 1)),
                    "week" => Some((next_weekday(today, Weekday::Fri, true), None, k + 1)),
                    "year" => Some((NaiveDate::from_ymd_opt(today.year(), 12, 31)?, None, k + 1)),
                    _ => None,
                };
            }
            "in" => return self.relative(i),
            _ => {}
        }
        if let Some(wd) = weekday(w) {
            return one(next_weekday(today, wd, false));
        }
        // ISO 2026-01-15 / 2026/01/15.
        if w.len() == 10
            && let Some(d) = NaiveDate::parse_from_str(w, "%Y-%m-%d").ok().or_else(|| NaiveDate::parse_from_str(w, "%Y/%m/%d").ok())
        {
            return one(d);
        }
        // jan 15[th][, 2027]
        if let Some(m) = month_name(w)
            && let Some(d) = day_number(self.w(i + 1))
        {
            return self.with_year(m, d, i + 2);
        }
        // 15[th] [of] jan[, 2027]
        if let Some(d) = day_number(w) {
            let k = if self.w(i + 1) == "of" { i + 2 } else { i + 1 };
            if let Some(m) = month_name(self.w(k)) {
                return self.with_year(m, d, k + 1);
            }
        }
        // the 15th / 15th
        let (k, w) = if w == "the" { (i + 1, self.w(i + 1)) } else { (i, w) };
        if let Some(d) = ordinal(w) {
            if let Some(m) = (self.w(k + 1) == "of").then(|| month_name(self.w(k + 2))).flatten() {
                return self.with_year(m, d, k + 3);
            }
            return Some((upcoming_day_of_month(today, d)?, None, k + 1));
        }
        None
    }

    fn with_year(&self, m: u32, d: u32, i: usize) -> Option<(NaiveDate, Option<NaiveTime>, usize)> {
        match self.year(i) {
            Some((y, j)) => Some((NaiveDate::from_ymd_opt(y, m, d)?, None, j)),
            None => Some((upcoming_month_day(self.ctx.today, m, d)?, None, i)),
        }
    }

    fn weekend(&self, next: bool) -> NaiveDate {
        let today = self.ctx.today;
        let this = match today.weekday() {
            Weekday::Sat | Weekday::Sun => today,
            _ => next_weekday(today, Weekday::Sat, false),
        };
        if next { monday_of(today) + Duration::days(12) } else { this }
    }

    /// `<date> [morning] [at 5pm]` or `<time> [<date>]`.
    fn single(&self, i: usize) -> Option<(DueSpec, usize)> {
        if let Some((date, implied, mut j)) = self.date_part(i) {
            let mut time = implied;
            if time.is_none()
                && let Some(t) = part_of_day(self.w(j))
            {
                time = Some(t);
                j += 1;
            }
            if let Some((t, k)) = at_time(self.t, j) {
                time = Some(t);
                j = k;
            }
            return Some((DueSpec { date, time, recurrence: None }, j));
        }
        let (time, j) = at_time(self.t, i)?;
        let on = if self.w(j) == "on" { j + 1 } else { j };
        if let Some((date, _, k)) = self.date_part(on) {
            return Some((DueSpec { date, time: Some(time), recurrence: None }, k));
        }
        let date = if time > self.ctx.now { self.ctx.today } else { self.ctx.today + Duration::days(1) };
        Some((DueSpec { date, time: Some(time), recurrence: None }, j))
    }

    /// `mon`, `mon, wed`, `mon and wed & fri`.
    fn weekday_list(&self, i: usize) -> Option<(Vec<Day>, usize)> {
        let mut days = vec![Day::from(weekday(self.w(i))?)];
        let mut j = i + 1;
        loop {
            let mut k = j;
            while matches!(self.w(k), "," | "and" | "&") {
                k += 1;
            }
            match weekday(self.w(k)) {
                Some(wd) if k > j => {
                    let d = Day::from(wd);
                    if !days.contains(&d) {
                        days.push(d);
                    }
                    j = k + 1;
                }
                _ => break,
            }
        }
        Some((days, j))
    }

    /// `1st`, `1st, 15th`, `1st and last day`.
    fn month_day_list(&self, i: usize) -> Option<(Vec<i8>, usize)> {
        let item = |k: usize| -> Option<(i8, usize)> {
            let k = if self.w(k) == "the" { k + 1 } else { k };
            if self.w(k) == "last" && self.w(k + 1) == "day" {
                return Some((-1, k + 2));
            }
            ordinal(self.w(k)).map(|d| (d as i8, k + 1))
        };
        let (first, mut j) = item(i)?;
        let mut days = vec![first];
        loop {
            let mut k = j;
            while matches!(self.w(k), "," | "and" | "&") {
                k += 1;
            }
            match item(k) {
                Some((d, next)) if k > j => {
                    if !days.contains(&d) {
                        days.push(d);
                    }
                    j = next;
                }
                _ => break,
            }
        }
        days.sort_by_key(|d| if *d < 0 { 99 } else { *d });
        Some((days, j))
    }

    /// Skips `of the month` / `of month` / `of each month`.
    fn of_month(&self, j: usize) -> usize {
        if self.w(j) != "of" {
            return j;
        }
        let k = if matches!(self.w(j + 1), "the" | "each" | "every") { j + 2 } else { j + 1 };
        if self.w(k) == "month" { k + 1 } else { j }
    }

    /// The part after `every`: returns the rule, an implied time (`every morning`) and the end.
    fn rule_body(&self, i: usize) -> Option<(Recurrence, Option<NaiveTime>, usize)> {
        let (n, i) = match self.w(i) {
            "other" => (2, i + 1),
            w => match small_number(w).filter(|_| !matches!(w, "a" | "an")) {
                // `every 15 jan` is a date, not an interval.
                Some(n) if month_name(self.w(i + 1)).is_none() => (n, i + 1),
                _ => (1, i),
            },
        };
        let w = self.w(i);
        let mut r;
        let mut j = i + 1;
        match w {
            "day" | "days" => r = Recurrence::new(Freq::Day),
            "weekday" | "weekdays" | "workday" | "workdays" => {
                r = Recurrence::new(Freq::Week);
                r.weekdays = vec![Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri];
            }
            "weekend" | "weekends" => {
                r = Recurrence::new(Freq::Week);
                r.weekdays = vec![Day::Sat, Day::Sun];
            }
            "week" | "weeks" => {
                r = Recurrence::new(Freq::Week);
                if self.w(j) == "on"
                    && let Some((days, k)) = self.weekday_list(j + 1)
                {
                    r.weekdays = days;
                    j = k;
                }
            }
            "month" | "months" => {
                r = Recurrence::new(Freq::Month);
                if self.w(j) == "on" {
                    let k = if self.w(j + 1) == "the" { j + 2 } else { j + 1 };
                    if let (Some(nth), Some(wd)) = (nth_word(self.w(k)), weekday(self.w(k + 1))) {
                        r.week_of_month = Some(nth);
                        r.weekdays = vec![wd.into()];
                        j = k + 2;
                    } else if let Some((days, k)) = self.month_day_list(j + 1) {
                        r.month_days = days;
                        j = k;
                    }
                }
            }
            "year" | "years" => {
                r = Recurrence::new(Freq::Year);
                if self.w(j) == "on"
                    && let Some((m, d, k)) = self.month_day(j + 1)
                {
                    r.month = Some(m as u8);
                    r.month_days = vec![d as i8];
                    j = k;
                }
            }
            "morning" | "afternoon" | "evening" | "night" if n == 1 => {
                return Some((Recurrence::new(Freq::Day), part_of_day(w), j));
            }
            _ => {
                if let Some((days, k)) = self.weekday_list(i) {
                    r = Recurrence::new(Freq::Week);
                    r.weekdays = days;
                    j = k;
                } else if let (Some(nth), Some(wd)) = (nth_word(w), weekday(self.w(i + 1))) {
                    r = Recurrence::new(Freq::Month);
                    r.week_of_month = Some(nth);
                    r.weekdays = vec![wd.into()];
                    j = self.of_month(i + 2);
                } else if let Some((days, k)) = self.month_day_list(i) {
                    r = Recurrence::new(Freq::Month);
                    r.month_days = days;
                    j = self.of_month(k);
                } else if let Some((m, d, k)) = self.month_day(i) {
                    r = Recurrence::new(Freq::Year);
                    r.month = Some(m as u8);
                    r.month_days = vec![d as i8];
                    j = k;
                } else {
                    return None;
                }
            }
        }
        r.interval = n;
        Some((r, None, j))
    }

    /// `jan 15`, `15 jan`, `15th of january`.
    fn month_day(&self, i: usize) -> Option<(u32, u32, usize)> {
        if let Some(m) = month_name(self.w(i)) {
            return day_number(self.w(i + 1)).map(|d| (m, d, i + 2));
        }
        let d = day_number(self.w(i))?;
        let k = if self.w(i + 1) == "of" { i + 2 } else { i + 1 };
        month_name(self.w(k)).map(|m| (m, d, k + 1))
    }

    /// `every …` / `daily` / `weekly` … with an optional time, `starting <date>` and `until <date>`.
    fn recurring(&self, i: usize, lead_time: Option<NaiveTime>) -> Option<(DueSpec, usize)> {
        let w = self.w(i);
        let (mut r, mut time, mut j) = match w {
            "every" | "every!" => {
                let (mut r, t, j) = self.rule_body(i + 1)?;
                r.from_completion = w == "every!";
                (r, t, j)
            }
            "daily" => (Recurrence::new(Freq::Day), None, i + 1),
            "weekly" => (Recurrence::new(Freq::Week), None, i + 1),
            "biweekly" | "fortnightly" => {
                let mut r = Recurrence::new(Freq::Week);
                r.interval = 2;
                (r, None, i + 1)
            }
            "monthly" => (Recurrence::new(Freq::Month), None, i + 1),
            "yearly" | "annually" => (Recurrence::new(Freq::Year), None, i + 1),
            _ => return None,
        };
        time = lead_time.or(time);
        if time.is_none()
            && let Some(t) = part_of_day(self.w(j)).filter(|_| self.w(j) != "tonight")
        {
            time = Some(t);
            j += 1;
        }
        if let Some((t, k)) = at_time(self.t, j) {
            time = Some(t);
            j = k;
        }
        let mut start = None;
        let mut explicit_start = false;
        for _ in 0..2 {
            match self.w(j) {
                "starting" | "from" | "starts" | "start" => {
                    let k = if self.w(j + 1) == "on" { j + 2 } else { j + 1 };
                    let (d, _, next) = self.date_part(k)?;
                    start = Some(d);
                    explicit_start = true;
                    j = next;
                }
                "until" | "till" | "ending" | "ends" | "through" => {
                    let k = if self.w(j + 1) == "on" { j + 2 } else { j + 1 };
                    let (d, _, next) = self.date_part(k)?;
                    r.until = Some(d);
                    j = next;
                }
                _ => break,
            }
        }
        let start = start.unwrap_or(self.ctx.today);
        // The first occurrence ignores the interval: `every other mon` starts on the next Monday,
        // which then anchors the series.
        let first = Recurrence { interval: 1, ..r.clone() };
        let mut date = first.next_after(start, start.pred_opt()?)?;
        if !explicit_start
            && date == self.ctx.today
            && let Some(t) = time
            && t <= self.ctx.now
            && let Some(next) = r.next_after(date, date)
        {
            date = next;
        }
        Some((DueSpec { date, time, recurrence: Some(r) }, j))
    }

    /// A due expression at `i` (with an optional leading `on`/`by`/`due`), or `no date`.
    fn due(&self, i: usize) -> Option<(Matched, usize)> {
        let w = self.w(i);
        if w == "no" && matches!(self.w(i + 1), "date" | "due") {
            let end = if self.w(i + 1) == "due" && self.w(i + 2) == "date" { i + 3 } else { i + 2 };
            return Some((Matched::NoDate, end));
        }
        if matches!(w, "on" | "by" | "due") {
            return self.due(i + 1).filter(|(m, _)| matches!(m, Matched::Due(_)));
        }
        if let Some((spec, j)) = self.recurring(i, None) {
            return Some((Matched::Due(spec), j));
        }
        // `9am every day`
        if let Some((t, j)) = at_time(self.t, i)
            && let Some((spec, k)) = self.recurring(j, Some(t))
        {
            return Some((Matched::Due(spec), k));
        }
        self.single(i).map(|(s, j)| (Matched::Due(s), j))
    }
}

/// Finds a due expression starting at token `i`; returns it and the index after it.
pub(crate) fn match_at(toks: &[Tok], i: usize, ctx: &DateCtx) -> Option<(Matched, usize)> {
    Parser { t: toks, ctx }.due(i)
}

/// Parses a whole string as a due date (`tomorrow 5pm`, `every mon`). `Ok(None)` for empty text or
/// `no date`.
pub fn parse_due(text: &str, ctx: &DateCtx) -> Result<Option<Due>, String> {
    let toks = tokenize(text);
    if toks.is_empty() {
        return Ok(None);
    }
    let p = Parser { t: &toks, ctx };
    match p.due(0) {
        Some((Matched::NoDate, j)) if j == toks.len() => Ok(None),
        Some((Matched::Due(spec), j)) if j == toks.len() => Ok(Some(spec.into_due(text))),
        _ => {
            let words: Vec<&str> = toks.iter().map(|t| t.norm.as_str()).collect();
            if matches!(words.first(), Some(&"every" | &"every!")) && words.iter().any(|w| matches!(*w, "hour" | "hours" | "minute" | "minutes" | "hr" | "hrs")) {
                return Err("hourly recurrence isn't supported — use a daily rule with a time".into());
            }
            Err(format!("couldn't understand the date “{}”", text.trim()))
        }
    }
}

/// Parses a single calendar date (deadlines): `jan 30`, `fri`, `in 2 weeks`, `2026-01-30`.
pub fn parse_date(text: &str, ctx: &DateCtx) -> Result<Option<NaiveDate>, String> {
    let toks = tokenize(text);
    if toks.is_empty() {
        return Ok(None);
    }
    let p = Parser { t: &toks, ctx };
    let k = if matches!(p.w(0), "on" | "by") { 1 } else { 0 };
    match p.date_part(k) {
        Some((d, _, j)) if j == toks.len() => Ok(Some(d)),
        _ if toks.len() == 2 && p.w(0) == "no" && matches!(p.w(1), "date" | "deadline") => Ok(None),
        _ => Err(format!("couldn't understand the date “{}”", text.trim())),
    }
}

impl Recurrence {
    /// No explicit weekday/day/month: the rule follows the due date itself.
    pub fn is_plain(&self) -> bool {
        self.weekdays.is_empty() && self.month_days.is_empty() && self.week_of_month.is_none() && self.month.is_none()
    }

    /// The first occurrence strictly after `after` in the series anchored at `anchor` (an occurrence
    /// itself: the current due date). `None` when the series ended (`until`).
    pub fn next_after(&self, anchor: NaiveDate, after: NaiveDate) -> Option<NaiveDate> {
        let n = self.interval.max(1) as i64;
        let next = match self.freq {
            Freq::Day => {
                let diff = (after - anchor).num_days();
                let k = if diff < 0 { 0 } else { diff / n + 1 };
                Some(anchor + Duration::days(k * n))
            }
            Freq::Week => {
                let days: Vec<Weekday> = if self.weekdays.is_empty() { vec![anchor.weekday()] } else { self.weekdays.iter().map(|d| (*d).into()).collect() };
                let a = monday_of(anchor);
                (1..=(14 * n + 7)).map(|k| after + Duration::days(k)).find(|d| days.contains(&d.weekday()) && ((monday_of(*d) - a).num_days() / 7).rem_euclid(n) == 0)
            }
            Freq::Month => {
                let mi = |d: NaiveDate| d.year() as i64 * 12 + d.month0() as i64;
                let (am, start) = (mi(anchor), mi(after));
                (0..(48 * n + 24)).find_map(|k| {
                    let m = start + k;
                    if (m - am).rem_euclid(n) != 0 {
                        return None;
                    }
                    let (y, mo) = (m.div_euclid(12) as i32, (m.rem_euclid(12) + 1) as u32);
                    let mut c: Vec<NaiveDate> = if let Some(nth) = self.week_of_month {
                        let wd: Weekday = self.weekdays.first().copied().map(Into::into).unwrap_or(anchor.weekday());
                        nth_weekday(y, mo, nth, wd).into_iter().collect()
                    } else if self.month_days.is_empty() {
                        clamp_day(y, mo, anchor.day() as i32).into_iter().collect()
                    } else {
                        self.month_days.iter().filter_map(|d| clamp_day(y, mo, *d as i32)).collect()
                    };
                    c.sort();
                    c.into_iter().find(|d| *d > after)
                })
            }
            Freq::Year => {
                let m = self.month.map(u32::from).unwrap_or(anchor.month());
                let day = self.month_days.first().map(|d| *d as i32).unwrap_or(anchor.day() as i32);
                (after.year()..after.year() + 8 * n as i32 + 8)
                    .filter(|y| ((*y - anchor.year()) as i64).rem_euclid(n) == 0)
                    .filter_map(|y| clamp_day(y, m, day))
                    .find(|d| *d > after)
            }
        }?;
        self.until.is_none_or(|u| next <= u).then_some(next)
    }

    /// Adds one interval (for `every!` rules without fixed days).
    fn add_interval(&self, d: NaiveDate) -> NaiveDate {
        let n = self.interval.max(1);
        match self.freq {
            Freq::Day => d + Days::new(n as u64),
            Freq::Week => d + Days::new(7 * n as u64),
            Freq::Month => add_months(d, n),
            Freq::Year => add_months(d, 12 * n),
        }
    }
}

/// The due date after completing a recurring task on `completed_on`: from the due date (skipping
/// occurrences that are already past), or from the completion date for `every!`. `None` when the
/// task isn't recurring or the series ended.
pub fn next_occurrence(due: &Due, completed_on: NaiveDate) -> Option<NaiveDate> {
    let r = due.recurrence.as_ref()?;
    let next = if r.from_completion {
        if r.is_plain() { Some(r.add_interval(completed_on)) } else { r.next_after(completed_on, completed_on) }
    } else {
        r.next_after(due.date, due.date.max(completed_on))
    }?;
    r.until.is_none_or(|u| next <= u).then_some(next)
}

/// Resolves a wall-clock time in `tz`. In a DST gap the time moves forward by the gap's length
/// (02:30 on a spring-forward night is 03:30, like JavaScript's `Temporal` "compatible" mode); when
/// ambiguous (clocks going back) the earlier instant wins.
pub fn resolve_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => Some(t),
        chrono::LocalResult::Ambiguous(a, _) => Some(a),
        chrono::LocalResult::None => {
            let before = tz.from_local_datetime(&(naive - Duration::hours(4))).earliest()?;
            let offset = before.offset().fix().local_minus_utc() as i64;
            Some(tz.from_utc_datetime(&(naive - Duration::seconds(offset))))
        }
    }
}

/// When a dated task with a time is due, as an instant. Floating times use `local`; fixed ones
/// their own `timezone`.
pub fn due_instant<Tz: TimeZone>(due: &Due, local: &Tz) -> Option<DateTime<Utc>> {
    let naive = due.date.and_time(due.time?);
    match due.timezone.as_deref().and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        Some(z) => resolve_local(&z, naive).map(|t| t.with_timezone(&Utc)),
        None => resolve_local(local, naive).map(|t| t.with_timezone(&Utc)),
    }
}

/// The local calendar date a task is due on (differs from `due.date` only for fixed-zone times).
pub fn local_due_date<Tz: TimeZone>(due: &Due, local: &Tz) -> NaiveDate {
    match (&due.timezone, due.time) {
        (Some(_), Some(_)) => due_instant(due, local).map(|t| t.with_timezone(local).date_naive()).unwrap_or(due.date),
        _ => due.date,
    }
}

#[cfg(test)]
mod tests;

//! The calendar, live (ADR-008, spec K8): locus reads kairos for Home's next event, the rhythm
//! grid (time measured from your Claude sessions) and the events of each application, through
//! kairosd's API until the HTTP service exists; what locus puts on the calendar goes through
//! kairosd too, never around it (kairos's single-writer rule).
//!
//! The rest of the window still runs on fixtures (Slice 1): the calendar is laid over them when
//! kairosd answers, and the fixtures stand when it does not. `--dump` and the tests never read it.

use crate::fixtures::Fixtures;
use chrono::{DateTime, Datelike, Duration, FixedOffset, Utc};
use kairos_core::config::Config;
use kairos_core::model::{EventKind, Instance, Span};
use kairos_store::client::Daemon;
use locus_contract::{AppEvent, AppEventKind, Next, ProjectKind, WorkDay, WorkSpan};

/// A connection to kairosd, from kairos's own config (`$KAIROS_CONFIG` or the default path).
pub struct Live {
    pub cfg: Config,
    daemon: Daemon,
}

impl Live {
    pub fn connect() -> Option<Live> {
        let cfg = Config::load(&Config::default_path()).ok()?;
        let daemon = Daemon::from_config(&cfg).ok()?;
        Some(Live { cfg, daemon })
    }

    /// The calendar from 90 days back to 60 ahead: past interviews stay on their application.
    pub fn fetch(&self, now: DateTime<Utc>) -> Option<Vec<Instance>> {
        let (idx, _) = self
            .daemon
            .index(now - Duration::days(90), now + Duration::days(60), None)
            .ok()??;
        Some(idx.instances)
    }

    /// Put something on the calendar for a locus item, through kairosd (your keypress: it applies
    /// as your own edit would, with an undo in kairos).
    pub fn add(&self, title: &str, at: DateTime<FixedOffset>, key: &str, locus: &str) -> Result<String, String> {
        let req = kairos_core::api::CreateEventReq {
            title: title.into(),
            start: at.to_rfc3339(),
            duration_min: Some(60),
            key: Some(key.into()),
            locus: Some(locus.into()),
            ..Default::default()
        };
        let d = Daemon::from_config(&self.cfg)
            .map_err(|e| e.to_string())?
            .interactive(true);
        match d.create_event(&req) {
            Ok(Ok(r)) => Ok(r.summary),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Every task kairos holds, open and done this week (spec E7).
    pub fn tasks(&self) -> Vec<kairos_core::api::TaskOverview> {
        match self.daemon.tasks_overview() {
            Ok(Ok(v)) => v,
            _ => Vec::new(),
        }
    }

    /// Mark a task done, from the Tasks page (your keypress, as in kairos).
    pub fn complete(&self, id: &str) -> Result<(), String> {
        let d = Daemon::from_config(&self.cfg)
            .map_err(|e| e.to_string())?
            .interactive(true);
        match d.complete_task(id, None) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Open chores (spec K9).
    pub fn chores(&self) -> Vec<kairos_core::api::ChoreItem> {
        match self.daemon.chores() {
            Ok(Ok(v)) => v,
            _ => Vec::new(),
        }
    }

    pub fn area_of(&self, key: &str) -> Option<String> {
        self.cfg.key(key).map(|k| k.area().to_string())
    }
}

/// What an event on an application's timeline is, from its title.
fn kind_of(title: &str) -> AppEventKind {
    let t = title.to_lowercase();
    if t.contains("interview") || t.contains("final round") || t.contains("superday") {
        AppEventKind::Interview
    } else if t.contains("test") || t.contains("assessment") || t.contains("hirevue") {
        AppEventKind::Assessment
    } else if t.contains("deadline") || t.contains("submit") || t.contains("closes") {
        AppEventKind::Submission
    } else {
        AppEventKind::Call
    }
}

/// kairos's tasks onto the Tasks page (spec E7), and each course's next due onto Studies, so
/// what is due shows on Home where it belongs. `area_of` maps a kairos key to its area.
pub fn apply_tasks(
    fx: &mut Fixtures,
    tasks: &[kairos_core::api::TaskOverview],
    area_of: impl Fn(&str) -> Option<String>,
) {
    let off = fx.now().offset().to_owned();
    let at = |t: DateTime<Utc>| t.with_timezone(&off);
    let items: Vec<locus_contract::TaskItem> = tasks
        .iter()
        .map(|o| {
            let t = &o.task;
            let entry = t.locus.as_deref().and_then(|l| fx.registry.get(l));
            let (due, due_is_date) = match t.due {
                Some(kairos_core::task::Due::At(d)) => (Some(at(d)), false),
                Some(kairos_core::task::Due::Date(d)) => (
                    d.and_hms_opt(23, 59, 0)
                        .and_then(|n| n.and_local_timezone(off).single()),
                    true,
                ),
                None => (None, false),
            };
            locus_contract::TaskItem {
                id: t.id.clone(),
                title: t.title.clone(),
                for_id: t.locus.clone(),
                for_label: entry.map_or_else(|| t.key.clone(), |p| p.name.clone()),
                area: entry
                    .map(|p| p.area.clone())
                    .or_else(|| area_of(&t.key))
                    .unwrap_or_else(|| "Personal".into()),
                due,
                due_is_date,
                before: o.before_title.clone(),
                left_min: t.remaining_min,
                estimate_min: t.estimate_min,
                next_block: o.next_block.map(at),
                put_off: o.put_off,
                done: (t.status == kairos_core::task::TaskStatus::Done).then(|| at(t.updated_at)),
            }
        })
        .collect();
    // A course's open tasks with a due date are what Studies shows as due.
    let courses: Vec<String> = fx
        .registry
        .projects
        .iter()
        .filter(|p| p.kind == ProjectKind::Course)
        .map(|p| p.id.clone())
        .collect();
    for id in courses {
        let Some(pf) = fx.projects.get_mut(&id) else { continue };
        let mut due: Vec<locus_contract::Due> = items
            .iter()
            .filter(|t| t.done.is_none() && t.for_id.as_deref() == Some(id.as_str()))
            .filter_map(|t| {
                Some(locus_contract::Due {
                    title: t.title.clone(),
                    kind: if t.title.to_lowercase().contains("sheet") {
                        locus_contract::DueKind::Sheet
                    } else {
                        locus_contract::DueKind::Coursework
                    },
                    at: t.due?.date_naive(),
                })
            })
            .collect();
        if !due.is_empty() {
            due.sort_by_key(|d| d.at);
            pf.view.due = due;
        }
    }
    fx.tasks.tasks = items;
}

/// Chores onto Home (spec K9): the one that fits the gap you are in now (the most put off), and
/// those overdue or put off for "waiting for you".
pub fn apply_chores(fx: &mut Fixtures, chores: &[kairos_core::api::ChoreItem]) {
    let chore = |c: &kairos_core::api::ChoreItem| locus_contract::Chore {
        title: c.title.clone(),
        minutes: c.minutes,
        put_off: c.put_off,
        overdue: c.overdue,
    };
    fx.home.chore_now = chores.iter().find(|c| c.fits_now).map(chore);
    fx.home.chores_waiting = chores
        .iter()
        .filter(|c| c.overdue || c.put_off > 0)
        .map(chore)
        .collect();
}

/// Lay the calendar over the fixtures. `area_of` maps a kairos key to its locus area.
pub fn apply(fx: &mut Fixtures, cal: &[Instance], area_of: impl Fn(&str) -> Option<String>, now: DateTime<Utc>) {
    let off = fx.now().offset().to_owned();
    let at = |t: DateTime<Utc>| t.with_timezone(&off);
    // Home's next event: the first timed thing of yours still to come.
    let next = cal
        .iter()
        .filter(|i| i.meta.kind == EventKind::Event && !i.meta.not_going && !i.transparent)
        .filter(|i| i.overlay == kairos_core::config::Overlay::Personal || i.meta.blocking)
        .filter_map(|i| match i.span {
            Span::Timed { start, .. } if start >= now => Some((start, i)),
            _ => None,
        })
        .min_by_key(|(s, _)| *s);
    fx.home.next = next.map(|(s, i)| Next {
        title: i.title.clone(),
        at: at(s),
        area: i.key.as_deref().and_then(&area_of).unwrap_or_else(|| "Personal".into()),
    });
    // The rhythm grid: this week's measured time, day by day.
    let today = at(now).date_naive();
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let worked: Vec<&Instance> = cal.iter().filter(|i| i.meta.kind == EventKind::Worked).collect();
    if !worked.is_empty() {
        let reg = &fx.registry;
        // Each span in half hours, shaded by how busy that half hour was (░ ▒ ▓ █): its lines
        // of session against the quartiles of this week's half hours, so a quiet week and a
        // frantic one both use the four fills.
        struct Piece {
            date: chrono::NaiveDate,
            start: chrono::NaiveTime,
            end: chrono::NaiveTime,
            project: String,
            lines: Option<u32>,
        }
        let mut pieces = Vec::new();
        for i in &worked {
            let Span::Timed { start, end } = i.span else { continue };
            let project = reg
                .projects
                .iter()
                .find(|p| p.name == i.title)
                .map_or_else(|| i.title.clone(), |p| p.id.clone());
            let mut t = start;
            let mut k = 0usize;
            while t < end {
                let next = (t + Duration::minutes(30)).min(end);
                let (s, e) = (at(t), at(next));
                if s.date_naive() >= monday {
                    let lines = (!i.meta.activity.is_empty())
                        .then(|| i.meta.activity.iter().skip(k * 3).take(3).map(|n| u32::from(*n)).sum());
                    pieces.push(Piece {
                        date: s.date_naive(),
                        start: s.time(),
                        // Past midnight: it ends with the day on the grid.
                        end: if e.date_naive() == s.date_naive() {
                            e.time()
                        } else {
                            chrono::NaiveTime::from_hms_opt(23, 59, 0).unwrap_or(chrono::NaiveTime::MIN)
                        },
                        project: project.clone(),
                        lines,
                    });
                }
                t = next;
                k += 1;
            }
        }
        let mut counts: Vec<u32> = pieces.iter().filter_map(|p| p.lines).collect();
        counts.sort_unstable();
        let q = |f: f64| {
            counts
                .get(((counts.len().saturating_sub(1)) as f64 * f) as usize)
                .copied()
                .unwrap_or(0)
        };
        let (q1, q2, q3) = (q(0.25), q(0.5), q(0.75));
        let intensity = |l: Option<u32>| match l {
            None => 2,
            Some(n) if n <= q1 => 1,
            Some(n) if n <= q2 => 2,
            Some(n) if n <= q3 => 3,
            Some(_) => 4,
        };
        fx.work.week = (0..=(today - monday).num_days())
            .map(|d| {
                let date = monday + Duration::days(d);
                let spans = pieces
                    .iter()
                    .filter(|p| p.date == date)
                    .map(|p| WorkSpan {
                        start: p.start,
                        end: p.end,
                        project: p.project.clone(),
                        intensity: intensity(p.lines),
                    })
                    .collect();
                WorkDay { date, spans }
            })
            .collect();
    }
    // Each application's events: the calendar's, replacing what an earlier pass added.
    let apps: Vec<String> = fx
        .registry
        .projects
        .iter()
        .filter(|p| p.kind == ProjectKind::Application)
        .map(|p| p.id.clone())
        .collect();
    for id in apps {
        let Some(app) = fx.projects.get_mut(&id).and_then(|p| p.view.application.as_mut()) else {
            continue;
        };
        app.events.retain(|e| e.calendar.is_none());
        for i in cal.iter().filter(|i| i.meta.locus.as_deref() == Some(id.as_str())) {
            let (start, _) = i.span.bounds(chrono_tz::Europe::London);
            app.events.push(AppEvent {
                at: at(start),
                kind: kind_of(&i.title),
                text: i.title.clone(),
                detail: vec!["on your calendar (kairos)".into()],
                calendar: Some(i.uid.clone()),
            });
        }
        app.events.sort_by(|a, b| b.at.cmp(&a.at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kairos_core::model::Meta;

    fn ev(uid: &str, title: &str, start: &str, mins: i64, meta: Meta) -> Instance {
        let s: DateTime<Utc> = start.parse().unwrap();
        Instance {
            uid: uid.into(),
            recurrence_id: None,
            source: "careers".into(),
            overlay: kairos_core::config::Overlay::Personal,
            key: Some("Careers".into()),
            title: title.into(),
            location: None,
            description: None,
            span: Span::Timed {
                start: s,
                end: s + Duration::minutes(mins),
            },
            recurring: false,
            transparent: meta.kind == EventKind::Worked,
            tentative: false,
            meta,
            href: None,
            alarms: Vec::new(),
        }
    }

    #[test]
    fn the_calendar_fills_next_the_rhythm_grid_and_an_applications_events() {
        let mut fx = Fixtures::load(&crate::fixtures::resolve("default")).unwrap();
        let now: DateTime<Utc> = "2026-10-24T10:00:00Z".parse().unwrap();
        let linked = Meta {
            locus: Some("millennium".into()),
            ..Meta::default()
        };
        let worked = Meta {
            kind: EventKind::Worked,
            // Ten-minute counts: a quiet half hour, a busy one, a middling one.
            activity: vec![1, 1, 1, 20, 20, 20, 5, 5, 5],
            ..Meta::default()
        };
        let cal = vec![
            ev("past", "Lecture", "2026-10-24T08:00:00Z", 60, Meta::default()),
            ev("intv", "Millennium final interview", "2026-10-27T09:00:00Z", 60, linked),
            ev("w1", "locus", "2026-10-24T07:00:00Z", 90, worked),
        ];
        apply(&mut fx, &cal, |k| (k == "Careers").then(|| "Careers".into()), now);
        let next = fx.home.next.clone().unwrap();
        assert_eq!(
            (next.title.as_str(), next.area.as_str()),
            ("Millennium final interview", "Careers")
        );
        let today = fx.work.week.last().unwrap();
        assert!(today.spans.iter().all(|s| s.project == "locus"));
        let shades: Vec<u8> = today.spans.iter().map(|s| s.intensity).collect();
        assert_eq!(shades, [1, 4, 2], "half hours shaded by how busy they were");
        let events = &fx.projects["millennium"].view.application.as_ref().unwrap().events;
        let e = events.iter().find(|e| e.calendar.as_deref() == Some("intv")).unwrap();
        assert_eq!(e.kind, AppEventKind::Interview);
        // A second pass replaces, never duplicates.
        apply(&mut fx, &cal, |_| None, now);
        let events = &fx.projects["millennium"].view.application.as_ref().unwrap().events;
        assert_eq!(events.iter().filter(|e| e.calendar.is_some()).count(), 1);
    }
}

#[cfg(test)]
mod chore_tests {
    use crate::app::App;
    use crate::fixtures::{Fixtures, resolve};
    use crate::screen::ScreenId;
    use crate::uselog::UseLog;

    fn home(fx: Fixtures, w: u16) -> Vec<String> {
        let app = App::new(fx, ScreenId::Home, UseLog::off());
        crate::dump::render(&app, w, 40)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn a_chore_on_the_top_line_never_pushes_anything_out() {
        // Spec K9's check: at 95, 120 and 170 columns, with a chore that fits now, everything
        // the top line showed without it is still there, uncut; a put-off chore waits for you.
        let base = Fixtures::load(&resolve("default")).unwrap();
        let mut with = base.clone();
        let laundry = kairos_core::api::ChoreItem {
            id: "t-1".into(),
            title: "laundry".into(),
            minutes: 20,
            due: None,
            put_off: 2,
            overdue: false,
            fits_now: true,
        };
        super::apply_chores(&mut with, &[laundry]);
        let mut shown_somewhere = false;
        for w in [95u16, 120, 170] {
            let (a, b) = (home(base.clone(), w), home(with.clone(), w));
            // Every word of the top line without the chore is on the top line with it.
            for word in a[0]
                .split(|c: char| c == '─' || c.is_whitespace())
                .filter(|s| s.len() > 1)
            {
                assert!(b[0].contains(word), "{w} cols: {word:?} lost:\n{}\n{}", a[0], b[0]);
            }
            shown_somewhere |= b[0].contains("laundry");
            let waiting = b.iter().any(|l| l.contains("laundry (put off 2×)"));
            assert!(waiting, "{w} cols: not waiting for you:\n{}", b.join("\n"));
        }
        assert!(shown_somewhere, "a wide window has room for it");
    }
}

#[cfg(test)]
mod task_tests {
    use crate::app::App;
    use crate::fixtures::{Fixtures, resolve};
    use crate::screen::ScreenId;
    use crate::uselog::UseLog;
    use chrono::{DateTime, Utc};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn task(
        id: &str,
        title: &str,
        key: &str,
        locus: Option<&str>,
        due: Option<&str>,
    ) -> kairos_core::api::TaskOverview {
        let now: DateTime<Utc> = "2026-10-24T09:00:00Z".parse().unwrap();
        let mut t = kairos_core::task::Task::new(title, key, 300, kairos_core::model::Author::Alec, now);
        t.id = id.into();
        t.locus = locus.map(String::from);
        t.due = due.map(|d| kairos_core::task::Due::At(d.parse().unwrap()));
        kairos_core::api::TaskOverview {
            task: t,
            next_block: Some("2026-10-25T13:00:00Z".parse().unwrap()),
            put_off: 0,
            before_title: None,
        }
    }

    #[test]
    fn tasks_are_grouped_by_what_they_are_for_and_a_courses_due_shows_in_studies() {
        // Spec E7: the Tasks page, and Home showing it (the line in waiting for you, the
        // course's next due in Studies).
        let mut fx = Fixtures::load(&resolve("default")).unwrap();
        let tasks = vec![
            task(
                "t-1",
                "B15 sheet 4",
                "Studies",
                Some("b15"),
                Some("2026-10-30T16:00:00Z"),
            ),
            task("t-2", "laundry", "Errands", None, None),
            task(
                "t-3",
                "Millennium prep",
                "Careers",
                Some("millennium"),
                Some("2026-10-27T09:00:00Z"),
            ),
        ];
        super::apply_tasks(&mut fx, &tasks, |k| (k == "Errands").then(|| "Personal".into()));
        assert_eq!(fx.tasks.tasks.len(), 3);
        let b15 = fx.tasks.tasks.iter().find(|t| t.id == "t-1").unwrap();
        assert_eq!(
            (b15.for_label.as_str(), b15.area.as_str()),
            ("B15 Control Systems", "Studies")
        );
        assert_eq!(fx.projects["b15"].view.due[0].title, "B15 sheet 4");
        let mut app = App::new(fx, ScreenId::Home, UseLog::off());
        let home = crate::dump::render(&app, 120, 40).unwrap();
        assert!(home.contains("☐ 3 open"), "{home}");
        // d focuses the line; Enter opens the page.
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.screen(), ScreenId::Tasks);
        let page = crate::dump::render(&app, 120, 40).unwrap();
        // The group due soonest first: Millennium, then B15, then Errands (no due date).
        let pos = |s: &str| page.find(s).unwrap_or_else(|| panic!("{s} missing:\n{page}"));
        assert!(pos("Millennium QD Internship 2027") < pos("B15 Control Systems"));
        assert!(pos("B15 Control Systems") < pos("Errands"));
        assert!(
            page.contains("next sun 14:00") || page.contains("next sun 13:00"),
            "{page}"
        );
        // Outside the live window Enter and d only say what they would do.
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.note.as_deref().is_some_and(|n| n.starts_with("would open kairos")),
            "{:?}",
            app.note
        );
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(
            app.note.as_deref().is_some_and(|n| n.starts_with("would mark")),
            "{:?}",
            app.note
        );
    }
}

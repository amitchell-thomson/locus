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
        fx.work.week = (0..=(today - monday).num_days())
            .map(|d| {
                let date = monday + Duration::days(d);
                let spans = worked
                    .iter()
                    .filter_map(|i| {
                        let Span::Timed { start, end } = i.span else {
                            return None;
                        };
                        let (s, e) = (at(start), at(end));
                        (s.date_naive() == date).then(|| WorkSpan {
                            start: s.time(),
                            // Past midnight: it ends with the day on the grid.
                            end: if e.date_naive() == date {
                                e.time()
                            } else {
                                chrono::NaiveTime::from_hms_opt(23, 59, 0).unwrap_or(chrono::NaiveTime::MIN)
                            },
                            // The worked bar's title is the registry name; the grid wants the id.
                            project: reg
                                .projects
                                .iter()
                                .find(|p| p.name == i.title)
                                .map_or_else(|| i.title.clone(), |p| p.id.clone()),
                            intensity: 3,
                        })
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
        assert_eq!(today.spans.len(), 1);
        assert_eq!(today.spans[0].project, "locus");
        let events = &fx.projects["millennium"].view.application.as_ref().unwrap().events;
        let e = events.iter().find(|e| e.calendar.as_deref() == Some("intv")).unwrap();
        assert_eq!(e.kind, AppEventKind::Interview);
        // A second pass replaces, never duplicates.
        apply(&mut fx, &cal, |_| None, now);
        let events = &fx.projects["millennium"].view.application.as_ref().unwrap().events;
        assert_eq!(events.iter().filter(|e| e.calendar.is_some()).count(), 1);
    }
}

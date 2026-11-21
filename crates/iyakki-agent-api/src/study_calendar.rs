//! The four study-plan calls that touch the calendar (F-SPLIT-03, D-SPLIT-03-X1 option c): the study store's own methods take the concrete
//! `CalendarStore`, which cannot cross the core socket, so the screens call this trait instead. The core holds both stores and does the work
//! (`StudyOverCalendar`); a UI sees only ids, a zone and counts. Nothing here hands out a store.

use std::sync::Arc;

use iyakki_clock::Zone;
use iyakki_shared::calendar::CalendarStore;
use iyakki_shared::study::StudyStore;

/// A study plan and its events in the person's calendar.
pub trait StudyCalendarApi: Send + Sync {
    /// Delete the plan and every calendar event it made; the number of events removed.
    fn delete_plan(&self, user: &str, id: &str) -> Result<usize, String>;
    /// Put every session of the plan in the calendar; the number of events added.
    fn to_calendar(&self, user: &str, id: &str, zone: &Zone) -> Result<usize, String>;
    /// As `to_calendar`, each event ringing `remind` minutes before it starts (0 = at the start).
    fn to_calendar_with(&self, user: &str, id: &str, zone: &Zone, remind: Option<u32>) -> Result<usize, String>;
    /// Take the plan's events out of the calendar again; the plan stays.
    fn calendar_remove(&self, user: &str, id: &str) -> Result<usize, String>;
}

/// The core's two stores as one `StudyCalendarApi`.
pub struct StudyOverCalendar {
    pub study: Arc<StudyStore>,
    pub calendar: Arc<CalendarStore>,
}

impl StudyCalendarApi for StudyOverCalendar {
    fn delete_plan(&self, user: &str, id: &str) -> Result<usize, String> {
        self.study.delete_plan(user, id, &self.calendar)
    }
    fn to_calendar(&self, user: &str, id: &str, zone: &Zone) -> Result<usize, String> {
        self.study.to_calendar(user, id, &self.calendar, zone)
    }
    fn to_calendar_with(&self, user: &str, id: &str, zone: &Zone, remind: Option<u32>) -> Result<usize, String> {
        self.study.to_calendar_with(user, id, &self.calendar, zone, remind)
    }
    fn calendar_remove(&self, user: &str, id: &str) -> Result<usize, String> {
        self.study.calendar_remove(user, id, &self.calendar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_goes_into_the_calendar_and_out_again_through_the_trait_and_the_events_are_the_stores_own() {
        let d = std::env::temp_dir().join(format!("iyakki-studycal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (study, calendar) = (Arc::new(StudyStore::new(&d.join("study"))), Arc::new(CalendarStore::new(&d.join("calendar"))));
        let both: Arc<dyn StudyCalendarApi> = Arc::new(StudyOverCalendar { study: study.clone(), calendar: calendar.clone() });
        let zone = Zone::utc();
        let spec = iyakki_shared::study::PlanSpec::from_args(&serde_json::json!({
            "topic": "Rust", "unit": "days", "count": 2, "start": "2030-01-01",
            "sessions": [{"title": "One", "notes": "a"}, {"title": "Two", "notes": "b"}]
        }))
        .unwrap();
        let plan = study.create_plan("alice", &spec, 1_800_000_000, &zone).unwrap();
        assert_eq!(both.to_calendar("alice", &plan.id, &zone).unwrap(), 2);
        assert_eq!(calendar.events("alice").len(), 2, "the trait wrote into the store the core holds");
        assert!(both.to_calendar("alice", &plan.id, &zone).is_err(), "twice is refused by name");
        assert_eq!(both.calendar_remove("alice", &plan.id).unwrap(), 2);
        assert!(calendar.events("alice").is_empty());
        assert_eq!(both.to_calendar_with("alice", &plan.id, &zone, Some(10)).unwrap(), 2);
        assert_eq!(both.delete_plan("alice", &plan.id).unwrap(), 2);
        assert!(calendar.events("alice").is_empty() && study.plans("alice").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}

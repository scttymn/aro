//! `calendar`: CalendarProvider (`android.content.IContentProvider`) and CalendarContract service.
//!
//! Exposes android.content.IContentProvider for authority `com.android.calendar`
//! to provide calendar accounts, event storage, event instances, and reminders.
//! Persists calendar data across runs in `Layout.data/system/users/0/calendar.json`.

use super::cursor::{write_query_reply, CellValue, CursorWindowBuilder};
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarAccount {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub account_name: String,
    pub account_type: String,
    pub owner_account: String,
    pub color: i32,
    pub timezone: String,
    pub visible: bool,
    pub sync_events: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: i64,
    pub calendar_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub location: String,
    pub dtstart: i64,
    pub dtend: i64,
    pub all_day: bool,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub end_timezone: Option<String>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub has_alarm: bool,
    #[serde(default)]
    pub color: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarReminder {
    pub id: i64,
    pub event_id: i64,
    pub minutes: i32,
    pub method: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarData {
    pub calendars: Vec<CalendarAccount>,
    pub events: Vec<CalendarEvent>,
    pub reminders: Vec<CalendarReminder>,
    pub next_event_id: i64,
    pub next_reminder_id: i64,
}

fn get_local_timezone() -> String {
    if let Ok(link) = std::fs::read_link("/etc/localtime") {
        if let Some(s) = link.to_str() {
            if let Some(pos) = s.find("zoneinfo/") {
                return s[pos + 9..].to_string();
            }
        }
    }
    "US/Central".to_string()
}

fn to_local_julian_and_minute(millis: i64) -> (i64, i32) {
    let secs = millis / 1000;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time_val = secs as libc::time_t;
    unsafe {
        libc::localtime_r(&time_val, &mut tm);
    }
    let gmtoff = tm.tm_gmtoff;
    let local_millis = millis + (gmtoff * 1000);
    let julian_day = 2440588 + (local_millis / 86_400_000);
    let minute = tm.tm_hour * 60 + tm.tm_min;
    (julian_day, minute)
}

impl CalendarData {
    pub fn default_data() -> Self {
        let tz = get_local_timezone();
        let default_cal = CalendarAccount {
            id: 1,
            name: "Local Calendar".into(),
            display_name: "Personal".into(),
            account_name: "local".into(),
            account_type: "LOCAL".into(),
            owner_account: "local@aro".into(),
            color: -14575885, // 0xFF00897B teal
            timezone: tz.clone(),
            visible: true,
            sync_events: true,
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let start1 = (now / 3_600_000) * 3_600_000 + 1_800_000;
        let end1 = start1 + 3_600_000;
        let start2 = start1 + 86_400_000;
        let end2 = start2 + 3_600_000;

        let sample1 = CalendarEvent {
            id: 1,
            calendar_id: 1,
            title: "ARO Verification".into(),
            description: "Verify Calendar app and provider in ARO runtime".into(),
            location: "Linux Desktop".into(),
            dtstart: start1,
            dtend: end1,
            all_day: true,
            duration: None,
            timezone: tz.clone(),
            end_timezone: None,
            rrule: None,
            has_alarm: false,
            color: Some(-14575885),
        };
        let sample2 = CalendarEvent {
            id: 2,
            calendar_id: 1,
            title: "Team Standup".into(),
            description: "Daily team check-in".into(),
            location: "Office".into(),
            dtstart: start2,
            dtend: end2,
            all_day: true,
            duration: None,
            timezone: tz,
            end_timezone: None,
            rrule: None,
            has_alarm: false,
            color: Some(-11751600),
        };

        Self {
            calendars: vec![default_cal],
            events: vec![sample1, sample2],
            reminders: Vec::new(),
            next_event_id: 3,
            next_reminder_id: 1,
        }
    }
}

pub struct CalendarService {
    path: PathBuf,
    data: Mutex<CalendarData>,
}

impl CalendarService {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("system/users/0/calendar.json");
        let default_data = CalendarData::default_data();
        let loaded = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<CalendarData>(&content) {
                    Ok(mut file) => {
                        if file.calendars.is_empty() {
                            file.calendars = default_data.calendars;
                        }
                        if file.events.is_empty() {
                            file.events = default_data.events;
                            if file.next_event_id <= 2 {
                                file.next_event_id = 3;
                            }
                        }
                        file
                    }
                    Err(e) => {
                        log::warn!("calendar: failed to parse {}: {e}", path.display());
                        default_data
                    }
                },
                Err(e) => {
                    log::warn!("calendar: failed to read {}: {e}", path.display());
                    default_data
                }
            }
        } else {
            default_data
        };

        let svc = Self {
            path,
            data: Mutex::new(loaded),
        };
        svc.persist();
        svc
    }

    fn persist(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let guard = self.data.lock().unwrap();
        if let Ok(json) = serde_json::to_string_pretty(&*guard) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    pub fn insert_event(&self, mut event: CalendarEvent) -> i64 {
        let mut guard = self.data.lock().unwrap();
        let id = guard.next_event_id;
        guard.next_event_id += 1;
        event.id = id;
        guard.events.push(event);
        drop(guard);
        self.persist();
        id
    }

    pub fn update_event(&self, id: i64, updater: impl FnOnce(&mut CalendarEvent)) -> bool {
        let mut guard = self.data.lock().unwrap();
        if let Some(ev) = guard.events.iter_mut().find(|e| e.id == id) {
            updater(ev);
            drop(guard);
            self.persist();
            true
        } else {
            false
        }
    }

    pub fn delete_event(&self, id: i64) -> bool {
        let mut guard = self.data.lock().unwrap();
        let initial_len = guard.events.len();
        guard.events.retain(|e| e.id != id);
        guard.reminders.retain(|r| r.event_id != id);
        let removed = guard.events.len() < initial_len;
        drop(guard);
        if removed {
            self.persist();
        }
        removed
    }

    pub fn insert_reminder(&self, mut reminder: CalendarReminder) -> i64 {
        let mut guard = self.data.lock().unwrap();
        let id = guard.next_reminder_id;
        guard.next_reminder_id += 1;
        reminder.id = id;
        guard.reminders.push(reminder);
        drop(guard);
        self.persist();
        id
    }
}

pub struct CalendarProvider {
    pub service: Arc<CalendarService>,
    pub bulk_cursor: SIBinder,
}

fn read_uri(p: &mut Parcel) -> Option<String> {
    let type_id = p.read_i32().ok()?;
    match type_id {
        0 => None,
        1 | 2 | 3 => ap::read_string8(p).ok().flatten(),
        _ => None,
    }
}

fn write_uri(p: &mut Parcel, uri: Option<&str>) -> Result<()> {
    match uri {
        None => p.write_i32(0),
        Some(s) => {
            p.write_i32(1)?; // TYPE_ID = 1 (StringUri)
            ap::string8(p, Some(s))?;
            Ok(())
        }
    }
}

fn read_content_values(p: &mut Parcel) -> std::collections::HashMap<String, CellValue> {
    let mut map = std::collections::HashMap::new();
    let size = p.read_i32().unwrap_or(0);
    if size <= 0 {
        return map;
    }
    let n = p.read_i32().unwrap_or(0);
    for _ in 0..n {
        let key = ap::read_string16(p).ok().flatten().unwrap_or_default();
        let val_type = p.read_i32().unwrap_or(-1);
        let val = match val_type {
            -1 => CellValue::Null,
            0 => {
                let s = ap::read_string16(p).ok().flatten().unwrap_or_default();
                CellValue::String(s)
            }
            1 => {
                let i = p.read_i32().unwrap_or(0);
                CellValue::Integer(i as i64)
            }
            6 => {
                let l = p.read_i64().unwrap_or(0);
                CellValue::Integer(l)
            }
            7 => {
                let f = p.read_f32().unwrap_or(0.0);
                CellValue::Float(f as f64)
            }
            8 => {
                let d = p.read_f64().unwrap_or(0.0);
                CellValue::Float(d)
            }
            9 => {
                let b = p.read_i32().unwrap_or(0) != 0;
                CellValue::Integer(if b { 1 } else { 0 })
            }
            _ => CellValue::Null,
        };
        map.insert(key, val);
    }
    map
}

fn get_str<'a>(map: &'a std::collections::HashMap<String, CellValue>, key: &str) -> Option<&'a str> {
    match map.get(key)? {
        CellValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn get_i64(map: &std::collections::HashMap<String, CellValue>, key: &str) -> Option<i64> {
    match map.get(key)? {
        CellValue::Integer(i) => Some(*i),
        CellValue::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn get_bool(map: &std::collections::HashMap<String, CellValue>, key: &str) -> bool {
    match map.get(key) {
        Some(CellValue::Integer(i)) => *i != 0,
        Some(CellValue::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

impl Service for CalendarProvider {
    const DESCRIPTOR: &'static str = "android.content.IContentProvider";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "QUERY"),
        (2, "GET_TYPE"),
        (3, "INSERT"),
        (4, "DELETE"),
        (10, "UPDATE"),
        (13, "BULK_INSERT"),
        (20, "APPLY_BATCH"),
        (21, "CALL"),
        (25, "CANONICALIZE"),
        (26, "UNCANONICALIZE"),
        (29, "GET_TYPE_ASYNC"),
        (30, "CANONICALIZE_ASYNC"),
        (31, "UNCANONICALIZE_ASYNC"),
        (32, "GET_TYPE_ANONYMOUS_ASYNC"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "QUERY" => {
                // 1. Skip AttributionSource
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }

                // 2. Read Uri
                let uri = read_uri(data);

                // 3. Read projection
                let proj_len = data.read_i32().unwrap_or(0);
                let mut projection = Vec::new();
                for _ in 0..proj_len {
                    if let Ok(Some(col)) = data.read::<Option<String>>() {
                        projection.push(col);
                    }
                }

                // 4. Read queryArgs bundle bytes
                let bundle_len = data.read_i32().unwrap_or(-1);
                let _bundle_bytes = if bundle_len > 0 {
                    let mut b = Vec::with_capacity(bundle_len as usize);
                    let count = (bundle_len as usize + 3) / 4;
                    for _ in 0..count {
                        if let Ok(val) = data.read_u32() {
                            b.extend_from_slice(&val.to_le_bytes());
                        }
                    }
                    b.truncate(bundle_len as usize);
                    b
                } else {
                    Vec::new()
                };

                let uri_str = uri.as_deref().unwrap_or_default();
                log::info!("calendar: QUERY uri={uri_str:?} proj={projection:?} bundle_len={bundle_len}");

                let guard = self.service.data.lock().unwrap();

                if uri_str.contains("calendars") {
                    let cols = if projection.is_empty() {
                        vec![
                            "_id".into(),
                            "name".into(),
                            "calendar_displayName".into(),
                            "calendar_color".into(),
                            "calendar_access_level".into(),
                            "visible".into(),
                            "sync_events".into(),
                            "account_name".into(),
                            "account_type".into(),
                            "ownerAccount".into(),
                        ]
                    } else {
                        projection
                    };

                    let mut window = CursorWindowBuilder::new("calendar_window", cols.clone());
                    for cal in &guard.calendars {
                        let mut row = Vec::new();
                        for col in &cols {
                            match col.as_str() {
                                "_id" => row.push(CellValue::Integer(cal.id)),
                                "name" => row.push(CellValue::String(cal.name.clone())),
                                "calendar_displayName" => row.push(CellValue::String(cal.display_name.clone())),
                                "account_name" => row.push(CellValue::String(cal.account_name.clone())),
                                "account_type" => row.push(CellValue::String(cal.account_type.clone())),
                                "ownerAccount" => row.push(CellValue::String(cal.owner_account.clone())),
                                "calendar_color" => row.push(CellValue::Integer(cal.color as i64)),
                                "calendar_color_index" => row.push(CellValue::String("1".into())),
                                "calendar_access_level" => row.push(CellValue::Integer(700)), // CAL_ACCESS_OWNER
                                "visible" => row.push(CellValue::Integer(if cal.visible { 1 } else { 0 })),
                                "sync_events" => row.push(CellValue::Integer(if cal.sync_events { 1 } else { 0 })),
                                "calendar_timezone" => row.push(CellValue::String(cal.timezone.clone())),
                                "canOrganizerRespond" => row.push(CellValue::Integer(1)),
                                "canModifyTimeZone" => row.push(CellValue::Integer(1)),
                                "maxReminders" => row.push(CellValue::Integer(5)),
                                "allowedReminders" => row.push(CellValue::String("0,1".into())),
                                "allowedAvailability" => row.push(CellValue::String("0,1".into())),
                                "allowedAttendeeTypes" => row.push(CellValue::String("0,1,2".into())),
                                "isPrimary" => row.push(CellValue::Integer(1)),
                                "deleted" => row.push(CellValue::Integer(0)),
                                _ => row.push(CellValue::Null),
                            }
                        }
                        window.add_row(row);
                    }
                    return write_query_reply(reply, &self.bulk_cursor, &cols, guard.calendars.len(), window);
                }

                if uri_str.contains("instances") {
                    let parts: Vec<&str> = uri_str.split('/').collect();
                    let mut begin = 0i64;
                    let mut end = i64::MAX;
                    if let Some(pos) = parts.iter().position(|&p| p == "when") {
                        if let Some(b_str) = parts.get(pos + 1) {
                            begin = b_str.parse().unwrap_or(0);
                        }
                        if let Some(e_str) = parts.get(pos + 2) {
                            end = e_str.parse().unwrap_or(i64::MAX);
                        }
                    } else if let Some(pos) = parts.iter().position(|&p| p == "whenbyday") {
                        if let Some(s_str) = parts.get(pos + 1) {
                            let start_day: i64 = s_str.parse().unwrap_or(2440588);
                            begin = (start_day - 2440588 - 1) * 86_400_000;
                        }
                        if let Some(e_str) = parts.get(pos + 2) {
                            let end_day: i64 = e_str.parse().unwrap_or(2440588 + 36500);
                            end = (end_day - 2440588 + 2) * 86_400_000;
                        }
                    }

                    log::info!("calendar: QUERY instances window [{begin}..{end}] total_events={}", guard.events.len());

                    let matching: Vec<&CalendarEvent> = guard
                        .events
                        .iter()
                        .filter(|e| e.dtstart < end && (e.dtend > begin || (e.dtend == e.dtstart && e.dtstart >= begin && e.dtstart <= end)))
                        .collect();

                    let cols = if projection.is_empty() {
                        vec![
                            "_id".into(),
                            "event_id".into(),
                            "begin".into(),
                            "end".into(),
                            "title".into(),
                            "description".into(),
                            "eventLocation".into(),
                            "allDay".into(),
                            "hasAlarm".into(),
                            "calendar_color".into(),
                            "displayColor".into(),
                            "startDay".into(),
                            "endDay".into(),
                            "startMinute".into(),
                            "endMinute".into(),
                        ]
                    } else {
                        projection
                    };

                    let default_color = guard.calendars.first().map(|c| c.color).unwrap_or(-14575885);
                    let mut window = CursorWindowBuilder::new("instances_window", cols.clone());

                    for ev in &matching {
                        let color = ev.color.unwrap_or(default_color);
                        let (start_day, start_minute) = to_local_julian_and_minute(ev.dtstart);
                        let (mut end_day, mut end_minute) = to_local_julian_and_minute(ev.dtend);
                        if end_minute == 0 && end_day > start_day {
                            end_minute = 1440;
                            end_day -= 1;
                        }

                        let mut row = Vec::new();
                        for col in &cols {
                            match col.as_str() {
                                "_id" => row.push(CellValue::Integer(ev.id)),
                                "event_id" => row.push(CellValue::Integer(ev.id)),
                                "begin" => row.push(CellValue::Integer(ev.dtstart)),
                                "end" => row.push(CellValue::Integer(ev.dtend)),
                                "title" => row.push(CellValue::String(ev.title.clone())),
                                "description" => row.push(CellValue::String(ev.description.clone())),
                                "eventLocation" => row.push(CellValue::String(ev.location.clone())),
                                "allDay" => row.push(CellValue::Integer(if ev.all_day { 1 } else { 0 })),
                                "hasAlarm" => row.push(CellValue::Integer(if ev.has_alarm { 1 } else { 0 })),
                                "calendar_color" => row.push(CellValue::Integer(default_color as i64)),
                                "eventColor" => row.push(CellValue::Integer(color as i64)),
                                "displayColor" => row.push(CellValue::Integer(color as i64)),
                                "calendar_id" => row.push(CellValue::Integer(ev.calendar_id)),
                                "startDay" => row.push(CellValue::Integer(start_day)),
                                "endDay" => row.push(CellValue::Integer(end_day)),
                                "startMinute" => row.push(CellValue::Integer(start_minute as i64)),
                                "endMinute" => row.push(CellValue::Integer(end_minute as i64)),
                                "selfAttendeeStatus" => row.push(CellValue::Integer(1)),
                                "organizer" => row.push(CellValue::String("local@aro".into())),
                                "guestsCanModify" => row.push(CellValue::Integer(1)),
                                "rrule" => row.push(CellValue::String(ev.rrule.as_deref().unwrap_or("").to_string())),
                                "rdate" => row.push(CellValue::String("".to_string())),
                                "calendar_access_level" => row.push(CellValue::Integer(700)),
                                "calendar_displayName" => row.push(CellValue::String("Local Calendar".into())),
                                "account_name" => row.push(CellValue::String("local".into())),
                                "account_type" => row.push(CellValue::String("LOCAL".into())),
                                "ownerAccount" => row.push(CellValue::String("local@aro".into())),
                                "canOrganizerRespond" => row.push(CellValue::Integer(1)),
                                "canModifyEvent" => row.push(CellValue::Integer(1)),
                                "maxReminders" => row.push(CellValue::Integer(5)),
                                "allowedReminders" => row.push(CellValue::String("0,1".into())),
                                c if c.contains("dispAllday") => {
                                    let is_disp_all_day = ev.all_day || (ev.dtend - ev.dtstart) >= 86_400_000;
                                    row.push(CellValue::Integer(if is_disp_all_day { 1 } else { 0 }));
                                }
                                _ => row.push(CellValue::Null),
                            }
                        }
                        window.add_row(row);
                    }
                    return write_query_reply(reply, &self.bulk_cursor, &cols, matching.len(), window);
                }

                if uri_str.contains("events") {
                    let specific_id = uri_str.rsplit('/').next().and_then(|s| s.parse::<i64>().ok());
                    let matching: Vec<&CalendarEvent> = if let Some(id) = specific_id {
                        guard.events.iter().filter(|e| e.id == id).collect()
                    } else {
                        guard.events.iter().collect()
                    };

                    let cols = if projection.is_empty() {
                        vec![
                            "_id".into(),
                            "calendar_id".into(),
                            "title".into(),
                            "description".into(),
                            "eventLocation".into(),
                            "dtstart".into(),
                            "dtend".into(),
                            "allDay".into(),
                            "hasAlarm".into(),
                            "eventTimezone".into(),
                            "displayColor".into(),
                        ]
                    } else {
                        projection
                    };

                    let default_color = guard.calendars.first().map(|c| c.color).unwrap_or(-14575885);
                    let mut window = CursorWindowBuilder::new("events_window", cols.clone());

                    for ev in &matching {
                        let color = ev.color.unwrap_or(default_color);
                        let mut row = Vec::new();
                        for col in &cols {
                            match col.as_str() {
                                "_id" => row.push(CellValue::Integer(ev.id)),
                                "calendar_id" => row.push(CellValue::Integer(ev.calendar_id)),
                                "title" => row.push(CellValue::String(ev.title.clone())),
                                "description" => row.push(CellValue::String(ev.description.clone())),
                                "eventLocation" => row.push(CellValue::String(ev.location.clone())),
                                "dtstart" => row.push(CellValue::Integer(ev.dtstart)),
                                "dtend" => row.push(CellValue::Integer(ev.dtend)),
                                "duration" => row.push(ev.duration.as_ref().map(|d| CellValue::String(d.clone())).unwrap_or(CellValue::Null)),
                                "eventTimezone" => row.push(CellValue::String(ev.timezone.clone())),
                                "eventEndTimezone" => row.push(ev.end_timezone.as_ref().map(|t| CellValue::String(t.clone())).unwrap_or(CellValue::Null)),
                                "allDay" => row.push(CellValue::Integer(if ev.all_day { 1 } else { 0 })),
                                "rrule" => row.push(ev.rrule.as_ref().map(|r| CellValue::String(r.clone())).unwrap_or(CellValue::Null)),
                                "hasAlarm" => row.push(CellValue::Integer(if ev.has_alarm { 1 } else { 0 })),
                                "lastDate" => row.push(CellValue::Integer(ev.dtend)),
                                "hasAttendeeData" => row.push(CellValue::Integer(0)),
                                "guestsCanModify" => row.push(CellValue::Integer(1)),
                                "guestsCanInviteOthers" => row.push(CellValue::Integer(1)),
                                "guestsCanSeeGuests" => row.push(CellValue::Integer(1)),
                                "organizer" => row.push(CellValue::String("local@aro".into())),
                                "isOrganizer" => row.push(CellValue::Integer(1)),
                                "deleted" => row.push(CellValue::Integer(0)),
                                "eventStatus" => row.push(CellValue::Integer(1)),
                                "selfAttendeeStatus" => row.push(CellValue::Integer(1)),
                                "calendar_color" => row.push(CellValue::Integer(default_color as i64)),
                                "eventColor" => row.push(CellValue::Integer(color as i64)),
                                "displayColor" => row.push(CellValue::Integer(color as i64)),
                                "calendar_access_level" => row.push(CellValue::Integer(700)),
                                "calendar_displayName" => row.push(CellValue::String("Local Calendar".into())),
                                "account_name" => row.push(CellValue::String("local".into())),
                                "account_type" => row.push(CellValue::String("LOCAL".into())),
                                "ownerAccount" => row.push(CellValue::String("local@aro".into())),
                                "canOrganizerRespond" => row.push(CellValue::Integer(1)),
                                "canModifyEvent" => row.push(CellValue::Integer(1)),
                                "maxReminders" => row.push(CellValue::Integer(5)),
                                "allowedReminders" => row.push(CellValue::String("0,1".into())),
                                "_sync_id" => row.push(CellValue::String("".into())),
                                "availability" => row.push(CellValue::Integer(0)),
                                "accessLevel" => row.push(CellValue::Integer(700)),
                                "customAppPackage" => row.push(CellValue::Null),
                                "customAppUri" => row.push(CellValue::Null),
                                "original_sync_id" => row.push(CellValue::Null),
                                _ => row.push(CellValue::Null),
                            }
                        }
                        window.add_row(row);
                    }
                    return write_query_reply(reply, &self.bulk_cursor, &cols, matching.len(), window);
                }

                if uri_str.contains("colors") {
                    let cols = if projection.is_empty() {
                        vec!["color_type".into(), "color_index".into(), "color".into()]
                    } else {
                        projection
                    };
                    let mut window = CursorWindowBuilder::new("colors_window", cols.clone());
                    let mut row = Vec::new();
                    for col in &cols {
                        match col.as_str() {
                            "color_type" => row.push(CellValue::Integer(0)),
                            "color_index" => row.push(CellValue::String("1".into())),
                            "color" => row.push(CellValue::Integer(-14575885)),
                            "account_name" => row.push(CellValue::String("local".into())),
                            "account_type" => row.push(CellValue::String("LOCAL".into())),
                            _ => row.push(CellValue::Null),
                        }
                    }
                    window.add_row(row);
                    return write_query_reply(reply, &self.bulk_cursor, &cols, 1, window);
                }

                if uri_str.contains("reminders") {
                    let cols = if projection.is_empty() {
                        vec!["_id".into(), "event_id".into(), "minutes".into(), "method".into()]
                    } else {
                        projection
                    };
                    let mut window = CursorWindowBuilder::new("reminders_window", cols.clone());
                    for rem in &guard.reminders {
                        let mut row = Vec::new();
                        for col in &cols {
                            match col.as_str() {
                                "_id" => row.push(CellValue::Integer(rem.id)),
                                "event_id" => row.push(CellValue::Integer(rem.event_id)),
                                "minutes" => row.push(CellValue::Integer(rem.minutes as i64)),
                                "method" => row.push(CellValue::Integer(rem.method as i64)),
                                _ => row.push(CellValue::Null),
                            }
                        }
                        window.add_row(row);
                    }
                    return write_query_reply(reply, &self.bulk_cursor, &cols, guard.reminders.len(), window);
                }

                if uri_str.contains("properties") {
                    let cols = if projection.is_empty() {
                        vec!["key".into(), "value".into()]
                    } else {
                        projection
                    };
                    let tz = get_local_timezone();
                    let prop_rows = [
                        ("timezoneType", "auto"),
                        ("timezoneInstances", tz.as_str()),
                        ("timezoneInstancesPrevious", tz.as_str()),
                    ];
                    let mut window = CursorWindowBuilder::new("properties_window", cols.clone());
                    for (k, v) in prop_rows {
                        let mut row = Vec::new();
                        for col in &cols {
                            match col.as_str() {
                                "key" => row.push(CellValue::String(k.to_string())),
                                "value" => row.push(CellValue::String(v.to_string())),
                                _ => row.push(CellValue::Null),
                            }
                        }
                        window.add_row(row);
                    }
                    return write_query_reply(reply, &self.bulk_cursor, &cols, prop_rows.len(), window);
                }

                // Generic empty cursor for any other table (attendees, etc.)
                let cols = if projection.is_empty() {
                    vec!["_id".into()]
                } else {
                    projection
                };
                let window = CursorWindowBuilder::new("empty_window", cols.clone());
                write_query_reply(reply, &self.bulk_cursor, &cols, 0, window)
            }
            "INSERT" => {
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }
                let uri = read_uri(data);
                let values = read_content_values(data);
                let uri_str = uri.as_deref().unwrap_or_default();
                log::info!("calendar: INSERT uri={uri_str:?} values={values:?}");

                let out_uri = if uri_str.contains("events") {
                    let title = get_str(&values, "title").unwrap_or("New Event").to_string();
                    let description = get_str(&values, "description").unwrap_or_default().to_string();
                    let location = get_str(&values, "eventLocation").unwrap_or_default().to_string();
                    let dtstart = get_i64(&values, "dtstart").unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    });
                    let dtend = get_i64(&values, "dtend").unwrap_or(dtstart + 3600_000);
                    let all_day = get_bool(&values, "allDay");
                    let timezone = get_str(&values, "eventTimezone").unwrap_or("UTC").to_string();
                    let rrule = get_str(&values, "rrule").map(|s| s.to_string());
                    let has_alarm = get_bool(&values, "hasAlarm");
                    let calendar_id = get_i64(&values, "calendar_id").unwrap_or(1);
                    let color = get_i64(&values, "eventColor").map(|c| c as i32);

                    let event = CalendarEvent {
                        id: 0,
                        calendar_id,
                        title,
                        description,
                        location,
                        dtstart,
                        dtend,
                        all_day,
                        duration: None,
                        timezone,
                        end_timezone: None,
                        rrule,
                        has_alarm,
                        color,
                    };
                    let id = self.service.insert_event(event);
                    log::info!("calendar: INSERT created event id={id}");
                    format!("content://com.android.calendar/events/{id}")
                } else if uri_str.contains("reminders") {
                    let event_id = get_i64(&values, "event_id").unwrap_or(0);
                    let minutes = get_i64(&values, "minutes").unwrap_or(10) as i32;
                    let method = get_i64(&values, "method").unwrap_or(1) as i32;
                    let reminder = CalendarReminder {
                        id: 0,
                        event_id,
                        minutes,
                        method,
                    };
                    let id = self.service.insert_reminder(reminder);
                    log::info!("calendar: INSERT created reminder id={id}");
                    format!("content://com.android.calendar/reminders/{id}")
                } else {
                    format!("{uri_str}/1")
                };

                ap::no_exception(reply)?;
                write_uri(reply, Some(&out_uri))?;
                Ok(true)
            }
            "UPDATE" => {
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }
                let uri = read_uri(data);
                let values = read_content_values(data);
                let uri_str = uri.as_deref().unwrap_or_default();
                log::info!("calendar: UPDATE uri={uri_str:?} values={values:?}");

                let specific_id = uri_str.rsplit('/').next().and_then(|s| s.parse::<i64>().ok());
                let updated = if let Some(id) = specific_id {
                    self.service.update_event(id, |ev| {
                        if let Some(t) = get_str(&values, "title") {
                            ev.title = t.to_string();
                        }
                        if let Some(d) = get_str(&values, "description") {
                            ev.description = d.to_string();
                        }
                        if let Some(l) = get_str(&values, "eventLocation") {
                            ev.location = l.to_string();
                        }
                        if let Some(s) = get_i64(&values, "dtstart") {
                            ev.dtstart = s;
                        }
                        if let Some(e) = get_i64(&values, "dtend") {
                            ev.dtend = e;
                        }
                        if values.contains_key("allDay") {
                            ev.all_day = get_bool(&values, "allDay");
                        }
                        if let Some(tz) = get_str(&values, "eventTimezone") {
                            ev.timezone = tz.to_string();
                        }
                        if let Some(r) = get_str(&values, "rrule") {
                            ev.rrule = Some(r.to_string());
                        }
                        if values.contains_key("hasAlarm") {
                            ev.has_alarm = get_bool(&values, "hasAlarm");
                        }
                    })
                } else {
                    false
                };

                ap::no_exception(reply)?;
                reply.write_i32(if updated { 1 } else { 0 })?;
                Ok(true)
            }
            "DELETE" => {
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }
                let uri = read_uri(data);
                let uri_str = uri.as_deref().unwrap_or_default();
                log::info!("calendar: DELETE uri={uri_str:?}");

                let specific_id = uri_str.rsplit('/').next().and_then(|s| s.parse::<i64>().ok());
                let deleted = if let Some(id) = specific_id {
                    self.service.delete_event(id)
                } else {
                    false
                };

                ap::no_exception(reply)?;
                reply.write_i32(if deleted { 1 } else { 0 })?;
                Ok(true)
            }
            "GET_TYPE" => {
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }
                let uri = read_uri(data);
                let uri_str = uri.as_deref().unwrap_or_default();
                let mime = if uri_str.contains("events") {
                    if uri_str.rsplit('/').next().and_then(|s| s.parse::<i64>().ok()).is_some() {
                        "vnd.android.cursor.item/event"
                    } else {
                        "vnd.android.cursor.dir/event"
                    }
                } else if uri_str.contains("calendars") {
                    "vnd.android.cursor.dir/calendar"
                } else {
                    "vnd.android.cursor.dir/unknown"
                };
                ap::no_exception(reply)?;
                ap::string16(reply, Some(mime))?;
                Ok(true)
            }
            "APPLY_BATCH" => {
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }
                let _authority = ap::read_string16(data).ok().flatten();
                let num_ops = data.read_i32().unwrap_or(0);
                log::info!("calendar: APPLY_BATCH authority={_authority:?} num_ops={num_ops}");

                let mut results: Vec<(Option<String>, Option<i32>)> = Vec::new();
                for op_idx in 0..num_ops {
                    let op_type = data.read_i32().unwrap_or(0);
                    let op_uri = read_uri(data);
                    let has_method = data.read_i32().unwrap_or(0);
                    if has_method != 0 {
                        let _ = ap::read_string8(data);
                    }
                    let has_arg = data.read_i32().unwrap_or(0);
                    if has_arg != 0 {
                        let _ = ap::read_string8(data);
                    }
                    let values = read_content_values(data);
                    let _extras = read_content_values(data);
                    let has_selection = data.read_i32().unwrap_or(0);
                    if has_selection != 0 {
                        let _ = ap::read_string8(data);
                    }
                    // SelectionArgs: SparseArray
                    let sparse_size = data.read_i32().unwrap_or(-1);
                    if sparse_size > 0 {
                        for _ in 0..sparse_size {
                            let _key = data.read_i32();
                            let val_type = data.read_i32().unwrap_or(-1);
                            if val_type == 0 {
                                let _ = ap::read_string16(data);
                            } else if val_type == 1 {
                                let _ = data.read_i32();
                            }
                        }
                    }
                    let has_expected_count = data.read_i32().unwrap_or(0);
                    if has_expected_count != 0 {
                        let _ = data.read_i32();
                    }
                    let _yield_allowed = data.read_i32().unwrap_or(0) != 0;
                    let _exception_allowed = data.read_i32().unwrap_or(0) != 0;

                    let uri_str = op_uri.as_deref().unwrap_or_default();
                    log::info!("calendar: APPLY_BATCH op[{op_idx}] type={op_type} uri={uri_str:?}");

                    match op_type {
                        1 => {
                            // INSERT
                            let out_uri = if uri_str.contains("events") {
                                let title = get_str(&values, "title").unwrap_or("New Event").to_string();
                                let description = get_str(&values, "description").unwrap_or_default().to_string();
                                let location = get_str(&values, "eventLocation").unwrap_or_default().to_string();
                                let dtstart = get_i64(&values, "dtstart").unwrap_or_else(|| {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as i64)
                                        .unwrap_or(0)
                                });
                                let dtend = get_i64(&values, "dtend").unwrap_or(dtstart + 3600_000);
                                let all_day = get_bool(&values, "allDay");
                                let timezone = get_str(&values, "eventTimezone").unwrap_or("UTC").to_string();
                                let rrule = get_str(&values, "rrule").map(|s| s.to_string());
                                let has_alarm = get_bool(&values, "hasAlarm");
                                let calendar_id = get_i64(&values, "calendar_id").unwrap_or(1);
                                let color = get_i64(&values, "eventColor").map(|c| c as i32);

                                let event = CalendarEvent {
                                    id: 0,
                                    calendar_id,
                                    title,
                                    description,
                                    location,
                                    dtstart,
                                    dtend,
                                    all_day,
                                    duration: None,
                                    timezone,
                                    end_timezone: None,
                                    rrule,
                                    has_alarm,
                                    color,
                                };
                                let id = self.service.insert_event(event);
                                log::info!("calendar: APPLY_BATCH INSERT created event id={id}");
                                format!("content://com.android.calendar/events/{id}")
                            } else if uri_str.contains("reminders") {
                                let event_id = get_i64(&values, "event_id").unwrap_or(0);
                                let minutes = get_i64(&values, "minutes").unwrap_or(10) as i32;
                                let method = get_i64(&values, "method").unwrap_or(1) as i32;
                                let reminder = CalendarReminder {
                                    id: 0,
                                    event_id,
                                    minutes,
                                    method,
                                };
                                let id = self.service.insert_reminder(reminder);
                                log::info!("calendar: APPLY_BATCH INSERT created reminder id={id}");
                                format!("content://com.android.calendar/reminders/{id}")
                            } else {
                                format!("{uri_str}/1")
                            };
                            results.push((Some(out_uri), None));
                        }
                        2 => {
                            // UPDATE
                            let specific_id = uri_str.rsplit('/').next().and_then(|s| s.parse::<i64>().ok());
                            let updated = if let Some(id) = specific_id {
                                self.service.update_event(id, |ev| {
                                    if let Some(t) = get_str(&values, "title") {
                                        ev.title = t.to_string();
                                    }
                                    if let Some(d) = get_str(&values, "description") {
                                        ev.description = d.to_string();
                                    }
                                    if let Some(l) = get_str(&values, "eventLocation") {
                                        ev.location = l.to_string();
                                    }
                                    if let Some(s) = get_i64(&values, "dtstart") {
                                        ev.dtstart = s;
                                    }
                                    if let Some(e) = get_i64(&values, "dtend") {
                                        ev.dtend = e;
                                    }
                                    if values.contains_key("allDay") {
                                        ev.all_day = get_bool(&values, "allDay");
                                    }
                                    if let Some(tz) = get_str(&values, "eventTimezone") {
                                        ev.timezone = tz.to_string();
                                    }
                                    if let Some(r) = get_str(&values, "rrule") {
                                        ev.rrule = Some(r.to_string());
                                    }
                                    if values.contains_key("hasAlarm") {
                                        ev.has_alarm = get_bool(&values, "hasAlarm");
                                    }
                                })
                            } else {
                                false
                            };
                            results.push((None, Some(if updated { 1 } else { 0 })));
                        }
                        3 => {
                            // DELETE
                            let specific_id = uri_str.rsplit('/').next().and_then(|s| s.parse::<i64>().ok());
                            let deleted = if let Some(id) = specific_id {
                                self.service.delete_event(id)
                            } else {
                                false
                            };
                            results.push((None, Some(if deleted { 1 } else { 0 })));
                        }
                        _ => {
                            results.push((None, Some(0)));
                        }
                    }
                }

                ap::no_exception(reply)?;
                reply.write_i32(results.len() as i32)?;
                for (res_uri, res_count) in results {
                    reply.write_i32(1)?; // non-null typed object
                    if let Some(ref u) = res_uri {
                        reply.write_i32(1)?;
                        write_uri(reply, Some(u))?;
                    } else {
                        reply.write_i32(0)?;
                    }
                    if let Some(c) = res_count {
                        reply.write_i32(1)?;
                        reply.write_i32(c)?;
                    } else {
                        reply.write_i32(0)?;
                    }
                    reply.write_i32(0)?; // extras = null
                    reply.write_i32(0)?; // exception = null
                }
                Ok(true)
            }
            "CANONICALIZE" | "UNCANONICALIZE" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "CALL" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // empty bundle
                Ok(true)
            }
            _ => {
                log::warn!("calendar: unhandled {name} (code {_code})");
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calendar_service_crud_and_persistence() {
        let temp_dir = std::env::temp_dir().join(format!("aro_cal_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let service = CalendarService::load(&temp_dir);

        // Check defaults
        {
            let data = service.data.lock().unwrap();
            assert_eq!(data.calendars.len(), 1);
            assert_eq!(data.calendars[0].owner_account, "local@aro");
            assert_eq!(data.events.len(), 2);
        }

        // Insert event
        let id = service.insert_event(CalendarEvent {
            id: 0,
            calendar_id: 1,
            title: "Project Review".into(),
            description: "Review Milestone 4".into(),
            location: "Room A".into(),
            dtstart: 1000000,
            dtend: 1003600,
            all_day: false,
            duration: None,
            timezone: "UTC".into(),
            end_timezone: None,
            rrule: None,
            has_alarm: true,
            color: Some(-14575885),
        });
        assert_eq!(id, 3);

        // Update event
        let updated = service.update_event(id, |ev| {
            ev.title = "Project Review - Completed".into();
        });
        assert!(updated);

        // Verify update in-memory and reload from JSON
        let reloaded = CalendarService::load(&temp_dir);
        {
            let data = reloaded.data.lock().unwrap();
            let ev = data.events.iter().find(|e| e.id == id).expect("event should exist");
            assert_eq!(ev.title, "Project Review - Completed");
        }

        // Delete event
        assert!(service.delete_event(id));
        {
            let data = service.data.lock().unwrap();
            assert!(data.events.iter().find(|e| e.id == id).is_none());
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_calendar_provider_batch_and_query() {
        let temp_dir = std::env::temp_dir().join(format!("aro_cal_prov_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let service = Arc::new(CalendarService::load(&temp_dir));

        let dummy = crate::services::binder_of(crate::services::cursor::BulkCursorService);
        let provider = CalendarProvider {
            service: service.clone(),
            bulk_cursor: dummy,
        };

        // Construct APPLY_BATCH transaction parcel with an INSERT operation
        let mut data = Parcel::new();
        data.write_i32(0).unwrap(); // AttributionSource size = 0
        ap::string16(&mut data, Some("com.android.calendar")).unwrap(); // authority
        data.write_i32(1).unwrap(); // 1 operation

        // Operation: TYPE_INSERT (1)
        data.write_i32(1).unwrap(); // mType = 1 (INSERT)
        write_uri(&mut data, Some("content://com.android.calendar/events")).unwrap();
        data.write_i32(0).unwrap(); // mMethod = null
        data.write_i32(0).unwrap(); // mArg = null

        // ContentValues: ArrayMap with title
        data.write_i32(1).unwrap(); // valuesSize = 1
        data.write_i32(1).unwrap(); // N = 1
        ap::string16(&mut data, Some("title")).unwrap();
        data.write_i32(0).unwrap(); // VAL_STRING = 0
        ap::string16(&mut data, Some("Batch Created Event")).unwrap();

        data.write_i32(-1).unwrap(); // extrasSize = -1 (null)
        data.write_i32(0).unwrap(); // selection = null
        data.write_i32(-1).unwrap(); // selectionArgs = -1 (null)
        data.write_i32(0).unwrap(); // expectedCount = null
        data.write_i32(0).unwrap(); // yieldAllowed = false
        data.write_i32(0).unwrap(); // exceptionAllowed = false

        data.set_data_position(0);
        let mut reply = Parcel::new();
        let handled = provider.handle("APPLY_BATCH", 20, &mut data, &mut reply).unwrap();
        assert!(handled);

        // Verify that event was inserted into service
        {
            let data = service.data.lock().unwrap();
            let ev = data.events.iter().find(|e| e.title == "Batch Created Event");
            assert!(ev.is_some(), "Batch created event must be present in service");
            assert_eq!(ev.unwrap().id, 3);
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

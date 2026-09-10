//! `alarm`: android.app.IAlarmManager.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct AlarmService {
    pub activity: Arc<super::activity::ActivityService>,
    pub pending_intents: Arc<crate::pending_intent::Registry>,
    active_alarms: Mutex<Vec<(Option<SIBinder>, Arc<AtomicBool>)>>,
}

impl AlarmService {
    pub fn new(
        activity: Arc<super::activity::ActivityService>,
        pending_intents: Arc<crate::pending_intent::Registry>,
    ) -> Self {
        Self {
            activity,
            pending_intents,
            active_alarms: Mutex::new(Vec::new()),
        }
    }
}

fn elapsed_realtime_ms() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts); }
    (ts.tv_sec as i64) * 1000 + (ts.tv_nsec as i64) / 1_000_000
}

fn rtc_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Service for AlarmService {
    const DESCRIPTOR: &'static str = "android.app.IAlarmManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IALARMMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "canScheduleExactAlarms" | "hasScheduleExactAlarm" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "getNextAlarmClock" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?; // null AlarmClockInfo
                Ok(true)
            }
            "getNextWakeFromIdleTime" | "getPrioritizedAlarmDelay" => {
                ap::no_exception(reply)?;
                reply.write_i64(0)?;
                Ok(true)
            }
            "getConfigVersion" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                Ok(true)
            }
            "setTime" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "setTimeZone" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            "set" => {
                let calling_package: Option<String> = data.read()?;
                let alarm_type = data.read_i32()?;
                let trigger_at_time = data.read_i64()?;
                let window_length = data.read_i64()?;
                let interval = data.read_i64()?;
                let flags = data.read_i32()?;
                let has_operation = data.read_i32()?;
                let operation: Option<SIBinder> = if has_operation != 0 { data.read()? } else { None };
                let _listener: Option<SIBinder> = data.read()?;
                let listener_tag: Option<String> = data.read()?;

                log::info!("alarm: set pkg={calling_package:?} type={alarm_type} trigger={trigger_at_time} win={window_length} int={interval} flags={flags} op={has_operation} tag={listener_tag:?}");

                if let Some(ref op) = operation {
                    let mut alarms = self.active_alarms.lock().unwrap();
                    for (existing_op, cancelled) in alarms.iter() {
                        if existing_op.as_ref() == Some(op) {
                            cancelled.store(true, Ordering::SeqCst);
                        }
                    }
                    alarms.retain(|(_, c)| !c.load(Ordering::SeqCst));
                }

                let cancelled = Arc::new(AtomicBool::new(false));
                if operation.is_some() {
                    self.active_alarms.lock().unwrap().push((operation.clone(), cancelled.clone()));
                }

                let target = operation.as_ref().and_then(|op| self.pending_intents.target_for(op));
                let activity = self.activity.clone();

                let now_ms = match alarm_type {
                    0 | 1 => rtc_time_ms(),
                    2 | 3 => elapsed_realtime_ms(),
                    _ => elapsed_realtime_ms(),
                };
                let delay_ms = (trigger_at_time - now_ms).max(0);
                log::info!("alarm: scheduled delay={delay_ms}ms target={target:?}");

                std::thread::spawn(move || {
                    if delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms as u64));
                    }
                    if cancelled.load(Ordering::SeqCst) {
                        log::info!("alarm: cancelled before firing");
                        return;
                    }
                    log::info!("alarm: FIRING target={target:?}");
                    if let Some(t) = target {
                        if t.intent_type == 4 || t.intent_type == 5 || (t.intent_type == 0 && t.class.as_deref().map(|c| c.contains("Service")).unwrap_or(false)) {
                            activity.start_service(&t);
                        } else if t.intent_type == 2 {
                            activity.start_activity(t.package.clone(), t.class.clone(), t.action.clone(), t.data.clone());
                        } else {
                            if t.class.is_some() {
                                activity.start_service(&t);
                            } else {
                                activity.start_activity(t.package.clone(), t.class.clone(), t.action.clone(), t.data.clone());
                            }
                        }
                    }
                });

                ap::no_exception(reply)?;
                Ok(true)
            }
            "remove" => {
                let has_operation = data.read_i32()?;
                let operation: Option<SIBinder> = if has_operation != 0 { data.read()? } else { None };
                let _listener: Option<SIBinder> = data.read()?;

                log::info!("alarm: remove op={has_operation}");
                if let Some(ref op) = operation {
                    let mut alarms = self.active_alarms.lock().unwrap();
                    for (existing_op, cancelled) in alarms.iter() {
                        if existing_op.as_ref() == Some(op) {
                            cancelled.store(true, Ordering::SeqCst);
                        }
                    }
                    alarms.retain(|(_, c)| !c.load(Ordering::SeqCst));
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "removeAll" => {
                let _pkg: Option<String> = data.read()?;
                log::info!("alarm: removeAll for {_pkg:?}");
                let mut alarms = self.active_alarms.lock().unwrap();
                for (_, cancelled) in alarms.iter() {
                    cancelled.store(true, Ordering::SeqCst);
                }
                alarms.clear();
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("has") || name.starts_with("can") {
                    ap::boolean(reply, true)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alarm_clocks() {
        let boottime = elapsed_realtime_ms();
        assert!(boottime > 0, "boottime should be > 0");
        let rtc = rtc_time_ms();
        assert!(rtc > 1_000_000_000_000, "rtc ms should be after year 2001");

        // 10s timer in elapsed realtime
        let trigger = boottime + 10_000;
        let delay = (trigger - elapsed_realtime_ms()).max(0);
        assert!(delay > 9_000 && delay <= 10_000, "delay should be around 10s: {delay}");
    }
}

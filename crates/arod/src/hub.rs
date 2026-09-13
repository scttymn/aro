//! ARO's Binder service manager (the context manager, handle 0).
//!
//! Implements `android.os.IServiceManager` as the Android 16/17 framework
//! speaks it. Every lookup is logged: while the bus is being built, the list
//! of names an app asks for is the specification of what to implement next.
use rsbinder::hub::android_16::android::os::{
    IClientCallback::IClientCallback, IServiceCallback::IServiceCallback, IServiceManager::IServiceManager,
    Service::Service, ServiceDebugInfo::ServiceDebugInfo, ServiceWithMetadata::ServiceWithMetadata,
};
use rsbinder::hub::android_16::android::os::ConnectionInfo;
use rsbinder::{BinderResult, Interface, SIBinder, Strong};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct Hub {
    services: Mutex<BTreeMap<String, (SIBinder, i32)>>,
    reserved: Mutex<BTreeMap<String, i32>>,
    waiters: Mutex<Vec<(String, Strong<dyn IServiceCallback>)>>,
}

impl Hub {
    pub fn reserve(&self, names: &[&str], pid: i32) {
        let mut reserved = self.reserved.lock().unwrap();
        for name in names { reserved.insert((*name).into(), pid); }
    }

    pub fn owned_service(&self, name: &str, pid: i32) -> Option<SIBinder> {
        self.services.lock().unwrap().get(name)
            .filter(|(_, owner)| *owner == pid).map(|(binder, _)| binder.clone())
    }

    pub fn remove_process(&self, pid: i32) {
        // Retire reservations before removal. An addService already queued by
        // the dying process must not put its dead binder back into the registry.
        let mut reserved = self.reserved.lock().unwrap();
        for owner in reserved.values_mut() { if *owner == pid { *owner = 0; } }
        self.services.lock().unwrap().retain(|name, (_, owner)| {
            if *owner == pid { log::warn!("hub: withdrawing {name} after worker {pid} exited"); }
            *owner != pid
        });
    }

    fn lookup(&self, what: &str, name: &str) -> Option<SIBinder> {
        let found = self.services.lock().unwrap().get(name).map(|(b, _)| b.clone());
        let pid = rsbinder::thread_state::get_calling_pid();
        match &found {
            Some(_) => log::info!("hub: {what} {name:?} from pid {pid}: ok"),
            None => log::warn!("hub: {what} {name:?} from pid {pid}: no such service"),
        }
        found
    }

    pub fn register(&self, name: &str, binder: SIBinder) {
        self.register_owned(name, binder, std::process::id() as i32);
    }

    fn register_owned(&self, name: &str, binder: SIBinder, pid: i32) -> bool {
        let reserved = self.reserved.lock().unwrap();
        if reserved.get(name).is_some_and(|owner| *owner != pid) { return false; }
        self.services.lock().unwrap().insert(name.to_string(), (binder.clone(), pid));
        drop(reserved);
        let waiters: Vec<_> = self.waiters.lock().unwrap().iter().filter(|(n, _)| n == name).map(|(_, cb)| cb.clone()).collect();
        for cb in waiters {
            if let Err(e) = cb.onRegistration(name, &binder) {
                log::warn!("hub: onRegistration({name}) failed: {e:?}");
            }
        }
        true
    }
}

/// `BnServiceManager::new_binder` takes the value; share the registry through an Arc.
pub struct HubRef(pub std::sync::Arc<Hub>);

impl Interface for HubRef {}

impl IServiceManager for HubRef {
    fn getService(&self, name: &str) -> BinderResult<Option<SIBinder>> { self.0.getService(name) }
    fn getService2(&self, name: &str) -> BinderResult<Service> { self.0.getService2(name) }
    fn checkService(&self, name: &str) -> BinderResult<Option<SIBinder>> { self.0.checkService(name) }
    fn checkService2(&self, name: &str) -> BinderResult<Service> { self.0.checkService2(name) }
    fn addService(&self, name: &str, service: &SIBinder, a: bool, d: i32) -> BinderResult<()> { self.0.addService(name, service, a, d) }
    fn listServices(&self, d: i32) -> BinderResult<Vec<String>> { self.0.listServices(d) }
    fn registerForNotifications(&self, name: &str, cb: &Strong<dyn IServiceCallback>) -> BinderResult<()> { self.0.registerForNotifications(name, cb) }
    fn unregisterForNotifications(&self, name: &str, cb: &Strong<dyn IServiceCallback>) -> BinderResult<()> { self.0.unregisterForNotifications(name, cb) }
    fn isDeclared(&self, name: &str) -> BinderResult<bool> { self.0.isDeclared(name) }
    fn getDeclaredInstances(&self, i: &str) -> BinderResult<Vec<String>> { self.0.getDeclaredInstances(i) }
    fn updatableViaApex(&self, n: &str) -> BinderResult<Option<String>> { self.0.updatableViaApex(n) }
    fn getUpdatableNames(&self, a: &str) -> BinderResult<Vec<String>> { self.0.getUpdatableNames(a) }
    fn getConnectionInfo(&self, n: &str) -> BinderResult<Option<ConnectionInfo::ConnectionInfo>> { self.0.getConnectionInfo(n) }
    fn registerClientCallback(&self, n: &str, s: &SIBinder, cb: &Strong<dyn IClientCallback>) -> BinderResult<()> { self.0.registerClientCallback(n, s, cb) }
    fn tryUnregisterService(&self, n: &str, s: &SIBinder) -> BinderResult<()> { self.0.tryUnregisterService(n, s) }
    fn getServiceDebugInfo(&self) -> BinderResult<Vec<ServiceDebugInfo>> { self.0.getServiceDebugInfo() }
}

/// HAL instances ARO "declares" (the VINTF manifest, in effect): the gralloc
/// allocator service and the passthrough mapper library `mapper.aro.so`.
pub const DECLARED_HALS: &[&str] = &["android.hardware.graphics.allocator.IAllocator/default", "mapper/aro"];

#[cfg(test)]
mod worker_tests {
    use super::*;
    #[test]
    fn failed_worker_withdraws_only_its_own_binders() {
        let hub = Hub::default();
        hub.register_owned("audio", crate::services::token::new_token("audio"), 42);
        hub.register_owned("network", crate::services::token::new_token("network"), 43);
        hub.remove_process(42);
        assert!(hub.owned_service("audio", 42).is_none());
        assert!(hub.owned_service("network", 43).is_some());
        let info = hub.getServiceDebugInfo().unwrap();
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].debugPid, 43);
    }
    #[test]
    fn old_worker_exit_does_not_remove_a_replacement() {
        let hub = Hub::default();
        hub.register_owned("audio", crate::services::token::new_token("old"), 42);
        hub.register_owned("audio", crate::services::token::new_token("new"), 43);
        hub.remove_process(42);
        assert!(hub.owned_service("audio", 43).is_some());
    }
    #[test]
    fn reservations_reject_other_processes_and_late_dead_worker_registration() {
        let hub = Hub::default();
        hub.reserve(&["audio"], 42);
        assert!(!hub.register_owned("audio", crate::services::token::new_token("wrong"), 43));
        assert!(hub.register_owned("audio", crate::services::token::new_token("right"), 42));
        hub.remove_process(42);
        assert!(!hub.register_owned("audio", crate::services::token::new_token("late"), 42));
        assert!(hub.owned_service("audio", 42).is_none());
    }
}

impl Interface for Hub {}

impl IServiceManager for Hub {
    fn getService(&self, name: &str) -> BinderResult<Option<SIBinder>> {
        Ok(self.lookup("getService", name))
    }
    fn getService2(&self, name: &str) -> BinderResult<Service> {
        Ok(Service::ServiceWithMetadata(ServiceWithMetadata { service: self.lookup("getService2", name), isLazyService: false }))
    }
    fn checkService(&self, name: &str) -> BinderResult<Option<SIBinder>> {
        Ok(self.lookup("checkService", name))
    }
    fn checkService2(&self, name: &str) -> BinderResult<Service> {
        Ok(Service::ServiceWithMetadata(ServiceWithMetadata { service: self.lookup("checkService2", name), isLazyService: false }))
    }
    fn addService(&self, name: &str, service: &SIBinder, _allow_isolated: bool, _dump_priority: i32) -> BinderResult<()> {
        log::info!("hub: addService {name:?} from pid {}", rsbinder::thread_state::get_calling_pid());
        let pid = rsbinder::thread_state::get_calling_pid();
        if !self.register_owned(name, service.clone(), pid) {
            return Err(rsbinder::StatusCode::PermissionDenied.into());
        }
        Ok(())
    }
    fn listServices(&self, _dump_priority: i32) -> BinderResult<Vec<String>> {
        Ok(self.services.lock().unwrap().keys().cloned().collect())
    }
    fn registerForNotifications(&self, name: &str, callback: &Strong<dyn IServiceCallback>) -> BinderResult<()> {
        log::info!("hub: registerForNotifications {name:?}");
        if let Some((b, _)) = self.services.lock().unwrap().get(name).cloned() {
            callback.onRegistration(name, &b)?;
        }
        self.waiters.lock().unwrap().push((name.to_string(), callback.clone()));
        Ok(())
    }
    fn unregisterForNotifications(&self, name: &str, _callback: &Strong<dyn IServiceCallback>) -> BinderResult<()> {
        self.waiters.lock().unwrap().retain(|(n, _)| n != name);
        Ok(())
    }
    fn isDeclared(&self, name: &str) -> BinderResult<bool> {
        let declared = DECLARED_HALS.contains(&name);
        log::info!("hub: isDeclared {name:?} -> {declared}");
        Ok(declared)
    }
    fn getDeclaredInstances(&self, _iface: &str) -> BinderResult<Vec<String>> {
        Ok(vec![])
    }
    fn updatableViaApex(&self, _name: &str) -> BinderResult<Option<String>> {
        Ok(None)
    }
    fn getUpdatableNames(&self, _apex: &str) -> BinderResult<Vec<String>> {
        Ok(vec![])
    }
    fn getConnectionInfo(&self, _name: &str) -> BinderResult<Option<ConnectionInfo::ConnectionInfo>> {
        Ok(None)
    }
    fn registerClientCallback(&self, _name: &str, _service: &SIBinder, _callback: &Strong<dyn IClientCallback>) -> BinderResult<()> {
        Ok(())
    }
    fn tryUnregisterService(&self, name: &str, service: &SIBinder) -> BinderResult<()> {
        let mut services = self.services.lock().unwrap();
        if services.get(name).is_some_and(|(binder, pid)| binder == service && *pid == rsbinder::thread_state::get_calling_pid()) {
            services.remove(name);
        }
        Ok(())
    }
    fn getServiceDebugInfo(&self) -> BinderResult<Vec<ServiceDebugInfo>> {
        Ok(self.services.lock().unwrap().iter().map(|(n, (_, pid))| ServiceDebugInfo { name: n.clone(), debugPid: *pid }).collect())
    }
}

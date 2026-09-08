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
    services: Mutex<BTreeMap<String, SIBinder>>,
    waiters: Mutex<Vec<(String, Strong<dyn IServiceCallback>)>>,
}

impl Hub {
    fn lookup(&self, what: &str, name: &str) -> Option<SIBinder> {
        let found = self.services.lock().unwrap().get(name).cloned();
        let pid = rsbinder::thread_state::get_calling_pid();
        match &found {
            Some(_) => log::info!("hub: {what} {name:?} from pid {pid}: ok"),
            None => log::warn!("hub: {what} {name:?} from pid {pid}: no such service"),
        }
        found
    }

    pub fn register(&self, name: &str, binder: SIBinder) {
        self.services.lock().unwrap().insert(name.to_string(), binder.clone());
        let waiters: Vec<_> = self.waiters.lock().unwrap().iter().filter(|(n, _)| n == name).map(|(_, cb)| cb.clone()).collect();
        for cb in waiters {
            if let Err(e) = cb.onRegistration(name, &binder) {
                log::warn!("hub: onRegistration({name}) failed: {e:?}");
            }
        }
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
        self.register(name, service.clone());
        Ok(())
    }
    fn listServices(&self, _dump_priority: i32) -> BinderResult<Vec<String>> {
        Ok(self.services.lock().unwrap().keys().cloned().collect())
    }
    fn registerForNotifications(&self, name: &str, callback: &Strong<dyn IServiceCallback>) -> BinderResult<()> {
        log::info!("hub: registerForNotifications {name:?}");
        if let Some(b) = self.services.lock().unwrap().get(name).cloned() {
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
        log::info!("hub: isDeclared {name:?}");
        Ok(false)
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
    fn tryUnregisterService(&self, name: &str, _service: &SIBinder) -> BinderResult<()> {
        self.services.lock().unwrap().remove(name);
        Ok(())
    }
    fn getServiceDebugInfo(&self) -> BinderResult<Vec<ServiceDebugInfo>> {
        Ok(self.services.lock().unwrap().keys().map(|n| ServiceDebugInfo { name: n.clone(), debugPid: 0 }).collect())
    }
}

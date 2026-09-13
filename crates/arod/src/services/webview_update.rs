//! `webviewupdate`: android.webkit.IWebViewUpdateService.
//!
//! Tells apps which package provides Android WebView (com.android.webview)
//! Provider discovery shares its availability check with PackageManager.
use super::registry::Registry;
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};
use std::sync::Arc;

pub struct WebViewUpdateService {
    pub registry: Arc<Registry>,
}

// WebViewFactory constants from the installed framework.jar.
const LIBLOAD_SUCCESS: i32 = 0;
const LIBLOAD_FAILED_LISTING_WEBVIEW_PACKAGES: i32 = 4;

impl Service for WebViewUpdateService {
    const DESCRIPTOR: &'static str = "android.webkit.IWebViewUpdateService";
    const TABLE: &'static [(u32, &'static str)] = codes::IWEBVIEWUPDATESERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "waitForAndGetProvider" => {
                log::info!("webviewupdate: waitForAndGetProvider");
                ap::no_exception(reply)?;
                // Non-null WebViewProviderResponse
                reply.write_i32(1)?;
                // writeTypedObject(packageInfo)
                let provider = self.registry.webview_provider();
                let status = if provider.is_some() { LIBLOAD_SUCCESS } else { LIBLOAD_FAILED_LISTING_WEBVIEW_PACKAGES };
                match provider {
                    Some(spec) => {
                        reply.write_i32(1)?;
                        Registry::write_package_info(&spec, reply)?;
                    }
                    None => {
                        log::warn!("webviewupdate: no provider with WebView library metadata");
                        reply.write_i32(0)?;
                    }
                }
                reply.write_i32(status)?;
                Ok(true)
            }
            "getCurrentWebViewPackageName" => {
                ap::no_exception(reply)?;
                let provider = self.registry.webview_provider();
                ap::string16(reply, provider.as_ref().map(|spec| spec.package.as_str()))?;
                Ok(true)
            }
            "getCurrentWebViewPackage" | "getDefaultWebViewPackage" => {
                ap::no_exception(reply)?;
                match self.registry.webview_provider() {
                    Some(spec) => {
                        reply.write_i32(1)?;
                        Registry::write_package_info(&spec, reply)?;
                    }
                    None => {
                        reply.write_i32(0)?;
                    }
                }
                Ok(true)
            }
            "notifyRelroCreationCompleted" => {
                // This method is synchronous: its Proxy calls readException().
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::registry::AppSpec;
    use std::sync::Mutex;

    fn registry(with_library: bool) -> Arc<Registry> {
        let mut manifest = aro_apk::Manifest { package: "com.android.webview".into(), ..Default::default() };
        if with_library {
            manifest.app_meta_data.push(("com.android.webview.WebViewLibrary".into(), "libfixture.so".into()));
        }
        Arc::new(Registry {
            apps: Mutex::new(vec![AppSpec::from_manifest(&manifest, "/system/webview.apk".into(), 10002)]),
            display: (800, 600, 160),
        })
    }

    fn call(registry: Arc<Registry>, name: &str) -> Parcel {
        let service = WebViewUpdateService { registry };
        let mut reply = Parcel::new();
        assert!(service.handle(name, 0, &mut Parcel::new(), &mut reply).unwrap());
        reply.set_data_position(0);
        assert_eq!(reply.read_i32().unwrap(), 0); // readException
        reply
    }

    #[test]
    fn absent_or_invalid_provider_returns_failure_and_null_package() {
        for empty in [false, true] {
            let registry = registry(false);
            if empty { registry.apps.lock().unwrap().clear(); }
            let mut reply = call(registry.clone(), "waitForAndGetProvider");
            assert_eq!(reply.read_i32().unwrap(), 1); // response present
            assert_eq!(reply.read_i32().unwrap(), 0); // PackageInfo absent
            assert_eq!(reply.read_i32().unwrap(), 4); // WebViewFactory failure code
            let mut name = call(registry, "getCurrentWebViewPackageName");
            assert_eq!(name.read::<Option<String>>().unwrap(), None);
        }
    }

    #[test]
    fn provider_preserves_manifest_metadata_and_reports_its_name() {
        let registry = registry(true);
        let spec = registry.webview_provider().unwrap();
        assert_eq!(Registry::application_info(&spec).base.meta_data, spec.meta_data);
        let mut name = call(registry.clone(), "getCurrentWebViewPackageName");
        assert_eq!(name.read::<Option<String>>().unwrap().as_deref(), Some("com.android.webview"));
        let mut reply = call(registry, "waitForAndGetProvider");
        assert_eq!(reply.read_i32().unwrap(), 1);
        assert_eq!(reply.read_i32().unwrap(), 1);
        reply.set_data_position(reply.data_size() - 4);
        assert_eq!(reply.read_i32().unwrap(), 0); // success after PackageInfo
    }

    #[test]
    fn relro_notification_has_synchronous_exception_reply() {
        let reply = call(registry(true), "notifyRelroCreationCompleted");
        assert_eq!(reply.data_position(), reply.data_size());
    }
}

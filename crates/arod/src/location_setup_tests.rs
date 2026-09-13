use super::*;

#[derive(Default)]
struct Fake {
    installed: bool,
    enabled: bool,
    accept: bool,
    fail_install: bool,
    cancel_at_confirm: bool,
    cancel_at_install: bool,
    calls: Vec<&'static str>,
}
impl Backend for Fake {
    fn installed(&mut self) -> bool {
        self.installed
    }
    fn enabled(&mut self) -> bool {
        self.enabled
    }
    fn confirm(
        &mut self,
        _: &str,
        install: bool,
        enable: bool,
        canceled: &AtomicBool,
    ) -> Result<bool> {
        assert_eq!((install, enable), (!self.installed, !self.enabled));
        self.calls.push("confirm");
        if self.cancel_at_confirm {
            canceled.store(true, Ordering::Relaxed);
        }
        Ok(self.accept)
    }
    fn install(&mut self, canceled: &AtomicBool) -> Result<bool> {
        self.calls.push("install");
        if self.cancel_at_install {
            canceled.store(true, Ordering::Relaxed);
        }
        self.installed = !self.fail_install;
        Ok(self.installed)
    }
    fn enable(&mut self) -> Result<bool> {
        self.calls.push("enable");
        self.enabled = true;
        Ok(true)
    }
}
#[test]
fn deny_is_no_install_no_enable_and_no_repeated_prompt() {
    let mut fake = Fake::default();
    let mut decisions = HashMap::new();
    for _ in 0..2 {
        assert!(!prepare(
            &mut fake,
            "org.aro.test",
            &AtomicBool::new(false),
            &mut decisions
        )
        .unwrap());
    }
    assert_eq!(fake.calls, ["confirm"]);
    assert!(!fake.installed && !fake.enabled);
}
#[test]
fn consent_precedes_install_and_enable() {
    let mut fake = Fake {
        accept: true,
        ..Fake::default()
    };
    let mut decisions = HashMap::new();
    for _ in 0..2 {
        assert!(prepare(
            &mut fake,
            "org.aro.test",
            &AtomicBool::new(false),
            &mut decisions
        )
        .unwrap());
    }
    assert_eq!(fake.calls, ["confirm", "install", "enable"]);
    fake.enabled = false;
    assert!(!prepare(
        &mut fake,
        "org.aro.test",
        &AtomicBool::new(false),
        &mut decisions
    )
    .unwrap());
    assert_eq!(
        fake.calls,
        ["confirm", "install", "enable"],
        "a later host disable must not be overridden"
    );
}
#[test]
fn existing_geoclue_does_not_get_reinstalled() {
    let mut fake = Fake {
        installed: true,
        enabled: true,
        accept: true,
        ..Fake::default()
    };
    assert!(prepare(
        &mut fake,
        "org.aro.test",
        &AtomicBool::new(false),
        &mut HashMap::new()
    )
    .unwrap());
    assert_eq!(fake.calls, ["confirm"]);
}
#[test]
fn failed_install_never_enables_location() {
    let mut fake = Fake {
        accept: true,
        fail_install: true,
        ..Fake::default()
    };
    assert!(!prepare(
        &mut fake,
        "org.aro.test",
        &AtomicBool::new(false),
        &mut HashMap::new()
    )
    .unwrap());
    assert_eq!(fake.calls, ["confirm", "install"]);
    assert!(!fake.enabled);
}
#[test]
fn canceled_requests_stop_before_the_next_side_effect() {
    for (at_confirm, at_install) in [(true, false), (false, true)] {
        let mut fake = Fake {
            accept: true,
            cancel_at_confirm: at_confirm,
            cancel_at_install: at_install,
            ..Fake::default()
        };
        assert!(!prepare(
            &mut fake,
            "org.aro.test",
            &AtomicBool::new(false),
            &mut HashMap::new()
        )
        .unwrap());
        assert!(!fake.enabled);
        assert_eq!(fake.calls.contains(&"install"), at_install);
    }
}
#[test]
fn pre_canceled_request_does_not_prompt() {
    let mut fake = Fake::default();
    assert!(!prepare(
        &mut fake,
        "org.aro.test",
        &AtomicBool::new(true),
        &mut HashMap::new()
    )
    .unwrap());
    assert!(fake.calls.is_empty());
}

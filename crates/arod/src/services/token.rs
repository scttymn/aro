//! Plain Binder tokens: objects the app holds references to (activity token,
//! assist token, ...). They answer no transactions; identity is what matters.
use rsbinder::{Parcel, Remotable, Result, SIBinder, StatusCode, TransactionCode};

pub struct Token(pub &'static str);

impl Remotable for Token {
    fn descriptor() -> &'static str {
        "org.aro.Token"
    }
    fn on_transact(&self, code: TransactionCode, _data: &mut Parcel, _reply: &mut Parcel) -> Result<()> {
        log::debug!("token {}: transaction {code} ignored", self.0);
        Err(StatusCode::UnknownTransaction)
    }
    fn on_dump(&self, _w: &mut dyn std::io::Write, _a: &[String]) -> Result<()> {
        Ok(())
    }
}

pub fn new_token(name: &'static str) -> SIBinder {
    let b = rsbinder::native::Binder::new(Token(name));
    rsbinder::Interface::as_binder(&b)
}

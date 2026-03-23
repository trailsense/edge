use atat::{CmeError, CmsError, Error as AtatError, Parser, atat_derive, digest::ParseError};

// Responses
#[derive(atat_derive::AtatResp)]
pub struct NoResponse;

#[derive(atat_derive::AtatResp, Debug)]
pub struct IpResponse {
    pub ip: atat::heapless::String<32>,
}

// Commands
#[derive(atat_derive::AtatCmd)]
#[at_cmd("+NETOPEN", NoResponse, timeout_ms = 5000)]
pub struct NetOpen;

#[derive(atat_derive::AtatCmd)]
#[at_cmd("+IPADDR", IpResponse, timeout_ms = 1000)]
pub struct GetIpAddr;

#[derive(atat_derive::AtatResp, Clone, Debug)]
pub struct HttpActionResult {
    pub method: u8,
    pub status: u16,
    pub len: u16,
}

#[derive(atat_derive::AtatUrc, Clone, Debug)]
pub enum Urc {
    #[at_urc(b"+HTTPACTION")]
    HttpAction(HttpActionResult),
}

pub struct HttpUrcParser;
impl Parser for HttpUrcParser {
    fn parse(buf: &[u8]) -> Result<(&[u8], usize), ParseError> {
        const URC: &[u8] = b"+HTTPACTION";
        let (line, offset) = if let Some(rest) = buf.strip_prefix(b"\r\n") {
            (rest, 2)
        } else {
            (buf, 0)
        };
        if !line.starts_with(URC) {
            if URC.starts_with(line) || b"\r\n+HTTPACTION".starts_with(buf) {
                return Err(ParseError::Incomplete);
            }
            return Err(ParseError::NoMatch);
        }
        if let Some(end) = line.windows(2).position(|w| w == b"\r\n") {
            return Ok((&line[..end], offset + end + 2));
        }
        Err(ParseError::Incomplete)
    }
}

#[derive(Debug)]
pub enum GsmError {
    Atat(AtatError),
    IpTimeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GsmErrorKind {
    Transient,
    Hard,
}

impl GsmError {
    pub fn kind(&self) -> GsmErrorKind {
        match self {
            GsmError::IpTimeout => GsmErrorKind::Transient,
            GsmError::Atat(err) => atat_error_kind(err),
        }
    }
}

impl From<AtatError> for GsmError {
    fn from(e: AtatError) -> Self {
        GsmError::Atat(e)
    }
}

fn atat_error_kind(err: &AtatError) -> GsmErrorKind {
    match err {
        AtatError::Timeout
        | AtatError::Read
        | AtatError::Write
        | AtatError::Aborted
        | AtatError::ConnectionError(_) => GsmErrorKind::Transient,
        AtatError::CmsError(CmsError::NoNetwork | CmsError::NetworkTimeout) => {
            GsmErrorKind::Transient
        }
        AtatError::CmeError(
            CmeError::NoNetwork
            | CmeError::NetworkTimeout
            | CmeError::TemporarilyOutOfService
            | CmeError::MscTemporarilyNotReachable
            | CmeError::Congestion
            | CmeError::NoCellsInArea
            | CmeError::NetworkFailureAttach,
        ) => GsmErrorKind::Transient,
        _ => GsmErrorKind::Hard,
    }
}

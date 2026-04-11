use atat::{
    AtatCmd, CmeError, CmsError, Error as AtatError, InternalError, Parser, atat_derive,
    digest::ParseError,
};

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

pub struct RawAtCmd<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> {
    cmd: &'a str,
}

pub struct RawPayload<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> {
    payload: &'a str,
}

pub struct RawAtReadCmd<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> {
    cmd: &'a str,
}

impl<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> RawAtCmd<'a, MAX_LEN, TIMEOUT_MS> {
    pub fn new(cmd: &'a str) -> Self {
        Self { cmd }
    }
}

impl<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> RawPayload<'a, MAX_LEN, TIMEOUT_MS> {
    pub fn new(payload: &'a str) -> Self {
        Self { payload }
    }
}

impl<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> RawAtReadCmd<'a, MAX_LEN, TIMEOUT_MS> {
    pub fn new(cmd: &'a str) -> Self {
        Self { cmd }
    }
}

impl<const MAX_LEN: usize, const TIMEOUT_MS: u32> AtatCmd for RawAtCmd<'_, MAX_LEN, TIMEOUT_MS> {
    type Response = NoResponse;
    const MAX_LEN: usize = MAX_LEN;
    const MAX_TIMEOUT_MS: u32 = TIMEOUT_MS;

    fn write(&self, buf: &mut [u8]) -> usize {
        let cmd = self.cmd.as_bytes();
        let len = cmd.len();
        assert!(
            len + 2 <= buf.len(),
            "RawAtCmd exceeds TX buffer: needed={}, available={}",
            len + 2,
            buf.len()
        );
        buf[..len].copy_from_slice(cmd);
        buf[len] = b'\r';
        buf[len + 1] = b'\n';
        len + 2
    }

    fn parse(&self, resp: Result<&[u8], InternalError>) -> Result<Self::Response, AtatError> {
        let _ = resp.map_err(AtatError::from)?;
        Ok(NoResponse)
    }
}

impl<const MAX_LEN: usize, const TIMEOUT_MS: u32> AtatCmd for RawPayload<'_, MAX_LEN, TIMEOUT_MS> {
    type Response = NoResponse;
    const MAX_LEN: usize = MAX_LEN;
    const MAX_TIMEOUT_MS: u32 = TIMEOUT_MS;

    fn write(&self, buf: &mut [u8]) -> usize {
        let payload = self.payload.as_bytes();
        let len = payload.len();
        assert!(
            len <= buf.len(),
            "RawPayload exceeds TX buffer: needed={}, available={}",
            len,
            buf.len()
        );
        buf[..len].copy_from_slice(payload);
        len
    }

    fn parse(&self, resp: Result<&[u8], InternalError>) -> Result<Self::Response, AtatError> {
        let _ = resp.map_err(AtatError::from)?;
        Ok(NoResponse)
    }
}

impl<const MAX_LEN: usize, const TIMEOUT_MS: u32> AtatCmd
    for RawAtReadCmd<'_, MAX_LEN, TIMEOUT_MS>
{
    type Response = atat::heapless::String<512>;
    const MAX_LEN: usize = MAX_LEN;
    const MAX_TIMEOUT_MS: u32 = TIMEOUT_MS;

    fn write(&self, buf: &mut [u8]) -> usize {
        let cmd = self.cmd.as_bytes();
        let len = cmd.len();
        assert!(
            len + 2 <= buf.len(),
            "RawAtReadCmd exceeds TX buffer: needed={}, available={}",
            len + 2,
            buf.len()
        );
        buf[..len].copy_from_slice(cmd);
        buf[len] = b'\r';
        buf[len + 1] = b'\n';
        len + 2
    }

    fn parse(&self, resp: Result<&[u8], InternalError>) -> Result<Self::Response, AtatError> {
        let raw = resp.map_err(AtatError::from)?;
        let utf8 = core::str::from_utf8(raw).map_err(|_| AtatError::Parse)?;
        atat::heapless::String::<512>::try_from(utf8).map_err(|_| AtatError::Parse)
    }
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
    BufferTooSmall { needed: usize, available: usize },
    CommandBuildFailed,
    HttpActionTimeout,
    HttpStatus(u16),
    GsmInitError,
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
            GsmError::BufferTooSmall { .. } => GsmErrorKind::Hard,
            GsmError::CommandBuildFailed => GsmErrorKind::Hard,
            GsmError::HttpActionTimeout => GsmErrorKind::Transient,
            GsmError::HttpStatus(status) => {
                if *status >= 500 || *status == 408 || *status == 429 {
                    GsmErrorKind::Transient
                } else {
                    GsmErrorKind::Hard
                }
            }
            GsmError::Atat(err) => atat_error_kind(err),
            GsmError::GsmInitError => GsmErrorKind::Hard,
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
        | AtatError::Error
        | AtatError::InvalidResponse
        | AtatError::Parse
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

//! Minimal SOCKS5 server handshake (RFC 1928): no authentication, CONNECT only.

use std::net::{Ipv4Addr, Ipv6Addr};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SocksError {
    #[error("not a SOCKS5 client")]
    BadVersion,
    #[error("client offered no acceptable authentication method")]
    NoAuthMethod,
    #[error("only CONNECT is supported")]
    UnsupportedCommand,
    #[error("unsupported address type {0}")]
    BadAddressType(u8),
    #[error("i/o: {0}")]
    Io(String),
}

impl From<std::io::Error> for SocksError {
    fn from(e: std::io::Error) -> Self {
        SocksError::Io(e.to_string())
    }
}

/// Reply codes (RFC 1928 section 6).
pub const REP_SUCCEEDED: u8 = 0;
pub const REP_GENERAL_FAILURE: u8 = 1;
pub const REP_COMMAND_NOT_SUPPORTED: u8 = 7;
pub const REP_ADDRESS_NOT_SUPPORTED: u8 = 8;

/// Perform the greeting and read the CONNECT request; returns `(host, port)`.
/// Call [`reply`] afterwards with the outcome.
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
) -> Result<(String, u16), SocksError> {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(SocksError::BadVersion);
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        s.write_all(&[5, 0xFF]).await?;
        return Err(SocksError::NoAuthMethod);
    }
    s.write_all(&[5, 0]).await?;

    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(SocksError::BadVersion);
    }
    if req[1] != 1 {
        reply(s, REP_COMMAND_NOT_SUPPORTED).await?;
        return Err(SocksError::UnsupportedCommand);
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            s.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await?;
            Ipv6Addr::from(a).to_string()
        }
        other => {
            reply(s, REP_ADDRESS_NOT_SUPPORTED).await?;
            return Err(SocksError::BadAddressType(other));
        }
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await?;
    Ok((host, u16::from_be_bytes(port)))
}

/// Send the final reply (bound address is reported as 0.0.0.0:0).
pub async fn reply<S: AsyncWrite + Unpin>(s: &mut S, code: u8) -> Result<(), SocksError> {
    s.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    async fn run(client_bytes: &[u8]) -> (Result<(String, u16), SocksError>, Vec<u8>) {
        let (mut client, mut server) = duplex(1024);
        client.write_all(client_bytes).await.unwrap();
        client.shutdown().await.unwrap(); // EOF for truncated inputs
        let r = handshake(&mut server).await;
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        (r, out)
    }

    #[tokio::test]
    async fn connect_by_domain_ipv4_and_ipv6() {
        let mut dom = vec![5, 1, 0, 5, 1, 0, 3, 11];
        dom.extend(b"example.com");
        dom.extend(443u16.to_be_bytes());
        let (r, out) = run(&dom).await;
        assert_eq!(r.unwrap(), ("example.com".to_string(), 443));
        assert_eq!(
            out,
            [5, 0],
            "method selection reply only; request reply comes later"
        );

        let (r, _) = run(&[5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 7, 0x1f, 0x90]).await;
        assert_eq!(r.unwrap(), ("10.0.0.7".to_string(), 8080));

        let mut v6 = vec![5, 1, 0, 5, 1, 0, 4];
        v6.extend(Ipv6Addr::LOCALHOST.octets());
        v6.extend(22u16.to_be_bytes());
        assert_eq!(run(&v6).await.0.unwrap(), ("::1".to_string(), 22));
    }

    #[tokio::test]
    async fn rejects_bad_clients() {
        assert_eq!(run(&[4, 1, 0]).await.0, Err(SocksError::BadVersion));
        let (r, out) = run(&[5, 1, 2]).await;
        assert_eq!(r, Err(SocksError::NoAuthMethod));
        assert_eq!(out, [5, 0xFF]);
        // BIND (cmd 2) is unsupported
        let (r, out) = run(&[5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80]).await;
        assert_eq!(r, Err(SocksError::UnsupportedCommand));
        assert_eq!(out[2..4], [5, REP_COMMAND_NOT_SUPPORTED]);
        // unknown address type
        let (r, _) = run(&[5, 1, 0, 5, 1, 0, 9]).await;
        assert_eq!(r, Err(SocksError::BadAddressType(9)));
        // truncated request
        assert!(matches!(
            run(&[5, 1, 0, 5, 1]).await.0,
            Err(SocksError::Io(_))
        ));
    }

    #[tokio::test]
    async fn reply_format() {
        let (mut a, mut b) = duplex(64);
        reply(&mut a, REP_SUCCEEDED).await.unwrap();
        let mut buf = [0u8; 10];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    }
}

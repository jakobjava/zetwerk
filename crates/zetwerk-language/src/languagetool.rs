//! Grammar by a LanguageTool server the user runs.
//!
//! LanguageTool is the one serious checker of grammar for German and
//! Dutch, and it is a Java server. zetwerk talks to one only at the
//! address the settings name, and only if that address is this machine,
//! unless the settings say in so many words that another may be used:
//! what is checked is the user's text, and it is not to leave the
//! machine by an oversight. The public service is never contacted.

use std::net::IpAddr;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use crate::Server;

/// How long the server may take to accept a connection, and to answer.
const CONNECT: Duration = Duration::from_secs(2);
const ANSWER: Duration = Duration::from_secs(30);

/// Something LanguageTool found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The bytes of the text sent that it is about.
    pub range: Range<usize>,
    pub message: String,
    /// What could be written instead, the likeliest first.
    pub replacements: Vec<String>,
}

/// A LanguageTool server.
pub struct LanguageTool {
    /// Where its checks are asked for: `…/v2/check`.
    endpoint: String,
    agent: ureq::Agent,
}

/// The host a URL names, and whether it is asked over TLS.
fn host_of(url: &str) -> Result<(String, bool), String> {
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!("{url} is no http:// or https:// address"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(format!(
            "{url} names a user; an address without one, please"
        ));
    }
    let host = match authority.strip_prefix('[') {
        // An IPv6 address stands in brackets, its port behind them.
        Some(bracketed) => bracketed.split(']').next().unwrap_or(""),
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    };
    if host.is_empty() {
        return Err(format!("{url} names no host"));
    }
    Ok((host.to_lowercase(), secure))
}

/// Whether `host` is this machine: by name, or by an address that
/// never leaves it.
fn is_local(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The language as LanguageTool names it, for the dictionary `wanted`
/// (see [`crate::wanted`]): `en-GB`, `de-DE`, `nl`. English and German
/// are checked as they are written in one country; Dutch as Dutch,
/// unless it is Belgium's.
#[must_use]
pub fn code(wanted: &str) -> String {
    let (language, region) = wanted.split_once('_').unwrap_or((wanted, ""));
    match (language, region) {
        ("en" | "de" | "pt", region) if !region.is_empty() => format!("{language}-{region}"),
        ("nl", "BE") => "nl-BE".to_owned(),
        _ => language.to_owned(),
    }
}

/// Whether a rule of LanguageTool's is its spelling checker, which is
/// not asked: spelling is the dictionary's and the user's own words'.
fn is_speller(rule: &str) -> bool {
    rule.starts_with("MORFOLOGIK_RULE")
        || rule.starts_with("HUNSPELL")
        || rule.ends_with("SPELLER_RULE")
}

/// Where the UTF-16 unit `unit` of `text` is in its bytes: LanguageTool
/// counts as Java does.
fn byte_of(text: &str, unit: usize) -> usize {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        if units >= unit {
            return at;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// What the server's answer to a check of `text` says was found.
fn matches(text: &str, answer: &str) -> Result<Vec<Match>, String> {
    let answer: serde_json::Value = serde_json::from_str(answer)
        .map_err(|err| format!("its answer is not understood: {err}"))?;
    let found = answer
        .get("matches")
        .and_then(serde_json::Value::as_array)
        .ok_or("its answer lists no matches: is it a LanguageTool server?")?;
    let number = |value: &serde_json::Value, name: &str| {
        let number = value.get(name).and_then(serde_json::Value::as_u64);
        number.and_then(|number| usize::try_from(number).ok())
    };
    Ok(found
        .iter()
        .filter_map(|found| {
            let rule = found.get("rule").and_then(|rule| rule.get("id"));
            if rule
                .and_then(serde_json::Value::as_str)
                .is_some_and(is_speller)
            {
                return None;
            }
            let (offset, length) = (number(found, "offset")?, number(found, "length")?);
            let replacements = found
                .get("replacements")
                .and_then(serde_json::Value::as_array);
            let replacements = replacements
                .into_iter()
                .flatten()
                .filter_map(|replacement| {
                    let value = replacement.get("value").and_then(serde_json::Value::as_str);
                    value.map(str::to_owned)
                });
            Some(Match {
                range: byte_of(text, offset)..byte_of(text, offset + length),
                message: found.get("message")?.as_str()?.to_owned(),
                replacements: replacements.collect(),
            })
        })
        .collect())
}

impl LanguageTool {
    /// The server the settings name. The error says why it is not used:
    /// its address is no address, or is not this machine and the
    /// settings do not allow that.
    pub fn new(server: &Server) -> Result<Self, String> {
        let url = server.url.trim().trim_end_matches('/');
        let (host, secure) = host_of(url)?;
        if !is_local(&host) && !server.remote {
            return Err(format!(
                "{host} is not this machine: the text is not sent there \
                 (allow_remote = true under [language] allows it)"
            ));
        }
        let endpoint = if url.ends_with("/v2/check") {
            url.to_owned()
        } else if url.ends_with("/v2") {
            format!("{url}/check")
        } else {
            format!("{url}/v2/check")
        };
        // The answer is taken from where the text was sent, not from
        // wherever that server points to.
        let mut agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT)
            .timeout_read(ANSWER)
            .timeout_write(ANSWER)
            .redirects(0);
        if secure {
            let connector = native_tls::TlsConnector::new()
                .map_err(|err| format!("no TLS for {url}: {err}"))?;
            agent = agent.tls_connector(Arc::new(connector));
        }
        Ok(Self {
            endpoint,
            agent: agent.build(),
        })
    }

    /// Where checks are asked for.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// What the server finds in `text`, which is in `language` as
    /// LanguageTool names languages (see [`code`]). The error says why
    /// there is no answer.
    pub fn check(&self, text: &str, language: &str) -> Result<Vec<Match>, String> {
        let form = [("text", text), ("language", language)];
        let answer = self.agent.post(&self.endpoint).send_form(&form);
        let answer = answer.map_err(|err| match err {
            ureq::Error::Status(status, answer) => {
                let said = answer.into_string().unwrap_or_default();
                let said = said.lines().next().unwrap_or("").trim().to_owned();
                format!("it answers {status} {said}")
            }
            ureq::Error::Transport(err) => err.to_string(),
        })?;
        let answer = answer.into_string().map_err(|err| err.to_string())?;
        matches(text, &answer)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
pub(crate) mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};

    use super::*;

    /// A server on this machine that answers every request with
    /// `answer`, and hands on the body of each request it got.
    pub(crate) fn serve(status: &'static str, answer: String) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (asked, requests) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = stream.write_all(reply.as_bytes());
                if asked.send(format!("{head}\n{body}")).is_err() {
                    break;
                }
            }
        });
        (address, requests)
    }

    fn server(url: &str, remote: bool) -> Server {
        Server {
            url: url.to_owned(),
            remote,
        }
    }

    #[test]
    fn only_this_machine_is_asked_unless_the_settings_allow_more() {
        for local in [
            "http://localhost:8081",
            "http://LOCALHOST:8081/v2",
            "http://127.0.0.1:8010/",
            "http://127.8.9.1",
            "http://[::1]:8081",
            "https://localhost:8443/v2/check",
        ] {
            let tool = LanguageTool::new(&server(local, false));
            assert!(
                tool.is_ok_and(|tool| tool.endpoint().ends_with("/v2/check")),
                "{local}"
            );
        }
        for elsewhere in [
            "https://api.languagetool.org",
            "https://api.languagetoolplus.com/v2",
            "http://192.168.1.20:8081",
            "http://localhost.example.org",
            "http://localhost@example.org",
            "http://example.org/localhost",
            "http://[2001:db8::1]:8081",
            "http://0.0.0.0:8081",
        ] {
            let refused = LanguageTool::new(&server(elsewhere, false));
            assert!(refused.is_err(), "{elsewhere}");
        }
        assert!(LanguageTool::new(&server("https://lt.example.org", true)).is_ok());
        assert!(LanguageTool::new(&server("ftp://localhost", true)).is_err());
        assert!(LanguageTool::new(&server("localhost:8081", false)).is_err());
        let why = LanguageTool::new(&server("http://example.org", false))
            .err()
            .unwrap();
        assert!(
            why.contains("example.org") && why.contains("allow_remote"),
            "{why}"
        );
    }

    #[test]
    fn the_three_languages_are_named_as_languagetool_names_them() {
        assert_eq!(code(&crate::wanted("en")), "en-GB");
        assert_eq!(code(&crate::wanted("de")), "de-DE");
        assert_eq!(code(&crate::wanted("nl")), "nl");
        assert_eq!(code(&crate::wanted("en-US")), "en-US");
        assert_eq!(code(&crate::wanted("de-AT")), "de-AT");
        assert_eq!(code(&crate::wanted("nl-BE")), "nl-BE");
        assert_eq!(code(&crate::wanted("fr")), "fr");
    }

    #[test]
    fn a_check_sends_the_text_and_reads_what_was_found() {
        let answer = serde_json::json!({
            "software": { "name": "LanguageTool" },
            "matches": [
                {
                    "message": "Möglicherweise fehlt ein Komma.",
                    "offset": 13, "length": 4,
                    "replacements": [{ "value": ", dass" }, { "value": "das" }],
                    "rule": { "id": "KOMMA_VOR_DASS", "issueType": "typographical" },
                },
                {
                    "message": "Möglicher Tippfehler gefunden.",
                    "offset": 0, "length": 2,
                    "replacements": [],
                    "rule": { "id": "GERMAN_SPELLER_RULE", "issueType": "misspelling" },
                },
            ],
        });
        let (address, requests) = serve("200 OK", answer.to_string());
        let tool = LanguageTool::new(&server(&address, false)).unwrap();
        // 😀 is two units to LanguageTool and four bytes here.
        let text = "Ih glaube 😀 dass es geht.";
        let found = tool.check(text, "de-DE").unwrap();
        assert_eq!(found.len(), 1, "its speller is not listened to");
        assert_eq!(&text[found[0].range.clone()], "dass");
        assert_eq!(found[0].message, "Möglicherweise fehlt ein Komma.");
        assert_eq!(found[0].replacements, [", dass", "das"]);
        let asked = requests.recv().unwrap();
        assert!(asked.starts_with("POST /v2/check HTTP/1.1"), "{asked}");
        assert!(asked.contains("language=de-DE"), "{asked}");
        assert!(
            asked.contains("text=Ih+glaube+%F0%9F%98%80+dass+es+geht."),
            "{asked}"
        );
    }

    #[test]
    fn a_server_that_is_not_there_or_not_one_says_so() {
        let unused = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let tool = LanguageTool::new(&server(&unused, false)).unwrap();
        assert!(tool.check("text", "en-GB").is_err());

        let (address, _requests) = serve("200 OK", "{\"hello\": 1}".to_owned());
        let tool = LanguageTool::new(&server(&address, false)).unwrap();
        let why = tool.check("text", "en-GB").unwrap_err();
        assert!(why.contains("LanguageTool"), "{why}");

        let (address, _requests) = serve("400 Bad Request", "Error: no such language".to_owned());
        let tool = LanguageTool::new(&server(&address, false)).unwrap();
        let why = tool.check("text", "xx").unwrap_err();
        assert!(
            why.contains("400") && why.contains("no such language"),
            "{why}"
        );
    }
}

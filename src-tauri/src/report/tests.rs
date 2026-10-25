//! Error report tests (§9.6): redaction with hostile input, log tail bounds in the report, URL
//! limits. Values are placeholders (`example.com`, documentation IP ranges); nothing secret.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;

use super::*;
use crate::ipc::dto::{BookmarksDto, RecentEntry, RecentTarget, SourceLocation, SshConfigHost};

const HOME_WIN: &str = r"C:\Users\Alice";
const HOME_UNIX: &str = "/home/alice";

fn known() -> KnownValues {
    let mut k = KnownValues::default();
    k.add_home(HOME_WIN);
    k.add_home(HOME_UNIX);
    k.add_user("alice");
    k.add_user("deploy");
    k.add_host("web-01.example.com");
    k.add_host("example.com");
    k.add_host("db");
    k.add_host("203.0.113.7");
    k.add_path("/srv/app");
    k
}

fn redact(text: &str) -> String {
    Redactor::new(&known()).redact(text)
}

fn env() -> ReportEnvironment {
    ReportEnvironment {
        app_version: "0.2.0".into(),
        os: "Windows 11 24H2 (build 26100.2033)".into(),
        arch: "x86_64".into(),
        install: Some("Windows installer (NSIS .exe)".into()),
        ui_language: Some("uk".into()),
    }
}

// ---- known values --------------------------------------------------------------------------

#[test]
fn known_hosts_users_and_paths_are_replaced() {
    assert_eq!(
        redact("connect to web-01.example.com as deploy failed"),
        "connect to <host> as <user> failed"
    );
    // Case-insensitive; the longest value wins over the shorter one inside it.
    assert_eq!(
        redact("WEB-01.Example.COM and example.com"),
        "<host> and <host>"
    );
    // Subdomains keep their prefix only.
    assert_eq!(redact("www.example.com"), "www.<host>");
    assert_eq!(redact("cd /srv/app/releases"), "cd <path>");
}

#[test]
fn known_values_match_whole_tokens_only() {
    // `db` inside other words, `alice` inside `alice.smith` or `malice` stay.
    assert_eq!(
        redact("dbx mydb db-2 db_1 db."),
        "dbx mydb db-2 db_1 <host>."
    );
    assert_eq!(
        redact("alice.smith malice alice"),
        "alice.smith malice <user>"
    );
    // One-character values are ignored entirely.
    let mut k = KnownValues::default();
    k.add_user("a");
    k.add_host("x");
    assert_eq!(Redactor::new(&k).redact("a x a"), "a x a");
}

#[test]
fn home_directory_becomes_tilde_in_every_spelling() {
    assert_eq!(redact(r"C:\Users\Alice\.ssh\config"), r"~\.ssh\config");
    assert_eq!(redact(r"c:\users\alice\x"), r"~\x");
    assert_eq!(redact("C:/Users/Alice/x"), "~/x");
    // Debug and JSON double the backslashes.
    assert_eq!(
        redact(r#"path: "C:\\Users\\Alice\\.ssh\\id""#),
        r#"path: "~\\.ssh\\id""#
    );
    assert_eq!(redact("/home/alice/.ssh/known_hosts"), "~/.ssh/known_hosts");
    // Another home is a remote path.
    assert_eq!(redact("/home/bob/.ssh/known_hosts"), "<path>");
    // Not a prefix of a longer name.
    assert_eq!(redact("/home/alicex/y"), "<path>");
}

#[test]
fn values_with_special_characters_are_literal() {
    let mut k = KnownValues::default();
    k.add_host("h.*+?(x)[y]");
    k.add_user("50%off$1\\d");
    k.add_path("/tmp/a b%2F");
    let r = Redactor::new(&k);
    assert_eq!(r.redact("on h.*+?(x)[y] now"), "on <host> now");
    assert_eq!(
        r.redact("hXXXX(x)[y]"),
        "hXXXX(x)[y]",
        "no pattern semantics"
    );
    assert_eq!(r.redact("user 50%off$1\\d!"), "user <user>!");
    assert_eq!(r.redact("dir /tmp/a b%2F"), "dir <path>");
}

#[test]
fn placeholders_are_not_redacted_again() {
    let mut k = KnownValues::default();
    k.add_host("host");
    k.add_user("user");
    k.add_path("path");
    let r = Redactor::new(&k);
    assert_eq!(r.redact("host user path"), "<host> <user> <path>");
    // Single pass: the inserted `<host>` is never matched by `host` again.
    assert_eq!(r.redact("user@host"), "<user>@<host>");
}

// ---- patterns ------------------------------------------------------------------------------

#[test]
fn ip_addresses_are_replaced() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("dial 198.51.100.23:22 failed"),
        "dial <ip>:22 failed"
    );
    assert_eq!(
        r.redact("from 2001:db8::1 and [2001:db8:0:0:1::2]:2222"),
        "from <ip> and [<ip>]:2222"
    );
    assert_eq!(r.redact("fe80::1%eth0"), "<ip>%eth0");
    assert_eq!(r.redact("ip=192.0.2.1."), "ip=<ip>.");
    // Loopback and unspecified stay readable.
    assert_eq!(r.redact("127.0.0.1 ::1 0.0.0.0"), "127.0.0.1 ::1 0.0.0.0");
    // Not addresses: times, versions, Rust paths, MACs, words.
    for keep in [
        "12:34:56",
        "2026-10-23T10:15:02.123Z",
        "russh 0.64.1",
        "shellfin_lib::commands::app",
        "Error::Io",
        "aa:bb:cc:dd:ee:ff",
        "1.2.3.4.5",
        "v1.2.3.4",
        "999.1.1.1",
        "deadbeef cafe",
    ] {
        assert_eq!(r.redact(keep), keep, "{keep}");
    }
}

#[test]
fn user_at_host_and_emails_are_replaced() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("ssh bob@build.example.net -p 22"),
        "ssh <user>@<host> -p 22"
    );
    assert_eq!(r.redact("bob@198.51.100.4"), "<user>@<ip>");
    assert_eq!(
        r.redact("mail ops.team@example.org."),
        "mail <user>@<host>."
    );
    // WebKit stack frames and package specs stay.
    assert_eq!(
        r.redact("render@tauri://localhost/assets/index.js:1:2"),
        "render@tauri://localhost/assets/index.js:1:2"
    );
    assert_eq!(r.redact("react@18.3.1"), "react@18.3.1");
    assert_eq!(r.redact("@ alone and trailing@"), "@ alone and trailing@");
}

#[test]
fn key_values_are_redacted_by_key() {
    let r = Redactor::new(&known());
    // Debug of error codes (as in `error = ?e.kind` log lines).
    assert_eq!(
        r.redact(r#"RemoteNotFound { path: "/var/www/site/index.php" }"#),
        r#"RemoteNotFound { path: "<path>" }"#
    );
    assert_eq!(
        r.redact(r#"DnsFailed { host: "intra.corp.example" }"#),
        r#"DnsFailed { host: "<host>" }"#
    );
    assert_eq!(
        r.redact(r#"Io { op: ReadKey, path: Some("/opt/k") }"#),
        r#"Io { op: ReadKey, path: Some("<path>") }"#
    );
    assert_eq!(
        r.redact(r#"HostKeyChanged { fingerprint: "SHA256:abcd", known_hosts_file: "/etc/ssh/ssh_known_hosts" }"#),
        r#"HostKeyChanged { fingerprint: "<redacted>", known_hosts_file: "<path>" }"#
    );
    // Escaped quotes inside a Debug string do not end the value early.
    assert_eq!(
        r.redact(r#"path: "/a/\"b\"/c" next"#),
        r#"path: "<path>" next"#
    );
    // tracing fields.
    assert_eq!(
        r.redact("ignoring known_hosts file file=/etc/ssh/x reason=denied"),
        "ignoring known_hosts file file=<path> reason=denied"
    );
    assert_eq!(
        r.redact("host=intra.example user=bob"),
        "host=<host> user=<user>"
    );
    // JSON params.
    assert_eq!(
        r.redact(
            r#"{"host":"192.0.2.9","user":"bob","remotePath":"/data/x","file":"bookmarks.json"}"#
        ),
        r#"{"host":"<ip>","user":"<user>","remotePath":"<path>","file":"bookmarks.json"}"#
    );
    // A path under the home keeps the `~` form.
    assert_eq!(r.redact(r#"{"path":"/home/alice/k"}"#), r#"{"path":"~/k"}"#);
    // Keys only as whole identifiers; bare names and relative paths stay.
    assert_eq!(
        r.redact("xhost=abc somepath: \"q\""),
        "xhost=abc somepath: \"q\""
    );
    assert_eq!(
        r.redact(r#"panic file="src\\commands\\mod.rs" line=3"#),
        r#"panic file="src\\commands\\mod.rs" line=3"#
    );
    assert_eq!(
        r.redact("file=src/commands/app.rs"),
        "file=src/commands/app.rs"
    );
    // Windows paths outside the home are redacted too.
    assert_eq!(r.redact(r#"path: "D:\\work\\k.pem""#), r#"path: "<path>""#);
    assert_eq!(r.redact(r#"path: "~/.ssh/id""#), r#"path: "~/.ssh/id""#);
}

#[test]
fn absolute_remote_paths_in_free_text_are_replaced() {
    let r = Redactor::new(&known());
    assert_eq!(
        r.redact("cannot open /var/lib/secret-project/db.sqlite."),
        "cannot open <path>."
    );
    assert_eq!(r.redact("(/etc/passwd)"), "(<path>)");
    assert_eq!(r.redact("“/srv/data/x” missing"), "“<path>” missing");
    // URLs, relative paths, `~/` and toolchain paths stay.
    assert_eq!(
        r.redact("see https://example.org/a/b"),
        "see https://example.org/a/b"
    );
    assert_eq!(
        r.redact("in ~/notes and src/x.rs"),
        "in ~/notes and src/x.rs"
    );
    assert_eq!(
        r.redact("at /rustc/abc123/library/core/src/option.rs:1"),
        "at /rustc/abc123/library/core/src/option.rs:1"
    );
    assert_eq!(r.redact("a/b and ok/"), "a/b and ok/");
}

#[test]
fn very_long_input_is_bounded_and_fast() {
    let r = Redactor::new(&known());
    let long = format!(
        "{} web-01.example.com {}",
        "x".repeat(200_000),
        "y".repeat(10)
    );
    let start = std::time::Instant::now();
    let out = r.redact(&long);
    assert!(out.contains("<host>"));
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    // clean_text bounds the raw input, then lines and characters.
    let text = clean_text(&r, &long, 10, 500);
    assert!(text.chars().count() <= 500, "{}", text.chars().count());
    let many = "line db\n".repeat(1000);
    let text = clean_text(&r, &many, 5, 10_000);
    assert_eq!(text.lines().count(), 6, "5 lines and the cut marker");
    assert!(text.ends_with('…'));
    assert!(!text.contains(" db"));
}

#[test]
fn bound_raw_cuts_at_whitespace() {
    let s = format!(
        "{} secret-host.example.com",
        "a ".repeat(MAX_RAW_CHARS / 2 - 4)
    );
    let b = bound_raw(&s);
    assert!(b.chars().count() <= MAX_RAW_CHARS);
    assert!(
        !b.contains("secret"),
        "a value cut by the bound is dropped whole"
    );
    assert_eq!(bound_raw("short"), "short");
}

#[test]
fn windows_and_unc_paths_in_free_text_are_replaced() {
    let r = Redactor::new(&known());
    assert_eq!(
        r.redact(r"open C:\Users\Carol\Documents\plan.txt failed"),
        "open <path> failed"
    );
    assert_eq!(r.redact(r"D:\secret\keys\id.pem"), "<path>");
    assert_eq!(r.redact("(E:/backup/db.sql)"), "(<path>)");
    assert_eq!(r.redact(r"from \\fileserver\share\x.txt."), "from <path>.");
    // Debug-escaped forms.
    assert_eq!(r.redact(r"at \\\\fileserver\\share\\x"), "at <path>");
    assert_eq!(r.redact(r#"err "D:\\secret\\k""#), r#"err "<path>""#);
    // The own home stays `~`; drive letters inside words and relative paths stay.
    assert_eq!(r.redact(r"C:\Users\Alice\.ssh\config"), r"~\.ssh\config");
    assert_eq!(
        r.redact(r"xC:\a ab:\c src\main.rs"),
        r"xC:\a ab:\c src\main.rs"
    );
    assert_eq!(r.redact(r"a\\b"), r"a\\b");
}

#[test]
fn scp_style_host_paths_are_replaced() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("copy prod.internal:/srv/secret/app.env now"),
        "copy prod.internal:<path> now"
    );
    assert_eq!(r.redact("host:/etc/x"), "host:<path>");
    // Not URLs and not a drive; but the path of a `file:` URL is a local path.
    assert_eq!(
        r.redact("https://example.org/a/b"),
        "https://example.org/a/b"
    );
    assert_eq!(r.redact("file:///etc/x"), "file://<path>");
}

#[test]
fn file_urls_and_verbatim_windows_paths_are_replaced() {
    let r = Redactor::new(&known());
    assert_eq!(
        r.redact("open file:///C:/Users/carol/x.txt failed"),
        "open file://<path> failed"
    );
    assert_eq!(r.redact("FILE:///home/eve/x"), "FILE://<path>");
    // The own home keeps `~`, as without the scheme.
    assert_eq!(r.redact("file:///home/alice/x"), "file://~/x");
    // UNC forms: everything after `file://`.
    assert_eq!(
        r.redact("see file://server/share/x.txt."),
        "see file://<path>."
    );
    assert_eq!(r.redact("file:////server/share/x"), "file://<path>");
    // `xfile:///a` is no `file:` URL; a bare scheme stays.
    assert_eq!(r.redact("xfile:///a"), "xfile:///a");
    assert_eq!(r.redact("file:// and file:///"), "file:// and file:///");
    assert_eq!(r.redact(r"\\?\D:\secret\x.txt"), "<path>");
    assert_eq!(r.redact(r"at \\?\UNC\server\share\x."), "at <path>.");
    // A verbatim path is redacted whole, also under the home (no `\\?\~` remnant).
    assert_eq!(r.redact(r"\\?\C:\Users\Alice\x"), "<path>");
    // Debug-escaped.
    assert_eq!(r.redact(r#"path "\\\\?\\D:\\k""#), r#"path "<path>""#);
    // Not a verbatim prefix.
    assert_eq!(r.redact(r"\\? and \\?x"), r"\\? and \\?x");
}

#[test]
fn ansi_colours_do_not_hide_values() {
    let r = Redactor::new(&known());
    assert_eq!(r.redact("\u{1b}[31m10.1.2.3\u{1b}[0m"), "<ip>");
    assert_eq!(r.redact("\u{1b}[1;31m/home/eve/x\u{1b}[0m"), "<path>");
    assert_eq!(r.redact("web-01\u{1b}[0m.example.com"), "<host>");
    assert_eq!(r.redact("\u{9b}32mdb"), "<host>");
    assert_eq!(r.redact("\u{1b}[?25h/home/eve/x\u{1b}[2K"), "<path>");
    assert_eq!(r.redact("10.1.2\u{1b}[m.3"), "<ip>");
    assert_eq!(r.redact("failed:\u{1b}[31m10.9.8.7"), "failed:<ip>");
    // Unterminated CSI and other escapes (OSC) stay for the escaping pass.
    assert_eq!(r.redact("a\u{1b}[12"), "a\u{1b}[12");
    assert_eq!(r.redact("\u{1b}]8;;x\u{7}"), "\u{1b}]8;;x\u{7}");
    // A bare `ESC [` is no sequence: it never swallows the first character of a value.
    assert_eq!(r.redact("\u{1b}[alice"), "\u{1b}[<user>");
    assert_eq!(r.redact("\u{1b}[/home/eve/x"), "\u{1b}[<path>");
    assert_eq!(r.redact("\u{1b}[C:\\Users\\carol\\x"), "\u{1b}[<path>");
    assert_eq!(r.redact("\u{1b}[K db"), "\u{1b}[K <host>");
    // A removed sequence leaves a space between two values, so neither is glued to the other.
    assert_eq!(r.redact("10.1.2.3\u{1b}[0mabc"), "<ip> abc");
    assert_eq!(r.redact("x\u{1b}[m/home/eve/x"), "x <path>");
    assert_eq!(r.redact("alice\u{1b}[mX"), "<user> X");
    assert_eq!(r.redact("a \u{1b}[1m\u{1b}[31mb"), "a b");
}

#[test]
fn credential_keys_are_redacted() {
    let r = Redactor::new(&KnownValues::default());
    for (input, expected) in [
        ("password=hunter2 next", "password=<redacted> next"),
        ("password: hunter2", "password: <redacted>"),
        (
            r#"{"passphrase":"a b c"}"#,
            r#"{"passphrase":"<redacted>"}"#,
        ),
        (
            r#"Config { passwd: "x" }"#,
            r#"Config { passwd: "<redacted>" }"#,
        ),
        (
            "api_key=abc123 apiKey: def",
            "api_key=<redacted> apiKey: <redacted>",
        ),
        ("access_token='a b' x", "access_token='<redacted>' x"),
        ("secret=s3, token=t0", "secret=<redacted>, token=<redacted>"),
        ("refresh_token=r1", "refresh_token=<redacted>"),
        (
            "Authorization: Bearer eyJhbGci.payload.sig rest",
            "Authorization: <redacted> rest",
        ),
        (
            "authorization=Basic dXNlcjpwYXNz",
            "authorization=<redacted>",
        ),
        (
            "GET https://example.com/x?token=abc&sig=zzz&v=1",
            "GET https://example.com/x?token=<redacted>&sig=<redacted>&v=1",
        ),
        (
            r#"url "https://example.com/?signature=q1#top""#,
            r#"url "https://example.com/?signature=<redacted>#top""#,
        ),
        ("token: Some(abc)", "token: Some(<redacted>)"),
        ("X-Api-Key: abc", "X-Api-Key: <redacted>"),
        ("x_api-key=abc", "x_api-key=<redacted>"),
        (
            "pass=p1 credentials: c1",
            "pass=<redacted> credentials: <redacted>",
        ),
        ("auth=Basic dXNlcjpwYXNz x", "auth=<redacted> x"),
        ("Cookie: a=1; sid=s2", "Cookie: <redacted>"),
        (
            "Set-Cookie: sid=s2; Path=/; HttpOnly\nnext",
            "Set-Cookie: <redacted>\nnext",
        ),
        // `-` keys keep the other classes: the last segment counts.
        ("my-host=db.example", "my-host=<host>"),
    ] {
        assert_eq!(r.redact(input), expected, "{input}");
    }
    // Not keys: inside words, without a value, already a placeholder.
    for keep in [
        "mytokens=1",
        "password:",
        "password: <masked>",
        "password=***",
        "token",
        "x-tokens=1",
        r#""auth": {"method": "agent"}"#,
    ] {
        assert_eq!(r.redact(keep), keep, "{keep}");
    }
}

#[test]
fn percent_encoded_at_is_a_user_separator() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("ssh://carol%40db.internal/"),
        "ssh://<user>%40<host>/"
    );
    assert_eq!(r.redact("a%40198.51.100.4"), "<user>%40<ip>");
    // Percent-encoded userinfo before a real `@` is one user.
    assert_eq!(
        r.redact("https://carol%40example.com@host.example/"),
        "https://<user>@<host>/"
    );
    assert_eq!(r.redact("x%2Fy%40z@h.example"), "<user>@<host>");
    assert_eq!(
        r.redact("50%40 x %40y react%401.2.3"),
        "50%40 x %40y react%401.2.3"
    );
}

#[test]
fn paths_with_spaces_are_replaced_whole() {
    let r = Redactor::new(&known());
    assert_eq!(
        r.redact("cannot read /srv/My Project/notes.txt: denied"),
        "cannot read <path>: denied"
    );
    assert_eq!(
        r.redact(r"C:\Data\Client X\Q3 Report\a.xlsx missing"),
        "<path> missing"
    );
    // A following word without a separator, a key=value or a URL is not part of the path.
    assert_eq!(r.redact("/srv/app2 failed"), "<path> failed");
    assert_eq!(r.redact("/srv/x file=src/a.rs"), "<path> file=src/a.rs");
    assert_eq!(
        r.redact("/srv/x see https://example.org/a"),
        "<path> see https://example.org/a"
    );
    // Each path on its own.
    assert_eq!(r.redact("/a/b /c/d"), "<path> <path>");
}

#[test]
fn other_users_tilde_paths_are_replaced() {
    let r = Redactor::new(&known());
    assert_eq!(r.redact("read ~carol/.ssh/id_ed25519"), "read <path>");
    assert_eq!(
        r.redact("~/.ssh/config ~5 minutes ~bob"),
        "~/.ssh/config ~5 minutes ~bob"
    );
}

#[test]
fn more_host_keys_lists_and_ports() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("server=db.corp.example addr=192.0.2.4:5432 peer=198.51.100.9:22"),
        "server=<host> addr=<ip>:5432 peer=<ip>:22"
    );
    assert_eq!(
        r.redact(r#"{"address":"[2001:db8::7]:22","remote":"r.example","hostKeyAlias":"alias.example","label":"Prod DB"}"#),
        r#"{"address":"<ip>:22","remote":"<host>","hostKeyAlias":"<host>","label":"<host>"}"#
    );
    // Lists (`proxyJumpCycle.chain`), JSON and Debug.
    assert_eq!(
        r.redact(r#"{"chain":["jump-a.example","jump-b.example","jump-a.example"]}"#),
        r#"{"chain":["<host>","<host>","<host>"]}"#
    );
    assert_eq!(
        r.redact(r#"ProxyJumpCycle { chain: ["a1.example", "b2.example"] } next"#),
        r#"ProxyJumpCycle { chain: ["<host>", "<host>"] } next"#
    );
}

#[test]
fn idn_hosts_are_known_in_both_forms() {
    let mut k = KnownValues::default();
    k.add_host("bücher.example");
    let r = Redactor::new(&k);
    assert_eq!(
        r.redact("bücher.example and xn--bcher-kva.example"),
        "<host> and <host>"
    );
}

#[test]
fn ipv4_next_to_dashes_and_underscores() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("node-10.1.2.3-22 and _10.1.2.3 x"),
        "node-<ip>-22 and _<ip> x"
    );
    // Still not inside alphanumerics, and IPv6 still needs word boundaries.
    assert_eq!(
        r.redact("v10.1.2.3 10.1.2.3a x_ab::cd"),
        "v10.1.2.3 10.1.2.3a x_ab::cd"
    );
}

#[test]
fn ipv4_right_after_a_word_and_colon() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("connect failed:10.9.8.7:22"),
        "connect failed:<ip>:22"
    );
    assert_eq!(r.redact("peer:10.9.8.6"), "peer:<ip>");
    assert_eq!(r.redact("addr:10.9.8.4,"), "addr:<ip>,");
    // IPv6 too, unless it is shaped like a Rust path (only `::` separators, no digit).
    assert_eq!(r.redact("addr:2001:db8::5"), "addr:<ip>");
    assert_eq!(r.redact("peer:fe80::1234"), "peer:<ip>");
    assert_eq!(
        r.redact("connect failed:2001:db8::5 port 22"),
        "connect failed:<ip> port 22"
    );
    assert_eq!(r.redact("address:2001:db8::5."), "address:<ip>.");
    // Ambiguous `x:hex::hex` with a digit is redacted (in doubt, redact).
    assert_eq!(r.redact("peer:ab::c1"), "peer:<ip>");
    // Still not addresses: times, versions, Rust paths, words around the run.
    for keep in [
        "at 12:34:56",
        "T10:15:02.123Z",
        "ts=2026-10-23T10:15:02.123Z",
        "v1.2.3.4 build:1.2.3 rel:1.2.3.4.5",
        "shellfin_lib::commands::app Error::Io",
        "x:ab::cd mod:dead::beef crate:ab::cd addr:dead::beef",
        "std::io::Error a::b::c",
        "peer:10.9.8.6x",
        "peer::1 addr:127.0.0.1",
    ] {
        assert_eq!(r.redact(keep), keep, "{keep}");
    }
}

#[test]
fn user_at_bracketed_ipv6_drops_the_user() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        r.redact("ssh alice@[2001:db8::3]:22"),
        "ssh <user>@[<ip>]:22"
    );
    assert_eq!(r.redact("bob@[::1]"), "<user>@[::1]");
    // Not an address in the brackets: the user stays like any `name@[…]`.
    assert_eq!(r.redact("x@[not-ip] @[2001:db8::3]"), "x@[not-ip] @[<ip>]");
}

#[test]
fn quoted_paths_with_spaces_are_replaced_whole() {
    let r = Redactor::new(&known());
    assert_eq!(
        r.redact("open '/opt/Client Files/a b.txt' failed"),
        "open '<path>' failed"
    );
    assert_eq!(
        r.redact(r#"read "D:\Client X\Q3 plan.xlsx": denied"#),
        r#"read "<path>": denied"#
    );
    // Each quoted path on its own; an unclosed quote follows the unquoted rules.
    assert_eq!(r.redact("'/a b' and '/c d'"), "'<path>' and '<path>'");
    assert_eq!(r.redact("'/srv/x y.txt"), "'<path> y.txt");
    // The closing quote must be on the same line.
    assert_eq!(r.redact("'/srv/x\ny z'"), "'<path>\ny z'");
}

#[test]
fn tilde_user_name_window_is_bounded() {
    let r = Redactor::new(&known());
    let ok = format!("~{}/x", "a".repeat(MAX_TILDE_USER_CHARS));
    assert_eq!(r.redact(&ok), "<path>");
    let long = format!("~{}/x", "a".repeat(MAX_TILDE_USER_CHARS + 1));
    assert_eq!(r.redact(&long), long);
}

/// Inputs that made the redaction quadratic (a 64K backslash run took over a minute): every
/// case must stay linear. The bound is generous for slow debug builds; quadratic behaviour on
/// these sizes takes far longer.
#[test]
fn hostile_inputs_redact_in_linear_time() {
    let r = Redactor::new(&known());
    let n = 64 * 1024;
    let unchanged = [
        "\\".repeat(n),
        "~".repeat(n),
        "~~~~/".repeat(n / 5),
        "(~a".repeat(n / 3),
        "(\\".repeat(n / 2),
        "(\\\\".repeat(n / 3),
        ":".repeat(n),
        ".".repeat(n),
        "a:".repeat(n / 2),
        "'".repeat(n),
        "@".repeat(n),
        "[".repeat(n),
        "a@[".repeat(n / 3),
        "(\\\\?".repeat(n / 4),
        "file:".repeat(n / 5),
        "\u{1b}".repeat(n),
        "\u{1b}[".repeat(n / 2),
        "\u{1b}[1".repeat(n / 3),
        "\u{9b}".repeat(n),
        "%40".repeat(n / 3),
        "%4".repeat(n / 2),
        "a%40[".repeat(n / 5),
        "\u{1b}[a".repeat(n / 3),
        "\u{1b}[?".repeat(n / 3),
        "a-".repeat(n / 2),
        "%4".repeat(n / 2) + "@",
    ];
    for input in &unchanged {
        let start = std::time::Instant::now();
        let out = r.redact(input);
        let took = start.elapsed();
        assert_eq!(&out, input, "{}", input.get(..12).unwrap_or(input));
        assert!(
            took < std::time::Duration::from_secs(3),
            "{} took {took:?}",
            input.get(..12).unwrap_or(input)
        );
    }
    // Inputs with many matches (one span per match).
    let replaced = [
        ("(~bob/x".repeat(n / 7), "(<path>".to_owned()),
        // Each quote closes the previous path and opens the next.
        ("'/a ".repeat(n / 4), "'<path>".repeat(n / 4)),
        ("a@b ".repeat(n / 4), "<user>@<host> ".repeat(n / 4)),
        ("10.9.8.7 ".repeat(n / 9), "<ip> ".repeat(n / 9)),
        ("p:10.9.8.7 ".repeat(n / 11), "p:<ip> ".repeat(n / 11)),
        // Glued to the next word: not an address (only the last one, at the end, is).
        (
            "p:10.9.8.7".repeat(n / 10),
            format!("{}p:<ip>", "p:10.9.8.7".repeat(n / 10 - 1)),
        ),
        ("\\\\srv\\s ".repeat(n / 8), "<path> ".repeat(n / 8)),
        ("\\\\?\\D:\\x ".repeat(n / 9), "<path> ".repeat(n / 9)),
        // One verbatim path (`\\?\` then `\?\\?…`).
        ("\\\\?".repeat(n / 3), "<path>".to_owned()),
        ("file:/".repeat(n / 6), "file:<path>".to_owned()),
        ("file:///".repeat(n / 8), "file://<path>".to_owned()),
        ("file:///x ".repeat(n / 10), "file://<path> ".repeat(n / 10)),
        ("\u{1b}[1m".repeat(n / 4), String::new()),
        ("\u{1b}[1mx".repeat(n / 5), "x ".repeat(n / 5)),
        ("file://".repeat(n / 7), "file://<path>".to_owned()),
        (
            "file://s/x ".repeat(n / 11),
            "file://<path> ".repeat(n / 11),
        ),
        ("%41".repeat(n / 3) + "@h", "<user>@<host>".to_owned()),
        ("a%40".repeat(n / 4) + "@h", "<user>@<host>".to_owned()),
        (
            "X-Api-Key: a ".repeat(n / 13),
            "X-Api-Key: <redacted> ".repeat(n / 13),
        ),
        ("Cookie: ".repeat(n / 8), "Cookie: <redacted>".to_owned()),
        ("\u{1b}[31m10.1.2.3 ".repeat(n / 16), "<ip> ".repeat(n / 16)),
        ("a%40b ".repeat(n / 6), "<user>%40<host> ".repeat(n / 6)),
        ("token=".repeat(n / 6), "token=<redacted>".to_owned()),
        ("token=x ".repeat(n / 8), "token=<redacted> ".repeat(n / 8)),
        ("?sig=x&".repeat(n / 7), "?sig=<redacted>&".repeat(n / 7)),
        (
            "Authorization: Bearer x ".repeat(n / 24),
            "Authorization: <redacted> ".repeat(n / 24),
        ),
    ];
    for (input, expected) in &replaced {
        let start = std::time::Instant::now();
        let out = r.redact(input);
        let took = start.elapsed();
        assert_eq!(
            out.trim_end(),
            expected.trim_end(),
            "{}",
            input.get(..12).unwrap_or(input)
        );
        assert!(
            took < std::time::Duration::from_secs(3),
            "{} took {took:?}",
            input.get(..12).unwrap_or(input)
        );
    }
    // Unclosed or chained quotes and bare keys after credential keys: only the time matters.
    for input in [
        "token='".repeat(n / 7),
        "password='x".repeat(n / 11),
        "password: ".repeat(n / 10),
        "authorization: bearer ".repeat(n / 22),
    ] {
        let start = std::time::Instant::now();
        let out = r.redact(&input);
        let took = start.elapsed();
        assert!(!out.is_empty());
        assert!(
            took < std::time::Duration::from_secs(3),
            "{} took {took:?}",
            input.get(..12).unwrap_or(&input)
        );
    }
}

#[test]
fn plus_addressing_and_fingerprints() {
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(r.redact("mail carol+ops@example.org"), "mail <user>@<host>");
    assert_eq!(
        r.redact("key SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s for the host"), // gitleaks:allow
        "key SHA256:<redacted> for the host"
    );
    assert_eq!(
        r.redact("MD5:16:27:ac:a5:76:28:2d:36:63:1b:56:4d:eb:df:a6:48 ok"),
        "MD5:<redacted> ok"
    );
    // Short values are not fingerprints.
    assert_eq!(r.redact("SHA256:abc"), "SHA256:abc");
}

#[test]
fn localhost_as_computer_name_is_not_redacted() {
    let local = LocalIdentity {
        user: "me".into(),
        computer: Some("localhost.localdomain".into()),
        home: HOME_UNIX.into(),
    };
    let k = collect_known(&BookmarksDto::default(), &[], &[], &[], "", &local);
    let r = Redactor::new(&k);
    assert_eq!(
        r.redact("at http://tauri.localhost/a.js:1:2"),
        "at http://tauri.localhost/a.js:1:2"
    );
}

// ---- escaping ------------------------------------------------------------------------------

#[test]
fn controls_bidi_and_invisibles_become_visible() {
    assert_eq!(
        escape_line("a\u{1b}[2Jb\tc\u{7f}\u{9b}"),
        "a\\x1b[2Jb\\x09c\\x7f\\x9b"
    );
    assert_eq!(escape_line("x\u{202E}gpj.exe"), "x⟨U+202E⟩gpj.exe");
    assert_eq!(
        escape_line("a\u{200B}b\u{FEFF}c\u{2066}d\u{E0041}e\u{FE0F}f\u{FFFB}"),
        "a⟨U+200B⟩b⟨U+FEFF⟩c⟨U+2066⟩d⟨U+E0041⟩e⟨U+FE0F⟩f⟨U+FFFB⟩"
    );
    assert_eq!(escape_line("Привіт ✓"), "Привіт ✓");
    // In a report text: line separators escaped, newlines kept as line breaks.
    let r = Redactor::new(&KnownValues::default());
    assert_eq!(
        clean_text(&r, "one\r\ntwo\u{2028}three\rfour", 10, 100),
        "one\ntwo⟨U+2028⟩three\nfour"
    );
}

#[test]
fn redaction_sees_values_hidden_by_nothing_but_escapes_after() {
    // A known host next to a bidi control is still found (redaction runs before escaping).
    let r = Redactor::new(&known());
    let out = clean_text(&r, "\u{202E}web-01.example.com\u{202C}", 1, 100);
    assert_eq!(out, "⟨U+202E⟩<host>⟨U+202C⟩");
}

#[test]
fn fences_are_longer_than_any_backtick_run() {
    assert_eq!(fenced("text", "a"), "```text\na\n```\n");
    let f = fenced("text", "x ```` y");
    assert!(
        f.starts_with("`````text\n") && f.ends_with("\n`````\n"),
        "{f}"
    );
}

// ---- validation ----------------------------------------------------------------------------

#[test]
fn codes_contexts_and_languages_are_validated() {
    assert_eq!(valid_code("remoteNotFound"), "remoteNotFound");
    let long_code = "a".repeat(65);
    for bad in ["", "1abc", "a b", "a`b", "<x>", long_code.as_str()] {
        assert_eq!(valid_code(bad), "unknown", "{bad}");
    }
    assert_eq!(
        valid_context("file.newFolder").as_deref(),
        Some("file.newFolder")
    );
    assert_eq!(valid_context("sftp_list").as_deref(), Some("sftp_list"));
    let long_context = "c".repeat(81);
    for bad in ["", "a b", "x`", "a/b", long_context.as_str()] {
        assert_eq!(valid_context(bad), None, "{bad}");
    }
    assert_eq!(valid_language("pt-BR").as_deref(), Some("pt-BR"));
    assert_eq!(valid_language("en\n"), None);
}

#[test]
fn utc_times() {
    assert_eq!(format_utc(0).unwrap(), "1970-01-01 00:00:00 UTC");
    assert_eq!(
        format_utc(1_792_750_502_123).unwrap(),
        "2026-10-23 10:15:02 UTC"
    );
    assert_eq!(format_utc(-1), None);
    assert_eq!(format_utc(i64::MAX), None);
}

// ---- environment ---------------------------------------------------------------------------

/// The option texts must match `.github/ISSUE_TEMPLATE/bug_report.yml` exactly, or GitHub
/// ignores the pre-filled value.
#[test]
fn dropdown_options_match_the_bug_form() {
    let yml = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.github/ISSUE_TEMPLATE")
            .join(ISSUE_TEMPLATE),
    )
    .unwrap()
    .replace("\r\n", "\n");
    let options = |id: &str| -> Vec<String> {
        let block = yml.split(&format!("id: {id}\n")).nth(1).unwrap();
        let block = block.split("options:\n").nth(1).unwrap();
        block
            .lines()
            .map_while(|l| l.trim().strip_prefix("- ").map(str::to_owned))
            .collect()
    };
    assert_eq!(options("install"), INSTALL_OPTIONS);
    assert_eq!(options("arch"), ARCH_OPTIONS);
    for id in ["version", "os", "arch", "install", "actual", "logs"] {
        assert!(yml.contains(&format!("id: {id}\n")), "field {id}");
    }
}

#[test]
fn install_and_arch_options() {
    use crate::ipc::dto::Platform;
    use tauri::utils::config::BundleType;
    let all = |p, dev, app, b| install_option(p, dev, app, b);
    assert_eq!(
        all(Platform::Windows, false, false, Some(BundleType::Nsis)),
        Some(INSTALL_OPTIONS[0])
    );
    assert_eq!(
        all(Platform::Windows, false, false, Some(BundleType::Msi)),
        None
    );
    assert_eq!(
        all(Platform::Windows, false, false, None),
        Some(INSTALL_OPTIONS[6])
    );
    assert_eq!(
        all(Platform::Macos, false, false, Some(BundleType::App)),
        None
    );
    assert_eq!(
        all(Platform::Linux, false, false, Some(BundleType::Deb)),
        Some(INSTALL_OPTIONS[4])
    );
    assert_eq!(
        all(Platform::Linux, false, false, Some(BundleType::Rpm)),
        Some(INSTALL_OPTIONS[5])
    );
    assert_eq!(
        all(Platform::Linux, false, false, Some(BundleType::AppImage)),
        Some(INSTALL_OPTIONS[3])
    );
    assert_eq!(
        all(Platform::Linux, false, true, None),
        Some(INSTALL_OPTIONS[3])
    );
    assert_eq!(
        all(Platform::Linux, false, false, None),
        Some(INSTALL_OPTIONS[6])
    );
    assert_eq!(
        all(Platform::Linux, true, true, Some(BundleType::Deb)),
        Some(INSTALL_OPTIONS[7])
    );
    for o in [
        all(Platform::Windows, false, false, Some(BundleType::Nsis)),
        all(Platform::Linux, true, false, None),
    ]
    .into_iter()
    .flatten()
    {
        assert!(INSTALL_OPTIONS.contains(&o));
    }
    assert_eq!(arch_option("windows", "x86_64"), ARCH_OPTIONS[0]);
    assert_eq!(arch_option("linux", "x86_64"), ARCH_OPTIONS[0]);
    assert_eq!(arch_option("windows", "aarch64"), ARCH_OPTIONS[1]);
    assert_eq!(arch_option("macos", "aarch64"), ARCH_OPTIONS[2]);
    assert_eq!(arch_option("linux", "aarch64"), ARCH_OPTIONS[3]);
    assert_eq!(arch_option("macos", "x86_64"), ARCH_OPTIONS[3]);
}

#[test]
fn current_environment_is_plausible() {
    let e = current_environment("0.2.0", Some("en"));
    assert_eq!(e.app_version, "0.2.0");
    assert_eq!(e.arch, std::env::consts::ARCH);
    assert_eq!(e.ui_language.as_deref(), Some("en"));
    // Tests are debug builds.
    assert_eq!(
        e.install.as_deref(),
        Some("Running in development (npm run tauri dev)")
    );
    assert_eq!(current_environment("0.2.0", Some("e n")).ui_language, None);
}

// ---- collected values ----------------------------------------------------------------------

fn ssh_host(alias: &str, hostname: &str, user: Option<&str>, jump: Option<&str>) -> SshConfigHost {
    SshConfigHost {
        alias: alias.into(),
        hostname: hostname.into(),
        user: user.map(Into::into),
        port: 22,
        proxy_jump: jump.map(Into::into),
        identity_files: vec!["~/.ssh/id_work".into()],
        origin: SourceLocation {
            file: "~/.ssh/config".into(),
            line: 1,
        },
        has_warnings: false,
    }
}

#[test]
fn app_state_values_are_collected() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, _) = crate::storage::bookmarks::BookmarkStore::load(dir.path());
    let input: crate::ipc::dto::BookmarkInput = serde_json::from_value(serde_json::json!({
        "id": null, "name": "Customer Prod", "host": "app.customer.example", "port": 2222,
        "user": "svc-app", "auth": {"method": "key", "identityFile": "D:/keys/customer.pem"},
        "proxyJump": {"kind": "spec", "spec": "jumper@bastion.example:2200"},
        "groupId": null, "tags": [], "startDir": "/opt/customer", "useSshConfig": false,
        "startupCommand": "export LEVEL=debug", "notes": "call Bob first"
    }))
    .unwrap();
    store.save(input, 1).unwrap();
    let bookmarks: BookmarksDto = store.dto();
    let recents = vec![RecentEntry {
        target: RecentTarget::Quick {
            spec: "carol@[2001:db8::5]:22".into(),
        },
        label: "carol@quick".into(),
        user_at_host: "carol@quickhost.example".into(),
        via: Some("bastion2.example".into()),
        last_connected_at_ms: 1,
        last_dir: Some("/var/carol".into()),
        terminal_count: 0,
    }];
    let sessions = vec![OpenSession {
        label: "Staging".into(),
        target: RecentTarget::SshConfig {
            alias: "stage".into(),
        },
        hops: vec![crate::ipc::dto::HopRef {
            index: 0,
            label: "stage".into(),
            user: "stageuser".into(),
            host: "resolved-stage.example".into(),
            port: 22,
        }],
        dirs: vec!["/home/stageuser".into(), "/var/www/stage-site".into()],
    }];
    let k = collect_known(
        &bookmarks,
        &[ssh_host(
            "work",
            "10.20.30.40",
            Some("wuser"),
            Some("hop.example"),
        )],
        &recents,
        &sessions,
        "defuser",
        &LocalIdentity {
            user: "localme".into(),
            computer: Some("DESKTOP-ABC123".into()),
            home: HOME_WIN.into(),
        },
    );
    let r = Redactor::new(&k);
    let text = "Customer Prod app.customer.example svc-app D:/keys/customer.pem jumper bastion.example \
                /opt/customer carol quickhost.example 2001:db8::5 bastion2.example /var/carol Staging \
                stage work 10.20.30.40 wuser hop.example defuser localme DESKTOP-ABC123 \
                stageuser resolved-stage.example in-home:/home/stageuser served-from:/var/www/stage-site \
                C:\\Users\\Alice\\x";
    let out = r.redact(text);
    for leak in [
        "Customer",
        "customer",
        "svc-app",
        "jumper",
        "bastion",
        "carol",
        "quickhost",
        "2001:db8",
        "Staging",
        "stage",
        "work",
        "10.20",
        "wuser",
        "hop.",
        "defuser",
        "localme",
        "DESKTOP",
        "Alice",
        "resolved-stage",
        "/home/stageuser",
        "stage-site",
    ] {
        assert!(!out.contains(leak), "{leak} leaked: {out}");
    }
    assert!(out.ends_with("~\\x"), "{out}");
}

// ---- the report ----------------------------------------------------------------------------

fn sample_input(log: Option<Vec<String>>) -> ReportInput {
    ReportInput {
        code: Some("remoteNotFound".into()),
        params: Some(serde_json::json!({ "path": "/srv/app/x", "host": "web-01.example.com" })),
        detail: Some("sftp: no such file /srv/app/x on deploy@web-01.example.com".into()),
        message: Some("“/srv/app/x” was not found on web-01.example.com\u{202E}".into()),
        context: Some("sftp_stat".into()),
        stack: Some("Error: x\n    at f (http://tauri.localhost/assets/a.js:1:2)\n    at C:\\Users\\Alice\\y".into()),
        occurred_at_ms: Some(1_792_750_502_123),
        log,
    }
}

#[test]
fn report_contains_everything_redacted() {
    let log = vec![
        "2026-10-23T10:15:00Z  INFO shellfin: starting Shellfin version=0.2.0".to_owned(),
        "2026-10-23T10:15:01Z  WARN shellfin: ignoring known_hosts file file=/home/alice/.ssh/known_hosts reason=x".to_owned(),
        "2026-10-23T10:15:02Z ERROR shellfin: failed error=RemoteNotFound { path: \"/srv/app/x\" } peer=198.51.100.9:22".to_owned(),
    ];
    let report = build(&sample_input(Some(log)), &env(), &known());
    let md = &report.markdown;
    for leak in [
        "web-01",
        "example.com",
        "/srv/app",
        "deploy",
        "Alice",
        "198.51.100",
        "/home/alice",
        "\u{202E}",
    ] {
        assert!(!md.contains(leak), "{leak} leaked:\n{md}");
        assert!(!report.issue_url.contains(leak), "{leak} leaked in the URL");
    }
    for expected in [
        "## Error",
        "- Code: `remoteNotFound`",
        "- Where: `sftp_stat`",
        "- Time: 2026-10-23 10:15:02 UTC",
        "⟨U+202E⟩",
        "\"host\":\"<host>\"",
        "\"path\":\"<path>\"",
        "## Environment",
        "- Shellfin: 0.2.0",
        "- Installed as: Windows installer (NSIS .exe)",
        "- UI language: uk",
        "## Log (last 3 lines, redacted)",
        "file=~/.ssh/known_hosts",
        "peer=<ip>:22",
        "http://tauri.localhost/assets/a.js:1:2",
        "at ~\\y",
    ] {
        assert!(md.contains(expected), "missing {expected}:\n{md}");
    }
    assert!(report.log_included);
    assert_eq!(report.log_lines, 3);
    assert_eq!(report.log_lines_in_url, 3);
    assert!(report.issue_url.len() <= MAX_URL_CHARS);
    assert!(normalize_external_url(&report.issue_url).is_ok());
}

#[test]
fn report_without_error_has_the_environment_only() {
    let report = build(&ReportInput::default(), &env(), &known());
    assert!(!report.markdown.contains("## Error"));
    assert!(report.markdown.starts_with("## Environment\n"));
    assert!(!report.log_included);
    assert_eq!(report.log_lines, 0);
    assert!(!report.issue_url.contains("title="));
    assert!(!report.issue_url.contains("actual="));
    assert!(report.issue_url.contains("version=0.2.0"));
    // The log was requested but there was none.
    let report = build(
        &ReportInput {
            log: Some(Vec::new()),
            ..ReportInput::default()
        },
        &env(),
        &known(),
    );
    assert!(!report.log_included);
    assert!(!report.markdown.contains("## Log"));
}

#[test]
fn report_never_contains_bookmark_notes_or_startup_commands() {
    // Only the given texts and the environment are in a report; the app state is used only to
    // find values to remove. A masked secret in a log line stays masked.
    let log = vec!["startup_command=*** password=***".to_owned()];
    let report = build(&sample_input(Some(log)), &env(), &known());
    assert!(report.markdown.contains("startup_command=*** password=***"));
    for absent in ["export LEVEL", "call Bob"] {
        assert!(!report.markdown.contains(absent));
    }
}

#[test]
fn hostile_input_is_bounded() {
    let input = ReportInput {
        code: Some("x`y\n## injected".into()),
        params: Some(serde_json::json!({ "k": "v".repeat(100_000) })),
        detail: Some("d\n".repeat(100_000)),
        message: Some(format!(
            "\u{0}\u{1b}]8;;http://evil/\u{7}{}",
            "m".repeat(100_000)
        )),
        context: Some("bad context`".into()),
        stack: Some("at x\n".repeat(10_000)),
        occurred_at_ms: Some(i64::MIN),
        log: Some(vec!["l".repeat(1_000_000); 3]),
    };
    let report = build(&input, &env(), &known());
    let md = &report.markdown;
    assert!(md.contains("- Code: `unknown`"));
    assert!(!md.contains("## injected"));
    assert!(!md.contains("- Where:"));
    assert!(!md.contains("- Time:"));
    assert!(!md.contains('\u{0}') && !md.contains('\u{1b}') && !md.contains('\u{7}'));
    assert!(md.contains("\\x1b]8;;"));
    // Message + params + detail + stack + 3 log lines, each bounded.
    let bound = MAX_MESSAGE_CHARS
        + MAX_PARAMS_CHARS
        + MAX_DETAIL_CHARS
        + MAX_STACK_CHARS
        + 3 * MAX_LINE_CHARS
        + 2000;
    assert!(md.chars().count() <= bound, "{}", md.chars().count());
    assert!(report.issue_url.len() <= MAX_URL_CHARS);
    // A 2000-character log line does not fit into the URL: no `logs` field at all.
    assert_eq!(report.log_lines_in_url, 0);
    assert!(!report.issue_url.contains("&logs="));
}

// ---- the URL -------------------------------------------------------------------------------

fn summary(message: &str) -> UrlSummary {
    UrlSummary {
        code: "bug".into(),
        context: Some("sftp_list".into()),
        time: Some("2026-10-23 10:15:02 UTC".into()),
        message: message.into(),
    }
}

fn query_value(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

#[test]
fn url_has_the_form_fields() {
    let (url, n) = issue_url(&env(), Some(&summary("Something & else = 100% <b>")), &[]);
    assert_eq!(n, 0);
    assert!(url.starts_with(
        "https://github.com/shellfin-app/shellfin/issues/new?template=bug_report.yml&"
    ));
    assert_eq!(
        query_value(&url, "title").unwrap(),
        "[Bug]: Error bug in sftp_list"
    );
    assert_eq!(query_value(&url, "version").unwrap(), "0.2.0");
    assert_eq!(query_value(&url, "os").unwrap(), env().os);
    assert_eq!(
        query_value(&url, "install").unwrap(),
        "Windows installer (NSIS .exe)"
    );
    assert!(ARCH_OPTIONS.contains(&query_value(&url, "arch").unwrap().as_str()));
    let actual = query_value(&url, "actual").unwrap();
    assert!(
        actual.starts_with("Error `bug` in `sftp_list` at 2026-10-23 10:15:02 UTC:"),
        "{actual}"
    );
    assert!(actual.contains("Something & else = 100% <b>"), "{actual}");
    assert_eq!(query_value(&url, "logs"), None);
    // Exactly what app_open_external opens.
    assert_eq!(normalize_external_url(&url).unwrap().as_str(), url);
}

#[test]
fn unknown_install_text_is_omitted() {
    let mut e = env();
    e.install = Some("Something else".into());
    let (url, _) = issue_url(&e, None, &[]);
    assert_eq!(query_value(&url, "install"), None);
    e.install = None;
    let (url, _) = issue_url(&e, None, &[]);
    assert_eq!(query_value(&url, "install"), None);
}

#[test]
fn logs_fill_the_url_with_the_newest_whole_lines() {
    let log: Vec<String> = (0..500).map(|i| format!("line {i:03} ü&=%+ é")).collect();
    let (url, n) = issue_url(&env(), Some(&summary("msg")), &log);
    assert!(url.len() <= MAX_URL_CHARS, "{}", url.len());
    assert!(n > 5 && n < 500, "{n}");
    let logs = query_value(&url, "logs").unwrap();
    let expected = log[log.len() - n..].join("\n");
    assert_eq!(logs, expected, "whole lines, newest last");
    // One more line (its encoding plus an encoded newline) would not have fitted.
    let next: String =
        url::form_urlencoded::byte_serialize(log[log.len() - n - 1].as_bytes()).collect();
    assert!(url.len() + next.len() + 3 > MAX_URL_CHARS);
    assert!(normalize_external_url(&url).is_ok());
}

#[test]
fn url_is_always_within_the_limit() {
    // Worst cases: long non-ASCII messages, OS strings and lines that encode to 9 chars each.
    let heavy = "ж\u{2028}€".repeat(400);
    let mut e = env();
    e.os = heavy.clone();
    e.app_version = heavy.clone();
    for message in ["", "x", &heavy, &"%".repeat(5000)] {
        for log in [vec![], vec![heavy.clone()], vec!["short".to_owned(); 1000]] {
            let (url, n) = issue_url(&e, Some(&summary(message)), &log);
            assert!(url.len() <= MAX_URL_CHARS, "{}", url.len());
            assert!(n <= log.len());
            assert!(normalize_external_url(&url).is_ok());
        }
    }
}

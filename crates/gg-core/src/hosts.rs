//! Remote addresses as hosting sites see them: the host, owner and repository a remote's URL
//! names, whether that host is GitHub, and the page that compares two branches there (the
//! fallback when a pull request cannot be made through `gh`). No user name, password or token
//! in a URL is ever kept.

/// The repository a remote's URL names on its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// The host, lowercase, without a port (`github.com`, `github.example.com`).
    pub host: String,
    /// The host as its web pages are addressed: with the port of an `https://` URL that
    /// gives one (an `ssh://` port is not the web server's).
    pub web: String,
    pub owner: String,
    /// The repository's name, without `.git`.
    pub name: String,
}

/// The repository `url` (a remote's address) names, if it names one as `owner/name` on a
/// host: `https://[user@]host[:port]/owner/name[.git]`, `http://…`, `ssh://[user@]host[:port]/
/// owner/name[.git]`, `git://…`, or `[user@]host:owner/name[.git]` (scp-like). Local paths,
/// `file://` and anything with more or fewer path segments are `None`. GitHub's ssh-over-443
/// host (`ssh.github.com`) is `github.com`. The host of an ssh address is as written, which
/// may be an alias of the user's ssh config: see [`ssh_host`] and [`RemoteRepo::on_host`].
pub fn parse_remote(url: &str) -> Option<RemoteRepo> {
    parse(url).map(|(repo, _)| repo)
}

/// The host git reaches `url` at through ssh (`ssh://`, `git+ssh://`, `ssh+git://` or
/// scp-like), as written there, lowercase: possibly an alias of the user's ssh config
/// (`github.com-work`), whose real host name `ssh -G` tells. `None` for other addresses.
pub fn ssh_host(url: &str) -> Option<String> {
    parse(url).and_then(|(_, ssh)| ssh)
}

impl RemoteRepo {
    /// This repository on `host` instead: the real host name an ssh alias stands for. Its
    /// pages are addressed by that name (an ssh port is not the web server's).
    pub fn on_host(mut self, host: &str) -> Self {
        let host = normal_host(host);
        self.web.clone_from(&host);
        self.host = host;
        self
    }
}

/// `host` lowercase, without one trailing dot (the root of a fully qualified name).
fn lower_host(host: &str) -> String {
    host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase()
}

/// `host` as [`lower_host`] gives it; GitHub's ssh-over-443 host is `github.com`.
fn normal_host(host: &str) -> String {
    let host = lower_host(host);
    if host == "ssh.github.com" {
        "github.com".to_owned()
    } else {
        host
    }
}

/// [`parse_remote`], and the host as written when the address is reached through ssh.
fn parse(url: &str) -> Option<(RemoteRepo, Option<String>)> {
    let url = url.trim();
    let (host, path, web_port, ssh) = if let Some((scheme, _)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(
            scheme.as_str(),
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
        ) {
            return None;
        }
        let parsed = url::Url::parse(url).ok()?;
        let host = parsed.host_str()?.to_owned();
        let web_port = parsed.port().filter(|_| scheme == "https");
        let ssh = matches!(scheme.as_str(), "ssh" | "git+ssh" | "ssh+git");
        (host, parsed.path().to_owned(), web_port, ssh)
    } else {
        // scp-like: `[user@]host:path`, the host without `/` (else it is a local path) and
        // longer than a drive letter (`C:\…`).
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') || authority.contains('\\') || path.starts_with("//") {
            return None;
        }
        let host = authority.rsplit('@').next()?;
        if host.len() < 2 {
            return None;
        }
        (host.to_owned(), path.to_owned(), None, true)
    };
    let written = lower_host(&host);
    let host = normal_host(&host);
    // Only a plain host name is a hosting site's: `x?.ghe.com` ends like one, but git and
    // `gh` read another host out of it (Task 9 review I1).
    if !is_plain_host(&written) || !is_plain_host(&host) {
        return None;
    }
    let segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let [owner, name] = segments.as_slice() else {
        return None;
    };
    let name = name.strip_suffix(".git").unwrap_or(name);
    if owner.is_empty() || name.is_empty() || owner.starts_with('~') {
        return None;
    }
    let web = match web_port {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let repo = RemoteRepo {
        host,
        web,
        owner: (*owner).to_owned(),
        name: name.to_owned(),
    };
    Some((repo, ssh.then_some(written)))
}

/// Whether `host` is a plain host name: `^[A-Za-z0-9]([A-Za-z0-9._-]{0,251}[A-Za-z0-9])?$`
/// (letters, digits, `.`, `_`, `-`; neither starting nor ending with a dot or hyphen; at most
/// 253 characters). Only such a name is handed to `ssh -G`, and only such a name is taken
/// from its answer: ssh puts the host into `Match exec` and `ProxyCommand` strings of the
/// user's ssh config (`%h`, `%n`), which a shell runs, so `$(…)`, a backtick, `;`, `|`,
/// quotes, `%` or a space in a remote's address must never reach it.
pub fn is_plain_host(host: &str) -> bool {
    let bytes = host.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    bytes.len() <= 253
        && first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether `host` is GitHub by its name alone: `github.com`, or an Enterprise Cloud host
/// (`*.ghe.com`). Any other host (an Enterprise Server's) is taken for GitHub only when `gh`
/// already has credentials stored for exactly that host (`RepoManager::on_github`), never by
/// its name (`github.example.net` may be anyone's): otherwise `gh` would be pointed at it
/// with `GH_HOST` and could send it a token. Other hosts are not supported (spec §5.5).
/// Case and one trailing dot do not matter; a host that is not a plain name
/// ([`is_plain_host`]) never is GitHub.
pub fn is_known_github(host: &str) -> bool {
    let host = lower_host(host);
    is_plain_host(&host) && (host == "github.com" || host.ends_with(".ghe.com"))
}

/// The page on `repo`'s GitHub host that compares `head` (a branch of `head_owner`'s fork,
/// when given) with `base` and offers to open a pull request: `https://<host>/<owner>/<repo>/
/// compare/<base>...[<head owner>:]<head>?expand=1`, each name percent-encoded. Only for a
/// repository whose host is known to be GitHub (`RepoManager::on_github`).
pub fn compare_url(repo: &RemoteRepo, base: &str, head: &str, head_owner: Option<&str>) -> String {
    let head = match head_owner {
        Some(owner) => format!("{}:{}", encode(owner), encode(head)),
        None => encode(head),
    };
    format!(
        "https://{}/{}/{}/compare/{}...{head}?expand=1",
        repo.web,
        encode(&repo.owner),
        encode(&repo.name),
        encode(base)
    )
}

/// `text` percent-encoded for a URL path: every byte but letters, digits, `-._~` and `/`.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(host: &str, web: &str, owner: &str, name: &str) -> RemoteRepo {
        RemoteRepo {
            host: host.into(),
            web: web.into(),
            owner: owner.into(),
            name: name.into(),
        }
    }

    #[test]
    fn remote_urls_name_their_host_owner_and_repository() {
        let gh = repo("github.com", "github.com", "octo", "cat");
        for url in [
            "https://github.com/octo/cat.git",
            "https://github.com/octo/cat",
            "https://github.com/octo/cat/",
            "HTTPS://GitHub.com/octo/cat.git",
            "http://github.com/octo/cat.git",
            "ssh://git@github.com/octo/cat.git",
            "ssh://git@github.com:22/octo/cat.git",
            "ssh://git@ssh.github.com:443/octo/cat.git",
            "git://github.com/octo/cat.git",
            "git@github.com:octo/cat.git",
            "git@github.com:octo/cat",
            "github.com:octo/cat.git",
        ] {
            assert_eq!(parse_remote(url), Some(gh.clone()), "{url}");
        }
        // Enterprise hosts, with ports: the web port only from an https URL.
        assert_eq!(
            parse_remote("https://github.example.com:8443/team/app.git"),
            Some(repo(
                "github.example.com",
                "github.example.com:8443",
                "team",
                "app"
            ))
        );
        assert_eq!(
            parse_remote("ssh://git@github.example.com:2222/team/app.git"),
            Some(repo(
                "github.example.com",
                "github.example.com",
                "team",
                "app"
            ))
        );
        assert_eq!(
            parse_remote("git@acme.ghe.com:team/app.git"),
            Some(repo("acme.ghe.com", "acme.ghe.com", "team", "app"))
        );
        // Not a repository on a host.
        for url in [
            "/srv/git/app.git",
            "../app.git",
            "file:///srv/git/app.git",
            r"C:\repos\app.git",
            "C:/repos/app.git",
            "https://github.com/octo",
            "https://gitlab.com/group/sub/app.git",
            "https://example.com/~user/app.git",
            "",
        ] {
            assert_eq!(parse_remote(url), None, "{url}");
        }
    }

    #[test]
    fn only_plain_host_names_are_plain() {
        for host in [
            "github.com",
            "gh-work",
            "github.com-work",
            "a",
            "a_b.c-d",
            "x9",
        ] {
            assert!(is_plain_host(host), "{host}");
        }
        let long = "a".repeat(254);
        for host in [
            "",
            "$(touch x)",
            "a`id`b",
            "a;b",
            "%h",
            "a%hb",
            "a'b",
            "a\"b",
            ".lead",
            "trail.",
            "-lead",
            "trail-",
            "a b",
            "a|b",
            "a&b",
            "a\nb",
            "[::1]",
            "a:22",
            long.as_str(),
        ] {
            assert!(!is_plain_host(host), "{host:?}");
        }
        assert!(is_plain_host(&"a".repeat(253)));
    }

    /// The host an ssh address is reached at, as written (maybe an alias of the ssh config).
    #[test]
    fn ssh_addresses_name_their_host_as_written() {
        for (url, host) in [
            ("git@github.com-work:me/r.git", Some("github.com-work")),
            ("git@gh-work:me/r.git", Some("gh-work")),
            ("ssh://git@GH-Work:22/me/r.git", Some("gh-work")),
            (
                "git+ssh://git@ssh.github.com:443/me/r.git",
                Some("ssh.github.com"),
            ),
            ("https://github.com/me/r.git", None),
            ("git://github.com/me/r.git", None),
            ("/srv/git/r.git", None),
        ] {
            assert_eq!(ssh_host(url).as_deref(), host, "{url}");
        }
        let aliased = parse_remote("git@gh-work:me/r.git").unwrap();
        assert_eq!(aliased.host, "gh-work");
        assert_eq!(
            aliased.on_host("SSH.GitHub.com."),
            repo("github.com", "github.com", "me", "r")
        );
    }

    /// A user name, password or token in the URL is never kept anywhere in what is parsed,
    /// nor in the compare page made from it.
    #[test]
    fn credentials_in_a_remote_url_are_dropped() {
        for url in [
            "https://ada:ghp_SECRET@github.com/octo/cat.git",
            "https://x-access-token:SECRET@github.example.com:8443/octo/cat.git",
            "ssh://SECRET@github.com/octo/cat.git",
        ] {
            let parsed = parse_remote(url).unwrap();
            let all = format!("{parsed:?}");
            assert!(!all.contains("SECRET") && !all.contains("ada"), "{all}");
            let page = compare_url(&parsed, "main", "x", None);
            assert!(!page.contains("SECRET") && !page.contains('@'), "{page}");
            assert_eq!(
                (parsed.owner.as_str(), parsed.name.as_str()),
                ("octo", "cat")
            );
        }
    }

    /// Only github.com and Enterprise Cloud are GitHub by name; a host named `github.*` is
    /// not (it needs credentials stored in `gh` for it).
    #[test]
    fn github_hosts_are_known_by_name() {
        for host in ["github.com", "acme.ghe.com"] {
            assert!(is_known_github(host), "{host}");
        }
        for host in [
            "github.example.com",
            "github.example.net",
            "github.corp",
            "ghe.com.evil.example",
            "gitlab.com",
            "bitbucket.org",
            "git.example.com",
            "notgithub.com",
        ] {
            assert!(!is_known_github(host), "{host}");
        }
    }

    /// Security (Task 9 review I1): a host that is not a plain name is no hosting site's,
    /// whatever it ends with: `x?.ghe.com` would pass a suffix test while git and gh read
    /// another host. Case and one trailing dot do not matter.
    #[test]
    fn only_a_plain_host_name_is_a_hosting_sites() {
        for url in [
            "git@evil.example?.ghe.com:o/n.git",
            "git@evil.example#.ghe.com:o/n.git",
            "git@a b.ghe.com:o/n.git",
            "git@x%2e.ghe.com:o/n.git",
            "git@x%2E.ghe.com:o/n.git",
            "git@x..:o/n.git",
            "git@-x.ghe.com:o/n.git",
        ] {
            assert_eq!(parse_remote(url), None, "{url}");
            assert_eq!(ssh_host(url), None, "{url}");
        }
        let upper = parse_remote("git@ACME.GHE.COM.:o/n.git").unwrap();
        assert_eq!(upper.host, "acme.ghe.com");
        assert_eq!(
            ssh_host("git@ACME.GHE.COM.:o/n.git").as_deref(),
            Some("acme.ghe.com")
        );
        for host in [
            "x?.ghe.com",
            "x#.ghe.com",
            "a b.ghe.com",
            "x%2e.ghe.com",
            ".ghe.com",
            "github.com..",
        ] {
            assert!(!is_known_github(host), "{host}");
        }
        for host in ["GitHub.com", "github.com.", "ACME.ghe.com."] {
            assert!(is_known_github(host), "{host}");
        }
    }

    #[test]
    fn the_compare_page_names_both_branches_encoded() {
        let gh = repo("github.com", "github.com", "octo", "cat");
        assert_eq!(
            compare_url(&gh, "main", "feature/login", None),
            "https://github.com/octo/cat/compare/main...feature/login?expand=1"
        );
        assert_eq!(
            compare_url(&gh, "release 1", "fix#2+é", None),
            "https://github.com/octo/cat/compare/release%201...fix%232%2B%C3%A9?expand=1"
        );
        // A fork's branch, by its owner.
        assert_eq!(
            compare_url(&gh, "main", "topic", Some("ada")),
            "https://github.com/octo/cat/compare/main...ada:topic?expand=1"
        );
        let ghe = repo("github.example.com", "github.example.com:8443", "t", "a");
        assert_eq!(
            compare_url(&ghe, "main", "x", None),
            "https://github.example.com:8443/t/a/compare/main...x?expand=1"
        );
    }
}

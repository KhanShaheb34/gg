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
/// host (`ssh.github.com`) is `github.com`.
pub fn parse_remote(url: &str) -> Option<RemoteRepo> {
    let url = url.trim();
    let (host, path, web_port) = if let Some((scheme, _)) = url.split_once("://") {
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
        (host, parsed.path().to_owned(), web_port)
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
        (host.to_owned(), path.to_owned(), None)
    };
    let mut host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    if host == "ssh.github.com" {
        host = "github.com".to_owned();
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
    Some(RemoteRepo {
        host,
        web,
        owner: (*owner).to_owned(),
        name: name.to_owned(),
    })
}

/// Whether `host` is GitHub by its name alone: `github.com`, or an Enterprise Cloud host
/// (`*.ghe.com`). Any other host (an Enterprise Server's) is taken for GitHub only when `gh`
/// already has credentials stored for exactly that host (`RepoManager::on_github`), never by
/// its name (`github.example.net` may be anyone's): otherwise `gh` would be pointed at it
/// with `GH_HOST` and could send it a token. Other hosts are not supported (spec §5.5).
pub fn is_known_github(host: &str) -> bool {
    host == "github.com" || host.ends_with(".ghe.com")
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

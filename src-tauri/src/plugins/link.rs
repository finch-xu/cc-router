//! 解析「从链接安装」里用户粘贴的东西。纯函数。
//!
//! 接受:
//! - `https://github.com/owner/repo`
//! - `https://github.com/owner/repo/tree/<ref>[/子目录]` (也认 `/blob/`)
//! - `https://github.com/owner/repo/commit/<提交号>`
//! - `owner/repo`、`owner/repo@<ref>`、`owner/repo@<ref>/子目录`
//!
//! ref (分支 / tag / 提交号) 不能含 `/`: 否则与子目录分不开。

use super::files::is_safe_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    /// None = 默认分支
    pub git_ref: Option<String>,
    /// 仓库内子目录 (清单所在位置)
    pub path: Option<String>,
}

fn valid_owner(s: &str) -> bool {
    (1..=39).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') && !s.starts_with('-')
}

fn valid_repo(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && s != "."
        && s != ".."
}

fn valid_ref(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && !s.contains("..")
}

pub fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

pub fn parse(input: &str) -> Result<RepoRef, String> {
    const HINT: &str = "请粘贴 GitHub 仓库链接, 或 owner/repo@提交号";
    let s = input.trim();
    let s = s.split(['?', '#']).next().unwrap_or("").trim_end_matches('/');
    let url_rest = ["https://github.com/", "http://github.com/", "https://www.github.com/", "github.com/"]
        .iter()
        .find_map(|p| s.strip_prefix(p));

    let (owner, repo, git_ref, path): (&str, &str, Option<&str>, Option<String>) = if let Some(rest) = url_rest {
        let segs: Vec<&str> = rest.split('/').collect();
        if segs.len() < 2 {
            return Err(HINT.into());
        }
        let repo = segs[1].trim_end_matches(".git");
        match segs.get(2).copied() {
            None => (segs[0], repo, None, None),
            Some("tree") | Some("blob") => {
                let r = segs.get(3).copied().ok_or(HINT)?;
                let sub = segs[4..].join("/");
                (segs[0], repo, Some(r), (!sub.is_empty()).then_some(sub))
            }
            Some("commit") if segs.len() == 4 => (segs[0], repo, Some(segs[3]), None),
            _ => return Err(HINT.into()),
        }
    } else {
        if s.contains("://") {
            return Err("目前只支持 GitHub 仓库".into());
        }
        let (repo_part, ref_part) = match s.split_once('@') {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        let (owner, repo) = repo_part.split_once('/').ok_or(HINT)?;
        if repo.contains('/') {
            return Err(HINT.into());
        }
        match ref_part {
            None => (owner, repo, None, None),
            Some(rp) => match rp.split_once('/') {
                Some((r, sub)) => (owner, repo, Some(r), Some(sub.to_string())),
                None => (owner, repo, Some(rp), None),
            },
        }
    };

    if !valid_owner(owner) || !valid_repo(repo) {
        return Err(HINT.into());
    }
    if let Some(r) = git_ref {
        if !valid_ref(r) {
            return Err("分支 / tag / 提交号的格式不对".into());
        }
    }
    if let Some(p) = &path {
        if !is_safe_path(p) {
            return Err("仓库子目录的路径不合法".into());
        }
    }
    Ok(RepoRef {
        owner: owner.to_string(),
        repo: repo.trim_end_matches(".git").to_string(),
        git_ref: git_ref.map(str::to_string),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(owner: &str, repo: &str, git_ref: Option<&str>, path: Option<&str>) -> RepoRef {
        RepoRef { owner: owner.into(), repo: repo.into(), git_ref: git_ref.map(Into::into), path: path.map(Into::into) }
    }

    #[test]
    fn parses_supported_forms() {
        let sha = "9f3c2a1b9f3c2a1b9f3c2a1b9f3c2a1b9f3c2a1b";
        assert_eq!(parse("https://github.com/a/b").unwrap(), r("a", "b", None, None));
        assert_eq!(parse(" https://github.com/a/b.git/ ").unwrap(), r("a", "b", None, None));
        assert_eq!(parse("https://github.com/a/b/tree/main").unwrap(), r("a", "b", Some("main"), None));
        assert_eq!(parse("https://github.com/a/b/tree/v1.0.0/themes/dusk").unwrap(), r("a", "b", Some("v1.0.0"), Some("themes/dusk")));
        assert_eq!(parse(&format!("https://github.com/a/b/commit/{sha}")).unwrap(), r("a", "b", Some(sha), None));
        assert_eq!(parse("https://github.com/a/b?tab=readme#x").unwrap(), r("a", "b", None, None));
        assert_eq!(parse("a/b").unwrap(), r("a", "b", None, None));
        assert_eq!(parse(&format!("a/b@{sha}")).unwrap(), r("a", "b", Some(sha), None));
        assert_eq!(parse("a/b@main/x/y").unwrap(), r("a", "b", Some("main"), Some("x/y")));
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "https://gitlab.com/a/b",
            "https://github.com/a",
            "https://github.com/a/b/issues/1",
            "a",
            "a/b/c",
            "a/b@feature/../x",
            "a/b@ma in",
            "https://github.com/a/b/tree/main/../../etc",
            "-a/b",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn full_sha() {
        assert!(is_full_sha("9f3c2a1b9f3c2a1b9f3c2a1b9f3c2a1b9f3c2a1b"));
        assert!(!is_full_sha("9f3c2a1"));
    }
}

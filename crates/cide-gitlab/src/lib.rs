//! GitLab transport and session ownership. This crate never depends on a webview.
mod drafts;
mod fetch;
mod workspaces;
use cide_ipc::gitlab::*;
pub use drafts::NewDraft;
use parking_lot::Mutex;
use reqwest::{Method, Url, blocking::Client};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};
use workspaces::Workspaces;

pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Serialize, Deserialize)]
struct Credential {
    account: GitLabAccount,
    token: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    revision: u64,
    credentials: Vec<Credential>,
    reviews: Vec<GitLabReview>,
    preferences: GitLabPreferences,
}

pub struct GitLab {
    saved: Mutex<Saved>,
    path: PathBuf,
    workspaces: Workspaces,
    proxy: Mutex<cide_ipc::ProxySettings>,
    drafts: drafts::Drafts,
    /// The latest MR version with its diffs, per review, briefly. An agent writes its findings
    /// one tool call at a time, and each one needs the diff to place its line; thirty drafts
    /// must not be sixty GitLab requests.
    versions: Mutex<std::collections::HashMap<String, (std::time::Instant, Value)>>,
}

/// How long [`GitLab::latest_version`] trusts what it fetched. Short: a push to the MR makes
/// a new version, and a draft placed on the old one is marked outdated rather than wrong.
const VERSION_TTL: Duration = Duration::from_secs(60);

pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn host_url(raw: &str) -> Result<Url> {
    let mut url = Url::parse(raw.trim()).map_err(|_| "Enter a full GitLab HTTPS URL")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err("GitLab connections require HTTPS without embedded credentials".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}
fn api_url(host: &str, path: &str) -> Result<Url> {
    Url::parse(&format!("{}/api/v4/{path}", host.trim_end_matches('/')))
        .map_err(|_| "Invalid GitLab endpoint".into())
}
/// Match a host prefix on a path boundary, including GitLab installed under /gitlab.
pub fn project_from_url(host: &str, remote: &str) -> Result<String> {
    let host = host_url(host)?;
    let raw = if !remote.contains("://") && remote.contains(':') {
        let (left, path) = remote.split_once(':').ok_or("Invalid remote URL")?;
        format!(
            "https://{}/{}",
            left.rsplit('@').next().unwrap_or(left),
            path
        )
    } else {
        remote.to_string()
    };
    let url = Url::parse(&raw).map_err(|_| "Invalid GitLab URL")?;
    if url.host_str() != host.host_str()
        || (url.scheme() != "ssh" && url.port_or_known_default() != host.port_or_known_default())
    {
        return Err("This URL belongs to another GitLab host".into());
    }
    let path = url
        .path()
        .strip_prefix(host.path())
        .ok_or("URL is outside the configured GitLab installation")?;
    let project = path
        .split("/-/")
        .next()
        .unwrap_or(path)
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if project.is_empty() {
        return Err("Missing GitLab project".into());
    }
    Ok(project.to_string())
}
fn mr_iid(url: &str) -> Result<u64> {
    let url = Url::parse(url).map_err(|_| "Invalid MR URL")?;
    url.path()
        .split("/-/merge_requests/")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse().ok())
        .filter(|id| *id > 0)
        .ok_or("Paste a GitLab merge request URL".into())
}
fn number(v: &Value, key: &str) -> Result<u64> {
    v[key]
        .as_u64()
        .ok_or_else(|| format!("GitLab response is missing {key}"))
}
fn string(v: &Value, key: &str) -> Result<String> {
    v[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("GitLab response is missing {key}"))
}
fn response(data: Value) -> GitLabResponse {
    GitLabResponse {
        data,
        next_page: None,
    }
}

impl GitLab {
    pub fn load(path: PathBuf, temporary: PathBuf) -> Result<Self> {
        let saved = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Cannot read saved GitLab connections; the file was left unchanged")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(_) => return Err("Cannot read saved GitLab connections".into()),
        };
        let drafts = drafts::Drafts::load(path.with_file_name("gitlab-drafts.json"));
        Ok(Self {
            saved: Mutex::new(saved),
            path,
            workspaces: Workspaces::new(temporary)?,
            proxy: Mutex::new(cide_ipc::ProxySettings::default()),
            drafts,
            versions: Mutex::new(Default::default()),
        })
    }
    pub fn set_proxy(&self, proxy: cide_ipc::ProxySettings) {
        *self.proxy.lock() = proxy;
    }
    fn save(&self, saved: &mut Saved) -> Result<()> {
        saved.revision = saved.revision.saturating_add(1);
        let parent = self.path.parent().ok_or("Invalid GitLab storage path")?;
        fs::create_dir_all(parent).map_err(|_| "Cannot create GitLab storage")?;
        let tmp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options
                .open(&tmp)
                .map_err(|_| "Cannot save GitLab connections")?;
            file.write_all(
                &serde_json::to_vec(saved).map_err(|_| "Cannot encode GitLab connections")?,
            )
            .map_err(|_| "Cannot save GitLab connections")?;
            file.sync_all()
                .map_err(|_| "Cannot flush GitLab connections")?;
            fs::rename(&tmp, &self.path).map_err(|_| "Cannot replace GitLab connections")
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        Ok(result?)
    }
    pub fn board(&self) -> GitLabBoard {
        let saved = self.saved.lock();
        GitLabBoard {
            revision: saved.revision,
            accounts: saved
                .credentials
                .iter()
                .map(|c| c.account.clone())
                .collect(),
            reviews: saved.reviews.clone(),
            preferences: saved.preferences.clone(),
        }
    }
    fn credential(&self, id: &str) -> Result<Credential> {
        self.saved
            .lock()
            .credentials
            .iter()
            .find(|c| c.account.id == id)
            .cloned()
            .ok_or("Connect this GitLab account again".into())
    }
    fn review(&self, id: &str) -> Result<(GitLabReview, Credential)> {
        let review = self
            .saved
            .lock()
            .reviews
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or("This review has been closed")?;
        let c = self.credential(&review.account)?;
        Ok((review, c))
    }
    fn client(&self) -> Result<Client> {
        let proxy = self.proxy.lock();
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60));
        use cide_ipc::{ProxyMode, ProxyTarget};
        match (proxy.scope.git, proxy.mode) {
            (ProxyTarget::Direct, _) | (ProxyTarget::Configured, ProxyMode::Direct) => {
                builder = builder.no_proxy()
            }
            (ProxyTarget::Configured, ProxyMode::Manual) => {
                builder = builder.no_proxy();
                if let Some(url) = proxy
                    .https_url()
                    .or_else(|| cide_ipc::normalize_proxy_url(&proxy.all))
                {
                    let bypass = reqwest::NoProxy::from_string(&proxy.no_proxy);
                    builder = builder.proxy(
                        reqwest::Proxy::https(url)
                            .map_err(|_| "Invalid HTTPS proxy")?
                            .no_proxy(bypass),
                    );
                }
            }
            _ => {}
        }
        builder
            .build()
            .map_err(|_| "Cannot initialize GitLab HTTP client".into())
    }
    fn request(
        &self,
        c: &Credential,
        method: Method,
        path: &str,
        params: &[(&str, String)],
        body: Option<Value>,
        raw: bool,
    ) -> Result<GitLabResponse> {
        let mut url = api_url(&c.account.host, path)?;
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let mut request = self
            .client()?
            .request(method, url)
            .header("PRIVATE-TOKEN", &c.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut result = request.send().map_err(
            |_| "GitLab request failed; check your connection, TLS certificate, and proxy",
        )?;
        // Archived traces can be redirected to signed object-storage URLs. Never forward
        // the private token to a redirect destination; ordinary API calls still refuse redirects.
        if raw && path.ends_with("/trace") {
            for _ in 0..5 {
                if !result.status().is_redirection() {
                    break;
                }
                let location = result
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("GitLab trace redirect has no destination")?;
                let destination = result
                    .url()
                    .join(location)
                    .map_err(|_| "Invalid trace redirect")?;
                if destination.scheme() != "https"
                    || !destination.username().is_empty()
                    || destination.password().is_some()
                {
                    return Err("GitLab trace redirect must use HTTPS".into());
                }
                result = self
                    .client()?
                    .get(destination)
                    .send()
                    .map_err(|_| "Could not retrieve archived job log")?;
            }
        }
        let status = result.status();
        if !status.is_success() {
            let retry = result
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            return Err(match status.as_u16() {
                401 => "GitLab token expired or is invalid; reconnect the account".into(),
                403 => "GitLab denied this action; check your role and token api scope".into(),
                404 => "GitLab resource is unavailable or you do not have access".into(),
                409 => "The MR changed; refresh before approving or commenting".into(),
                429 => format!("GitLab rate limit reached. Retry after {retry} seconds."),
                _ => format!("GitLab returned HTTP {}", status.as_u16()),
            });
        }
        let next_page = result
            .headers()
            .get("x-next-page")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());
        let limit = if raw {
            8 * 1024 * 1024
        } else {
            32 * 1024 * 1024
        };
        let mut bytes = Vec::new();
        result
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read GitLab response")?;
        if bytes.len() > limit as usize {
            return Err("GitLab response exceeds the viewer limit; open it in GitLab".into());
        }
        let data = if raw {
            if bytes.contains(&0) && !path.ends_with("/trace") {
                return Err("This file is binary; open it in GitLab".into());
            }
            json!(
                String::from_utf8(bytes)
                    .map_err(|_| "This file is binary and cannot be displayed as text")?
            )
        } else if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| "GitLab returned an invalid response")?
        };
        Ok(GitLabResponse { data, next_page })
    }
    fn get(&self, c: &Credential, path: &str) -> Result<Value> {
        Ok(self.request(c, Method::GET, path, &[], None, false)?.data)
    }
    pub fn shutdown(&self) {
        self.workspaces.shutdown();
    }
    pub fn cancel_workspace(&self, review: &str) {
        self.workspaces.cancel(review);
    }
    pub fn review_open(&self, review: &str) -> bool {
        self.saved.lock().reviews.iter().any(|r| r.id == review)
    }
    pub fn workspace_versions(&self, review: &str) -> Vec<String> {
        self.workspaces.versions(review)
    }
    pub fn workspace_root(&self, review: &str, sha: &str) -> Option<PathBuf> {
        self.workspaces.root(review, sha)
    }
    /// The durable identity of an open review, for a caller outside this crate.
    pub fn review_of(&self, review: &str) -> Result<GitLabReview> {
        Ok(self.review(review)?.0)
    }
    /// The account an open review is read through: its host, for matching local remotes.
    pub fn account_of(&self, review: &str) -> Result<GitLabAccount> {
        Ok(self.review(review)?.1.account)
    }
    /// The MR's latest version, diffs included — what a draft's position is computed against.
    pub fn latest_version(&self, review: &str) -> Result<Value> {
        if let Some((at, version)) = self.versions.lock().get(review)
            && at.elapsed() < VERSION_TTL
        {
            return Ok(version.clone());
        }
        let (r, c) = self.review(review)?;
        let base = format!("projects/{}/merge_requests/{}", r.project, r.iid);
        let versions = self.get(&c, &format!("{base}/versions"))?;
        let latest = versions
            .as_array()
            .and_then(|v| v.first())
            .and_then(|v| v["id"].as_u64())
            .ok_or("GitLab is preparing the MR diff; try again shortly")?;
        let version = self.get(&c, &format!("{base}/versions/{latest}"))?;
        for key in ["base_commit_sha", "start_commit_sha", "head_commit_sha"] {
            if !version[key]
                .as_str()
                .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("GitLab is still preparing the MR revisions; try again shortly".into());
            }
        }
        self.versions.lock().insert(
            review.to_string(),
            (std::time::Instant::now(), version.clone()),
        );
        Ok(version)
    }
    /// The MR itself, as `Detail` answers it.
    pub fn detail(&self, review: &str) -> Result<Value> {
        Ok(self
            .execute(GitLabRequest::Detail {
                review: review.into(),
            })?
            .data)
    }
    /// Every discussion thread on the MR, all pages.
    pub fn discussions(&self, review: &str) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        let mut page = 1;
        loop {
            let response = self.execute(GitLabRequest::Discussions {
                review: review.into(),
                page,
            })?;
            all.extend(response.data.as_array().cloned().unwrap_or_default());
            match response.next_page {
                Some(next) if next > page => page = next,
                _ => return Ok(all),
            }
        }
    }
    /// A disposable checkout of the MR head that also holds the base commit, so
    /// `git diff <base> <head>` answers inside it. The base is best effort: a failure leaves a
    /// working checkout of the head, and the caller says so instead of refusing the review.
    pub fn review_checkout(&self, review: &str, head: &str, base: &str) -> Result<(String, bool)> {
        let root = self
            .execute(GitLabRequest::Checkout {
                review: review.into(),
                sha: head.into(),
            })?
            .data["root"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (r, c) = self.review(review)?;
        let mr = self.get(
            &c,
            &format!("projects/{}/merge_requests/{}", r.project, r.iid),
        )?;
        // The base lives in the *target* project; a fork's MR fetches it from there.
        let target = mr["target_project_id"].as_u64().unwrap_or(r.project);
        let url = string(
            &self.get(&c, &format!("projects/{target}"))?,
            "http_url_to_repo",
        )?;
        host_url(&url)?;
        project_from_url(&c.account.host, &url)?;
        let proxy = self.proxy.lock().clone();
        let with_base = match self
            .workspaces
            .fetch_into(review, head, base, &url, &c.token, &proxy)
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "the MR base could not be fetched into the review checkout");
                false
            }
        };
        Ok((root, with_base))
    }
    pub fn drafts(&self, review: &str) -> Vec<GitLabDraft> {
        self.drafts.list(review)
    }
    /// Write a draft for an agent run. Never reaches GitLab.
    pub fn draft_create(
        &self,
        review: &str,
        new: NewDraft,
        author: GitLabDraftAuthor,
    ) -> Result<GitLabDraft> {
        self.review(review)?;
        let version = self.latest_version(review)?;
        self.drafts
            .insert(drafts::compose(review, &version, new, author)?)
    }
    /// An edit by a run (`Some`) is limited to its own drafts; the user's (`None`) is not.
    pub fn draft_edit(
        &self,
        review: &str,
        draft: &str,
        body: Option<String>,
        severity: Option<GitLabSeverity>,
        run: Option<&str>,
    ) -> Result<GitLabDraft> {
        self.drafts.edit(review, draft, body, severity, run)
    }
    pub fn draft_discard(
        &self,
        review: &str,
        drafts: &[String],
        run: Option<&str>,
    ) -> Result<usize> {
        self.drafts.discard(review, drafts, run)
    }
    fn publish(&self, review: &str, ids: Vec<String>) -> Result<GitLabPublished> {
        let mut result = GitLabPublished {
            published: Vec::new(),
            failed: Vec::new(),
        };
        for id in ids {
            let outcome = self.drafts.get(review, &id).and_then(|draft| {
                // The body exactly as written: the severity is the draft's metadata, and
                // publishing it would put a label into somebody else's review thread.
                self.execute(GitLabRequest::Comment {
                    review: review.into(),
                    body: draft.body,
                    discussion: None,
                    position: draft.position,
                })
            });
            match outcome {
                // Removed one at a time, after its own POST: a batch that fails half-way leaves
                // exactly the unpublished half behind, never a posted comment still marked draft.
                Ok(_) => match self.drafts.discard(review, std::slice::from_ref(&id), None) {
                    Ok(_) => result.published.push(id),
                    Err(error) => result.failed.push(GitLabPublishFailure {
                        draft: id,
                        error: format!(
                            "Posted to GitLab, but the local draft could not be removed: {error}"
                        ),
                    }),
                },
                Err(error) => result
                    .failed
                    .push(GitLabPublishFailure { draft: id, error }),
            }
        }
        Ok(result)
    }
    fn comparison(&self, doc: &cide_ipc::gitlab::GitLabDocument) -> Result<GitLabResponse> {
        let (r, c) = self.review(&doc.review)?;
        let mr = self.get(
            &c,
            &format!("projects/{}/merge_requests/{}", r.project, r.iid),
        )?;
        let read = |project: u64, path: &str, sha: &str| -> Result<String> {
            validate_relative(path)?;
            if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Invalid review revision".into());
            }
            let response = self.request(
                &c,
                Method::GET,
                &format!("projects/{project}/repository/files/{}/raw", encode(path)),
                &[("ref", sha.to_owned())],
                None,
                true,
            )?;
            Ok(response.data.as_str().unwrap_or_default().to_owned())
        };
        let old = if doc.new_file {
            String::new()
        } else {
            read(
                mr["target_project_id"].as_u64().unwrap_or(r.project),
                &doc.old_path,
                &doc.base_sha,
            )?
        };
        let new = if doc.deleted_file {
            String::new()
        } else {
            read(
                mr["source_project_id"]
                    .as_u64()
                    .ok_or("MR source project has been removed")?,
                &doc.path,
                &doc.head_sha,
            )?
        };
        let diff = comparison_from_text(doc, old, new)?;
        Ok(GitLabResponse {
            data: serde_json::to_value(diff).map_err(|e| e.to_string())?,
            next_page: None,
        })
    }
    pub fn execute(&self, request: GitLabRequest) -> Result<GitLabResponse> {
        use GitLabRequest::*;
        if let Comparison { document } = &request {
            return self.comparison(document);
        }
        match request {
            Drafts { review } => {
                self.review(&review)?;
                Ok(response(json!(self.drafts.list(&review))))
            }
            DraftEdit {
                review,
                draft,
                body,
                severity,
            } => Ok(response(json!(
                self.drafts.edit(&review, &draft, body, severity, None)?
            ))),
            DraftDiscard { review, drafts } => Ok(response(json!(
                self.drafts.discard(&review, &drafts, None)?
            ))),
            DraftPublish { review, drafts } => {
                self.review(&review)?;
                Ok(response(json!(self.publish(&review, drafts)?)))
            }
            Board => Ok(response(serde_json::to_value(self.board()).unwrap())),
            Connect { host, token } => {
                let host = host_url(&host)?.as_str().trim_end_matches('/').to_string();
                if token.trim().is_empty() {
                    return Err("Enter a personal access token with api scope".into());
                }
                let mut c = Credential {
                    account: GitLabAccount {
                        id: uuid::Uuid::new_v4().to_string(),
                        host,
                        username: String::new(),
                        user_id: 0,
                    },
                    token,
                };
                let user = self.get(&c, "user")?;
                c.account.username = string(&user, "username")?;
                c.account.user_id = number(&user, "id")?;
                let mut saved = self.saved.lock();
                if let Some(old) = saved.credentials.iter_mut().find(|old| {
                    old.account.host == c.account.host && old.account.user_id == c.account.user_id
                }) {
                    c.account.id = old.account.id.clone();
                    *old = c;
                } else {
                    saved.credentials.push(c);
                }
                self.save(&mut saved)?;
                drop(saved);
                self.execute(Board)
            }
            Disconnect { account } => {
                let ids: Vec<_> = self
                    .saved
                    .lock()
                    .reviews
                    .iter()
                    .filter(|r| r.account == account)
                    .map(|r| r.id.clone())
                    .collect();
                for id in &ids {
                    self.workspaces.close(id)?;
                    self.drafts.forget(id)?;
                    self.versions.lock().remove(id);
                }
                let mut saved = self.saved.lock();
                saved.credentials.retain(|c| c.account.id != account);
                saved.reviews.retain(|r| r.account != account);
                self.save(&mut saved)?;
                drop(saved);
                self.execute(Board)
            }
            Preferences { preferences } => {
                for p in &preferences.excluded_files {
                    if p.contains(['[', ']', '{', '}', '\\']) {
                        return Err("Review patterns support *, **, and ? wildcards; character classes and braces are not supported".into());
                    }
                    globset::Glob::new(p).map_err(|e| format!("Invalid file pattern: {e}"))?;
                }
                let mut saved = self.saved.lock();
                saved.preferences = preferences;
                self.save(&mut saved)?;
                drop(saved);
                self.execute(Board)
            }
            List {
                account,
                scope,
                state,
                search,
                project,
                page,
            } => {
                let c = self.credential(&account)?;
                if !["opened", "closed", "merged", "all"].contains(&state.as_str()) {
                    return Err("Invalid MR state".into());
                }
                let mut params = vec![
                    ("scope", "all".into()),
                    ("state", state),
                    ("search", search),
                    ("page", page.max(1).to_string()),
                    ("per_page", "50".into()),
                    ("order_by", "updated_at".into()),
                ];
                params.push((
                    match scope.as_str() {
                        "created" => "author_id",
                        "review" => "reviewer_id",
                        "assigned" => "assignee_id",
                        _ => return Err("Invalid inbox filter".into()),
                    },
                    c.account.user_id.to_string(),
                ));
                let path = project
                    .filter(|p| !p.is_empty())
                    .map(|p| format!("projects/{}/merge_requests", encode(&p)))
                    .unwrap_or("merge_requests".into());
                self.request(&c, Method::GET, &path, &params, None, false)
            }
            Open { account, url } => {
                let c = self.credential(&account)?;
                let project = project_from_url(&c.account.host, &url)?;
                let iid = mr_iid(&url)?;
                let mr = self.get(
                    &c,
                    &format!("projects/{}/merge_requests/{iid}", encode(&project)),
                )?;
                let project = number(&mr, "project_id")?;
                let id = blake3::hash(format!("{}:{project}:{iid}", c.account.host).as_bytes())
                    .to_hex()
                    .to_string();
                let review = GitLabReview {
                    id: id.clone(),
                    account,
                    project,
                    iid,
                    title: string(&mr, "title")?,
                    url: string(&mr, "web_url")?,
                };
                let mut saved = self.saved.lock();
                if saved
                    .reviews
                    .iter()
                    .any(|r| r.id == id && r.account != review.account)
                {
                    return Err("This MR is already open with another account. Close its review tab before switching accounts.".into());
                }
                if !saved.reviews.iter().any(|r| r.id == id) {
                    self.workspaces.reopen(&id);
                    saved.reviews.push(review.clone());
                    self.save(&mut saved)?;
                }
                Ok(response(
                    serde_json::to_value(saved.reviews.iter().find(|r| r.id == id).unwrap())
                        .unwrap(),
                ))
            }
            Close { review } => {
                self.workspaces.close(&review)?;
                self.drafts.forget(&review)?;
                self.versions.lock().remove(&review);
                let mut saved = self.saved.lock();
                saved.reviews.retain(|r| r.id != review);
                self.save(&mut saved)?;
                drop(saved);
                self.execute(Board)
            }
            AfterPush {
                account,
                remote_url,
                branch,
            } => {
                let c = self.credential(&account)?;
                let project = project_from_url(&c.account.host, &remote_url)?;
                let p = self.get(&c, &format!("projects/{}", encode(&project)))?;
                let id = number(&p, "id")?;
                let mut mrs = Vec::new();
                let mut page = 1;
                loop {
                    let r = self.request(
                        &c,
                        Method::GET,
                        "merge_requests",
                        &[
                            ("state", "opened".into()),
                            ("scope", "all".into()),
                            ("source_branch", branch.clone()),
                            ("page", page.to_string()),
                            ("per_page", "100".into()),
                        ],
                        None,
                        false,
                    )?;
                    mrs.extend(
                        r.data
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|mr| mr["source_project_id"].as_u64() == Some(id))
                            .cloned(),
                    );
                    if let Some(next) = r.next_page {
                        page = next;
                    } else {
                        break;
                    }
                }
                let create_url = format!(
                    "{}/-/merge_requests/new?merge_request%5Bsource_branch%5D={}",
                    string(&p, "web_url")?,
                    encode(&branch)
                );
                Ok(response(
                    json!({"mergeRequests":mrs,"createUrl":create_url,"project":project}),
                ))
            }
            other => self.review_request(other),
        }
    }
    fn review_request(&self, request: GitLabRequest) -> Result<GitLabResponse> {
        use GitLabRequest::*;
        let id = match &request {
            Detail { review }
            | Versions { review }
            | Diffs { review, .. }
            | Notes { review, .. }
            | Discussions { review, .. }
            | Comment { review, .. }
            | Resolve { review, .. }
            | Approve { review, .. }
            | Approvals { review }
            | Pipelines { review, .. }
            | Jobs { review, .. }
            | Trace { review, .. }
            | File { review, .. }
            | Checkout { review, .. }
            | SourceTree { review, .. }
            | SourceFile { review, .. } => review,
            _ => return Err("Invalid review request".into()),
        };
        let (r, c) = self.review(id)?;
        let base = format!("projects/{}/merge_requests/{}", r.project, r.iid);
        let mut path = base.clone();
        let mut method = Method::GET;
        let mut params = vec![];
        let mut body = None;
        let mut raw = false;
        match request {
            Detail { .. } => {
                let mut detail = self.get(&c, &base)?;
                if let Some(source) = detail["source_project_id"].as_u64()
                    && let Ok(project) = self.get(&c, &format!("projects/{source}"))
                {
                    detail["source_project"] = project;
                }
                return Ok(response(detail));
            }
            Versions { .. } => path.push_str("/versions"),
            Diffs { version, page, .. } => {
                path.push_str(&format!("/versions/{version}"));
                params.push(("page", page.to_string()));
            }
            Notes { page, .. } => {
                path.push_str("/notes");
                params.extend([
                    ("page", page.max(1).to_string()),
                    ("per_page", "100".into()),
                    ("sort", "asc".into()),
                    ("order_by", "created_at".into()),
                ]);
            }
            Discussions { page, .. } => {
                path.push_str("/discussions");
                params.extend([
                    ("page", page.max(1).to_string()),
                    ("per_page", "100".into()),
                ]);
            }
            Comment {
                body: text,
                discussion,
                position,
                ..
            } => {
                if text.trim().is_empty() {
                    return Err("Write a comment first".into());
                }
                method = Method::POST;
                path.push_str(
                    &discussion
                        .map(|id| format!("/discussions/{}/notes", encode(&id)))
                        .unwrap_or("/discussions".into()),
                );
                let mut v = json!({"body":text});
                if let Some(position) = position {
                    v["position"] = position;
                }
                body = Some(v);
            }
            Resolve {
                discussion,
                resolved,
                ..
            } => {
                path.push_str(&format!("/discussions/{}", encode(&discussion)));
                method = Method::PUT;
                body = Some(json!({"resolved":resolved}));
            }
            Approve { sha, undo, .. } => {
                path.push_str(if undo { "/unapprove" } else { "/approve" });
                method = Method::POST;
                body = Some(json!({"sha":sha}));
            }
            Approvals { .. } => path.push_str("/approvals"),
            Pipelines { page, .. } => {
                path.push_str("/pipelines");
                params.extend([("page", page.max(1).to_string()), ("per_page", "50".into())]);
            }
            Jobs {
                project,
                pipeline,
                page,
                bridges,
                ..
            } => {
                path = format!(
                    "projects/{project}/pipelines/{pipeline}/{}",
                    if bridges { "bridges" } else { "jobs" }
                );
                params.extend([
                    ("page", page.max(1).to_string()),
                    ("per_page", "100".into()),
                ]);
            }
            Trace { project, job, .. } => {
                let trace = self.request(
                    &c,
                    Method::GET,
                    &format!("projects/{project}/jobs/{job}/trace"),
                    &[],
                    None,
                    true,
                );
                let payload = match trace {
                    Ok(response) => {
                        json!({ "state": "available", "text": response.data, "message": null })
                    }
                    Err(error)
                        if error == "GitLab resource is unavailable or you do not have access" =>
                    {
                        // Archival itself does not imply deletion: always try the trace first.
                        let job = self.get(&c, &format!("projects/{project}/jobs/{job}"))?;
                        if !job["erased_at"].is_null() {
                            json!({ "state": "erased", "text": "", "message": "This job's log was erased in GitLab." })
                        } else if job["archived"].as_bool() == Some(true) {
                            json!({ "state": "archived", "text": "", "message": "This job is archived. GitLab no longer provides its log through the API." })
                        } else {
                            json!({ "state": "unavailable", "text": "", "message": "GitLab has no log available for this job." })
                        }
                    }
                    Err(error) => return Err(error),
                };
                return Ok(GitLabResponse {
                    data: payload,
                    next_page: None,
                });
            }
            File {
                project,
                path: file,
                sha,
                ..
            } => {
                validate_relative(&file)?;
                path = format!("projects/{project}/repository/files/{}/raw", encode(&file));
                params.push(("ref", sha));
                raw = true;
            }
            Checkout { sha, .. } => {
                let mr = self.get(&c, &base)?;
                let source = mr["source_project_id"]
                    .as_u64()
                    .ok_or("MR source project has been removed")?;
                let source_project = self.get(&c, &format!("projects/{source}"))?;
                let url = string(&source_project, "http_url_to_repo")?;
                host_url(&url)?;
                project_from_url(&c.account.host, &url)?;
                // Do not hold settings across a network operation: closing a review updates
                // settings before cancelling its fetch and must be able to reach that cancellation.
                let proxy = self.proxy.lock().clone();
                let root = self
                    .workspaces
                    .checkout(&r.id, &sha, &url, &c.token, &proxy)?;
                return Ok(response(json!({"root":root,"sha":sha})));
            }
            SourceTree { sha, .. } => {
                return Ok(response(json!(self.workspaces.files(&r.id, &sha)?)));
            }
            SourceFile { path, sha, .. } => {
                return Ok(response(json!(self.workspaces.file(&r.id, &sha, &path)?)));
            }
            _ => return Err("Invalid review operation".into()),
        }
        self.request(&c, method, &path, &params, body, raw)
    }
}
fn validate_relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\\')
        || std::path::Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Invalid repository-relative path".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls_and_encoding() {
        assert_eq!(
            project_from_url(
                "https://git.example/gitlab",
                "https://git.example/gitlab/group/sub/repo/-/merge_requests/42"
            )
            .unwrap(),
            "group/sub/repo"
        );
        assert_eq!(
            project_from_url("https://git.example", "git@git.example:group/repo.git").unwrap(),
            "group/repo"
        );
        assert!(
            project_from_url(
                "https://git.example/gitlab",
                "https://git.example/gitlab-other/repo"
            )
            .is_err()
        );
        assert!(project_from_url("https://git.example", "https://evil.example/a").is_err());
        assert_eq!(
            mr_iid("https://host/g/p/-/merge_requests/42/diffs#note_7").unwrap(),
            42
        );
        assert_eq!(encode("a/b #"), "a%2Fb%20%23");
    }
    #[test]
    fn paths_cannot_escape() {
        for p in ["../secret", "/tmp/x", "a/../b", "a\\b", ""] {
            assert!(validate_relative(p).is_err());
        }
        assert!(validate_relative("a/b.go").is_ok());
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    fn mock(
        status: &str,
        headers: &str,
        body: &str,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        mock_sequence(vec![(status, headers, body)])
    }
    fn mock_sequence(
        replies: Vec<(&str, &str, &str)>,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let replies: Vec<_> = replies.into_iter().map(|(status, headers, body)| format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len()
        )).collect();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 2048];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                tx.send(String::from_utf8(bytes).unwrap()).unwrap();
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        (host, rx, thread)
    }
    fn service() -> (GitLab, PathBuf) {
        let root = std::env::temp_dir().join(format!("cide-gitlab-http-{}", uuid::Uuid::new_v4()));
        let service = GitLab::load(root.join("accounts.json"), root.join("reviews")).unwrap();
        let mut proxy = cide_ipc::ProxySettings::default();
        proxy.scope.git = cide_ipc::ProxyTarget::Direct;
        service.set_proxy(proxy);
        (service, root)
    }
    fn account(host: String) -> Credential {
        Credential {
            account: GitLabAccount {
                id: "test".into(),
                host,
                username: "reviewer".into(),
                user_id: 7,
            },
            token: "private-test-token".into(),
        }
    }
    #[test]
    fn archived_and_erased_traces_have_explicit_states() {
        for (metadata, expected) in [
            (r#"{"archived":true,"erased_at":null}"#, "archived"),
            (r#"{"archived":true,"erased_at":"2026-01-01"}"#, "erased"),
            (r#"{"archived":false,"erased_at":null}"#, "unavailable"),
        ] {
            let (service, root) = service();
            let (host, requests, thread) =
                mock_sequence(vec![("404 Not Found", "", "{}"), ("200 OK", "", metadata)]);
            {
                let mut saved = service.saved.lock();
                saved.credentials.push(account(host));
                saved.reviews.push(GitLabReview {
                    id: "r".into(),
                    account: "test".into(),
                    project: 7,
                    iid: 42,
                    title: "test".into(),
                    url: "https://example.test".into(),
                });
            }
            let response = service
                .execute(GitLabRequest::Trace {
                    review: "r".into(),
                    project: 7,
                    job: 99,
                })
                .unwrap();
            assert_eq!(response.data["state"], expected);
            assert_eq!(response.data["text"], "");
            assert!(requests.recv().unwrap().contains("/jobs/99/trace"));
            assert!(requests.recv().unwrap().contains("/jobs/99?"));
            thread.join().unwrap();
            drop(service);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn traces_are_read_before_considering_archival_and_unsafe_redirects_are_refused() {
        let (service, root) = service();
        let (host, sent, thread) = mock("200 OK", "", "retained archived output");
        let response = service
            .request(
                &account(host),
                Method::GET,
                "projects/7/jobs/99/trace",
                &[],
                None,
                true,
            )
            .unwrap();
        assert_eq!(response.data, "retained archived output");
        sent.recv().unwrap();
        thread.join().unwrap();
        let (host, sent, thread) = mock(
            "302 Found",
            "Location: http://example.invalid/trace\r\n",
            "",
        );
        let error = service
            .request(
                &account(host),
                Method::GET,
                "projects/7/jobs/99/trace",
                &[],
                None,
                true,
            )
            .unwrap_err();
        assert!(error.contains("HTTPS"));
        assert!(!error.contains("private-test-token"));
        sent.recv().unwrap();
        thread.join().unwrap();
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn transport_scopes_token_and_preserves_pagination() {
        let (service, root) = service();
        let (host, request, thread) = mock("200 OK", "X-Next-Page: 2\r\n", "[{\"iid\":42}]");
        let answer = service
            .request(
                &account(host),
                Method::GET,
                "projects/group%2Frepo/merge_requests",
                &[("reviewer_id", "7".into()), ("search", "a b&c".into())],
                None,
                false,
            )
            .unwrap();
        assert_eq!(answer.next_page, Some(2));
        assert_eq!(answer.data[0]["iid"], 42);
        let sent = request.recv().unwrap();
        assert!(sent.starts_with("GET /api/v4/projects/group%2Frepo/merge_requests?"));
        assert!(sent.contains("search=a+b%26c"));
        assert!(
            sent.to_lowercase()
                .contains("private-token: private-test-token")
        );
        thread.join().unwrap();
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn redirects_do_not_receive_credentials_and_errors_are_redacted() {
        for (status, headers, expected) in [
            (
                "302 Found",
                "Location: https://other.example/steal\r\n",
                "HTTP 302",
            ),
            ("401 Unauthorized", "", "expired"),
            ("403 Forbidden", "", "denied"),
            ("409 Conflict", "", "changed"),
            ("429 Too Many Requests", "Retry-After: 30\r\n", "30 seconds"),
        ] {
            let (service, root) = service();
            let (host, request, thread) = mock(status, headers, "private-test-token");
            let error = service
                .request(&account(host), Method::GET, "user", &[], None, false)
                .unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("private-test-token"));
            request.recv().unwrap();
            thread.join().unwrap();
            drop(service);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn settings_persist_without_exposing_credentials_in_board() {
        let (service, root) = service();
        {
            let mut saved = service.saved.lock();
            saved
                .credentials
                .push(account("https://git.example".into()));
            service.save(&mut saved).unwrap();
        }
        assert!(
            !serde_json::to_string(&service.board())
                .unwrap()
                .contains("private-test-token")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&service.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(service);
        let restored = GitLab::load(root.join("accounts.json"), root.join("reviews")).unwrap();
        assert_eq!(restored.board().accounts.len(), 1);
        drop(restored);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn review_writes_use_exact_revisions_and_thread_identity() {
        let position = json!({"base_sha":"base","start_sha":"start","head_sha":"head","position_type":"text","old_path":"old.go","new_path":"new.go","old_line":3});
        let cases = vec![
            (
                GitLabRequest::Approve {
                    review: "review".into(),
                    sha: "reviewed-head".into(),
                    undo: false,
                },
                "POST",
                "/approve",
                json!({"sha":"reviewed-head"}),
            ),
            (
                GitLabRequest::Comment {
                    review: "review".into(),
                    body: "inline".into(),
                    discussion: None,
                    position: Some(position.clone()),
                },
                "POST",
                "/discussions",
                json!({"body":"inline","position":position}),
            ),
            (
                GitLabRequest::Comment {
                    review: "review".into(),
                    body: "reply".into(),
                    discussion: Some("thread-id".into()),
                    position: None,
                },
                "POST",
                "/discussions/thread-id/notes",
                json!({"body":"reply"}),
            ),
            (
                GitLabRequest::Resolve {
                    review: "review".into(),
                    discussion: "thread-id".into(),
                    resolved: true,
                },
                "PUT",
                "/discussions/thread-id",
                json!({"resolved":true}),
            ),
        ];
        for (operation, method, suffix, expected) in cases {
            let (service, root) = service();
            let (host, sent, thread) = mock("200 OK", "", "{}");
            {
                let mut saved = service.saved.lock();
                saved.credentials.push(account(host));
                saved.reviews.push(GitLabReview {
                    id: "review".into(),
                    account: "test".into(),
                    project: 7,
                    iid: 42,
                    title: "test".into(),
                    url: "https://example.test".into(),
                });
            }
            service.execute(operation).unwrap();
            let request = sent.recv().unwrap();
            assert!(request.starts_with(&format!(
                "{method} /api/v4/projects/7/merge_requests/42{suffix}"
            )));
            let body = request.split_once("\r\n\r\n").unwrap().1;
            assert_eq!(serde_json::from_str::<Value>(body).unwrap(), expected);
            thread.join().unwrap();
            drop(service);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn publishing_posts_the_body_alone_and_keeps_only_what_failed() {
        let (service, root) = service();
        let (host, sent, thread) =
            mock_sequence(vec![("201 Created", "", "{}"), ("409 Conflict", "", "{}")]);
        {
            let mut saved = service.saved.lock();
            saved.credentials.push(account(host));
            saved.reviews.push(GitLabReview {
                id: "review".into(),
                account: "test".into(),
                project: 7,
                iid: 42,
                title: "test".into(),
                url: "https://example.test".into(),
            });
        }
        // Seeded, so the only requests the mock sees are the two posts.
        service.versions.lock().insert(
            "review".into(),
            (
                std::time::Instant::now(),
                json!({
                    "base_commit_sha": "a".repeat(40),
                    "start_commit_sha": "b".repeat(40),
                    "head_commit_sha": "c".repeat(40),
                    "diffs": [{"old_path":"a.go","new_path":"a.go","diff":"@@ -1,1 +1,2 @@\n x\n+y\n"}]
                }),
            ),
        );
        let author = GitLabDraftAuthor {
            label: "Review !42".into(),
            harness: None,
            run: Some("run".into()),
        };
        let draft = |body: &str, line| NewDraft {
            severity: GitLabSeverity::Critical,
            body: body.into(),
            path: Some("a.go".into()),
            line: Some(line),
            side: GitLabSide::New,
        };
        let first = service
            .draft_create("review", draft("first", 2), author.clone())
            .unwrap();
        let second = service
            .draft_create("review", draft("second", 1), author)
            .unwrap();
        let outcome = service
            .execute(GitLabRequest::DraftPublish {
                review: "review".into(),
                drafts: vec![first.id.clone(), second.id.clone()],
            })
            .unwrap();
        let outcome: GitLabPublished = serde_json::from_value(outcome.data).unwrap();
        assert_eq!(outcome.published, vec![first.id.clone()]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].draft, second.id);

        let request = sent.recv().unwrap();
        assert!(request.starts_with("POST /api/v4/projects/7/merge_requests/42/discussions"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body["body"],
            json!("first"),
            "the severity is never published"
        );
        assert_eq!(body["position"]["new_line"], json!(2));
        let _ = sent.recv().unwrap();
        thread.join().unwrap();

        let left: Vec<_> = service.drafts("review").into_iter().map(|d| d.id).collect();
        assert_eq!(left, vec![second.id]);
        service
            .execute(GitLabRequest::Close {
                review: "review".into(),
            })
            .unwrap();
        assert!(service.drafts("review").is_empty());
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }
}

/// Use libgit2 and Cide's existing hunk mapping, including EOF markers and line coordinates.
fn comparison_from_text(
    doc: &cide_ipc::gitlab::GitLabDocument,
    old: String,
    new: String,
) -> Result<cide_ipc::history::RevisionDiff> {
    use cide_ipc::{
        git::FileState,
        history::{RevSide, RevisionDiff},
    };
    let mut patch = git2::Patch::from_buffers(
        old.as_bytes(),
        Some(std::path::Path::new(&doc.old_path)),
        new.as_bytes(),
        Some(std::path::Path::new(&doc.path)),
        None,
    )
    .map_err(|e| e.to_string())?;
    let buffer = patch.to_buf().map_err(|e| e.to_string())?;
    let hunks = if patch.num_hunks() == 0 {
        vec![]
    } else {
        let diff = git2::Diff::from_buffer(&buffer).map_err(|e| e.to_string())?;
        cide_git::diff::raw_file_at(&diff, 0)
            .map_err(|e| format!("{e:?}"))?
            .map(|raw| cide_git::diff::hunk_views(&raw))
            .unwrap_or_default()
    };
    drop(patch);
    Ok(RevisionDiff {
        path: doc.path.clone(),
        old_path: (doc.old_path != doc.path).then(|| doc.old_path.clone()),
        new: RevSide::Commit {
            oid: doc.head_sha.clone(),
        },
        old: RevSide::Commit {
            oid: doc.base_sha.clone(),
        },
        new_oid: (!doc.deleted_file).then(|| doc.head_sha.clone()),
        old_oid: (!doc.new_file).then(|| doc.base_sha.clone()),
        status: if doc.new_file {
            FileState::Added
        } else if doc.deleted_file {
            FileState::Deleted
        } else if doc.old_path != doc.path {
            FileState::Renamed
        } else {
            FileState::Modified
        },
        binary: false,
        old_mode: if doc.new_file { 0 } else { 0o100644 },
        new_mode: if doc.deleted_file { 0 } else { 0o100644 },
        hunks,
        old_text: (!doc.new_file).then_some(old),
        new_text: (!doc.deleted_file).then_some(new),
        texts_omitted: false,
    })
}

#[cfg(test)]
mod comparison_tests {
    use super::*;
    use cide_ipc::gitlab::{GitLabDocument, GitLabDocumentMode};
    fn document() -> GitLabDocument {
        GitLabDocument {
            review: "review".into(),
            path: "new.go".into(),
            old_path: "old.go".into(),
            base_sha: "a".repeat(40),
            start_sha: "b".repeat(40),
            head_sha: "c".repeat(40),
            mode: GitLabDocumentMode::Diff,
            new_file: false,
            deleted_file: false,
        }
    }
    #[test]
    fn shared_diff_preserves_rename_eof_and_comment_coordinates() {
        let diff = comparison_from_text(
            &document(),
            "context\nold".into(),
            "context\nnew\nextra\n".into(),
        )
        .unwrap();
        assert_eq!(diff.old_path.as_deref(), Some("old.go"));
        let lines = &diff.hunks[0].lines;
        assert_eq!(lines[0].old_lineno, Some(1));
        assert_eq!(lines[0].new_lineno, Some(1));
        assert_eq!(lines[1].old_lineno, Some(2));
        assert_eq!(lines[1].new_lineno, None);
        assert!(lines[1].no_newline);
        assert_eq!(lines[2].new_lineno, Some(2));
        assert_eq!(lines[3].new_lineno, Some(3));
        assert_eq!(diff.old_text.as_deref(), Some("context\nold"));
    }
    #[test]
    fn shared_diff_handles_added_deleted_and_identical_files() {
        let mut doc = document();
        doc.new_file = true;
        let added = comparison_from_text(&doc, "".into(), "new\n".into()).unwrap();
        assert!(added.old_text.is_none());
        assert_eq!(added.hunks[0].lines[0].new_lineno, Some(1));
        doc.new_file = false;
        doc.deleted_file = true;
        let deleted = comparison_from_text(&doc, "old\n".into(), "".into()).unwrap();
        assert!(deleted.new_text.is_none());
        assert_eq!(deleted.hunks[0].lines[0].old_lineno, Some(1));
        doc.deleted_file = false;
        assert!(
            comparison_from_text(&doc, "same\n".into(), "same\n".into())
                .unwrap()
                .hunks
                .is_empty()
        );
    }
}

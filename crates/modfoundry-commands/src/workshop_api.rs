//! `workshop.details`: Steam workshop titles, sizes, update times and cached
//! preview icons.

use modfoundry_vfs::{root_name, WriteTxn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DETAILS_URL: &str =
    "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";

const CHUNK: usize = 50;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const READ_TIMEOUT: Duration = Duration::from_secs(15);

const MAX_DETAILS_BYTES: usize = 1024 * 1024;

/// The largest preview image accepted from the workshop.
pub const MAX_ICON_BYTES: usize = 1024 * 1024;

/// One published workshop item as the details API reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopItem {
    pub id: String,
    pub title: String,
    pub preview_url: String,
    pub size: u64,
    pub updated: u64,
}

/// Failure of the details request or of the icon cache.
#[derive(Debug)]
pub enum WorkshopError {
    Json(serde_json::Error),
    MissingId,
    Fetch(String),
    Cache(String),
}

impl fmt::Display for WorkshopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkshopError::Json(e) => write!(f, "invalid details JSON: {e}"),
            WorkshopError::MissingId => write!(f, "a found workshop item has no publishedfileid"),
            WorkshopError::Fetch(e) => write!(f, "workshop request failed: {e}"),
            WorkshopError::Cache(e) => write!(f, "icon cache write failed: {e}"),
        }
    }
}

impl std::error::Error for WorkshopError {}

impl From<serde_json::Error> for WorkshopError {
    fn from(e: serde_json::Error) -> Self {
        WorkshopError::Json(e)
    }
}

/// The host interactions `workshop.details` needs, so tests drive it without
/// the network.
pub trait WorkshopFetch: Send + Sync {
    /// POST the form body to the details endpoint and return the JSON text.
    fn post_details(&self, body: &str) -> Result<String, String>;
    /// GET an allowed preview URL, returning its bytes and content type.
    fn get_image(&self, url: &str, max_bytes: usize) -> Result<(Vec<u8>, String), String>;
}

/// The real client: blocking HTTPS over rustls, so no async runtime and no
/// system OpenSSL.
pub struct HttpFetch;

/// No redirects: a preview URL was allowlisted by host, and a redirect could leave it.
fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(READ_TIMEOUT))
        .timeout_recv_body(Some(READ_TIMEOUT))
        .build()
        .new_agent()
}

impl WorkshopFetch for HttpFetch {
    fn post_details(&self, body: &str) -> Result<String, String> {
        let mut response = http_agent()
            .post(DETAILS_URL)
            .header("content-type", "application/x-www-form-urlencoded")
            .send(body)
            .map_err(|e| e.to_string())?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_DETAILS_BYTES as u64)
            .read_to_string()
            .map_err(|e| e.to_string())
    }

    fn get_image(&self, url: &str, max_bytes: usize) -> Result<(Vec<u8>, String), String> {
        let mut response = http_agent().get(url).call().map_err(|e| e.to_string())?;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(max_bytes as u64)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok((bytes, content_type))
    }
}

/// Arguments for the `workshop.details` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkshopDetailsArgs {
    pub ids: Vec<String>,
    pub cache_dir: PathBuf,
    #[serde(default = "download_icons_default")]
    pub download_icons: bool,
}

fn download_icons_default() -> bool {
    true
}

/// One item as the command result reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModDetails {
    pub id: String,
    pub title: String,
    pub size: u64,
    pub updated: u64,
    pub icon: Option<PathBuf>,
}

/// Fetch details and, unless turned off, cache the preview icons for
/// `args.ids` under `args.cache_dir`.
pub fn fetch_details<F: WorkshopFetch + ?Sized>(
    args: &WorkshopDetailsArgs,
    fetch: &F,
) -> Result<Vec<ModDetails>, WorkshopError> {
    let mut items = Vec::new();
    for chunk in args.ids.chunks(CHUNK) {
        let text = fetch
            .post_details(&details_body(chunk))
            .map_err(WorkshopError::Fetch)?;
        items.extend(parse_details(&text)?);
    }

    let position: HashMap<&str, usize> = args
        .ids
        .iter()
        .filter(|id| is_digits(id))
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    items.retain(|item| position.contains_key(item.id.as_str()));
    items.sort_by_key(|item| position[item.id.as_str()]);

    let mut details = Vec::with_capacity(items.len());
    for item in items {
        let icon = match args.download_icons {
            true => cache_icon(&args.cache_dir, &item, fetch)?,
            false => None,
        };
        details.push(ModDetails {
            id: item.id,
            title: item.title,
            size: item.size,
            updated: item.updated,
            icon,
        });
    }
    Ok(details)
}

/// Parse a `GetPublishedFileDetails` response into its found items.
pub fn parse_details(json: &str) -> Result<Vec<WorkshopItem>, WorkshopError> {
    let root: Value = serde_json::from_str(json)?;
    let Some(items) = root
        .get("response")
        .and_then(|response| response.get("publishedfiledetails"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    let mut found = Vec::new();
    for item in items {
        if number(item.get("result")) != Some(1) {
            continue;
        }
        let id = text(item.get("publishedfileid"))
            .filter(|id| !id.is_empty())
            .ok_or(WorkshopError::MissingId)?;
        found.push(WorkshopItem {
            id,
            title: text(item.get("title")).unwrap_or_default(),
            preview_url: text(item.get("preview_url")).unwrap_or_default(),
            size: number(item.get("file_size")).unwrap_or(0),
            updated: number(item.get("time_updated")).unwrap_or(0),
        });
    }
    Ok(found)
}

/// The form-encoded request body for `ids`, dropping anything that is not all
/// ASCII digits.
pub fn details_body(ids: &[String]) -> String {
    let kept: Vec<&String> = ids.iter().filter(|id| is_digits(id)).collect();
    let mut body = format!("itemcount={}", kept.len());
    for (i, id) in kept.iter().enumerate() {
        body.push_str(&format!("&publishedfileids[{i}]={id}"));
    }
    body
}

/// The cache file name for an icon: `<id>-<time_updated>.jpg|png`, `None` for a
/// content type the app cannot render.
pub fn icon_file_name(id: &str, time_updated: u64, content_type: &str) -> Option<String> {
    if !is_digits(id) {
        return None;
    }
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let extension = match media_type.as_str() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        _ => return None,
    };
    Some(icon_name(id, time_updated, extension))
}

fn icon_name(id: &str, updated: u64, extension: &str) -> String {
    format!("{id}-{updated}.{extension}")
}

/// Whether a `preview_url` may be fetched: https on a Steam image host.
pub fn allowed_preview_url(url: &str) -> bool {
    const HOSTS: [&str; 2] = [".steamusercontent.com", ".steamstatic.com"];

    let Some(rest) = url.split_once("://") else {
        return false;
    };
    let (scheme, authority) = rest;
    if !scheme.eq_ignore_ascii_case("https") || authority.contains('@') {
        return false;
    }
    let authority = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
    if authority.contains(':') {
        return false;
    }
    let host = authority.to_ascii_lowercase();
    HOSTS.iter().any(|suffix| host.ends_with(suffix))
}

fn number(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn is_digits(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

fn cached_icon(cache_dir: &Path, id: &str, updated: u64) -> Option<PathBuf> {
    for extension in ["jpg", "png"] {
        let path = cache_dir.join(icon_name(id, updated, extension));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn cache_icon<F: WorkshopFetch + ?Sized>(
    cache_dir: &Path,
    item: &WorkshopItem,
    fetch: &F,
) -> Result<Option<PathBuf>, WorkshopError> {
    if !allowed_preview_url(&item.preview_url) {
        return Ok(None);
    }
    if let Some(cached) = cached_icon(cache_dir, &item.id, item.updated) {
        return Ok(Some(cached));
    }
    let Ok((bytes, content_type)) = fetch.get_image(&item.preview_url, MAX_ICON_BYTES) else {
        return Ok(None);
    };
    let Some(name) = icon_file_name(&item.id, item.updated, &content_type) else {
        return Ok(None);
    };
    write_icon(cache_dir, &name, bytes)?;
    Ok(Some(cache_dir.join(name)))
}

fn write_icon(cache_dir: &Path, name: &str, bytes: Vec<u8>) -> Result<(), WorkshopError> {
    let mut txn = WriteTxn::unrecorded(root_name("cache", cache_dir), cache_dir);
    txn.write(name, bytes)
        .and_then(|()| txn.finish().map(|_| ()))
        .map_err(|e| WorkshopError::Cache(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    enum Reply {
        Json(String),
        Fail(String),
        EchoBody,
    }

    struct StubFetch {
        replies: Vec<Reply>,
        posts: AtomicUsize,
        images: HashMap<String, (Vec<u8>, String)>,
        gets: AtomicUsize,
    }

    impl StubFetch {
        fn new(replies: Vec<Reply>) -> Self {
            Self {
                replies,
                posts: AtomicUsize::new(0),
                images: HashMap::new(),
                gets: AtomicUsize::new(0),
            }
        }

        fn with_image(mut self, url: &str, bytes: Vec<u8>, content_type: &str) -> Self {
            self.images.insert(url.into(), (bytes, content_type.into()));
            self
        }

        fn posts(&self) -> usize {
            self.posts.load(Ordering::Relaxed)
        }

        fn gets(&self) -> usize {
            self.gets.load(Ordering::Relaxed)
        }
    }

    impl WorkshopFetch for StubFetch {
        fn post_details(&self, body: &str) -> Result<String, String> {
            let call = self.posts.fetch_add(1, Ordering::Relaxed);
            match self.replies.get(call) {
                Some(Reply::Json(json)) => Ok(json.clone()),
                Some(Reply::Fail(e)) => Err(e.clone()),
                Some(Reply::EchoBody) => {
                    let first = body
                        .split("publishedfileids[0]=")
                        .nth(1)
                        .and_then(|rest| rest.split('&').next())
                        .unwrap_or_default();
                    Ok(format!(
                        r#"{{"response":{{"publishedfiledetails":[{{"publishedfileid":"{first}","result":1,"title":"{body}","time_updated":1}}]}}}}"#
                    ))
                }
                None => Err("no canned details".into()),
            }
        }

        fn get_image(&self, url: &str, _max_bytes: usize) -> Result<(Vec<u8>, String), String> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            self.images
                .get(url)
                .cloned()
                .ok_or_else(|| "no canned image".into())
        }
    }

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mf-ws-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const PREVIEW: &str = "https://images.steamusercontent.com/x.jpg";

    fn one_item(id: &str, updated: u64) -> String {
        format!(
            r#"{{"response":{{"publishedfiledetails":[
                {{"publishedfileid":"{id}","result":1,"title":"Community Framework",
                  "preview_url":"{PREVIEW}","file_size":25123,"time_updated":{updated}}}]}}}}"#
        )
    }

    fn args(dir: &Path, ids: &[&str]) -> WorkshopDetailsArgs {
        WorkshopDetailsArgs {
            ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            cache_dir: dir.to_path_buf(),
            download_icons: true,
        }
    }

    #[test]
    fn parses_found_items_and_skips_result_not_one() {
        let json = r#"{"response":{"publishedfiledetails":[
            {"publishedfileid":"1559212036","result":1,"title":"Community Framework",
             "preview_url":"https://images.steamusercontent.com/a.jpg","file_size":25123,
             "time_updated":1759140000},
            {"publishedfileid":"999","result":9}]}}"#;
        let items = parse_details(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            WorkshopItem {
                id: "1559212036".into(),
                title: "Community Framework".into(),
                preview_url: "https://images.steamusercontent.com/a.jpg".into(),
                size: 25123,
                updated: 1759140000,
            }
        );
    }

    #[test]
    fn parses_numeric_strings_and_tolerates_missing_fields() {
        let json = r#"{"response":{"publishedfiledetails":[
            {"publishedfileid":"7","result":"1","title":"T","file_size":"123","time_updated":"456"},
            {"publishedfileid":8,"result":1}]}}"#;
        let items = parse_details(json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].size, 123);
        assert_eq!(items[0].updated, 456);
        assert_eq!(items[0].preview_url, "");
        assert_eq!(items[1].id, "8");
        assert_eq!(items[1].title, "");
        assert_eq!(items[1].size, 0);
        assert_eq!(items[1].updated, 0);
    }

    #[test]
    fn empty_and_missing_details_are_empty() {
        assert!(parse_details(r#"{"response":{"publishedfiledetails":[]}}"#)
            .unwrap()
            .is_empty());
        assert!(parse_details(r#"{"response":{}}"#).unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_json_and_a_found_item_without_an_id() {
        assert!(parse_details("not json").is_err());
        assert!(matches!(
            parse_details(r#"{"response":{"publishedfiledetails":[{"result":1}]}}"#),
            Err(WorkshopError::MissingId)
        ));
    }

    #[test]
    fn body_lists_only_digit_ids_with_indices() {
        let body = details_body(&[
            "1559212036".into(),
            "abc".into(),
            "".into(),
            "12x".into(),
            "1646187754".into(),
        ]);
        assert_eq!(
            body,
            "itemcount=2&publishedfileids[0]=1559212036&publishedfileids[1]=1646187754"
        );
        assert_eq!(details_body(&[]), "itemcount=0");
    }

    #[test]
    fn icon_name_follows_the_content_type() {
        assert_eq!(
            icon_file_name("7", 42, "image/jpeg").as_deref(),
            Some("7-42.jpg")
        );
        assert_eq!(
            icon_file_name("7", 42, "image/png; charset=binary").as_deref(),
            Some("7-42.png")
        );
        assert_eq!(icon_file_name("7", 42, "image/gif"), None);
        assert_eq!(icon_file_name("7", 42, ""), None);
        assert_eq!(icon_file_name("../a", 42, "image/jpeg"), None);
    }

    #[test]
    fn allows_only_https_steam_image_hosts() {
        for ok in [
            "https://images.steamusercontent.com/x.jpg",
            "https://cdn.steamstatic.com/x.png",
            "https://a.b.steamstatic.com/x?y=1#z",
            "https://images.steamusercontent.com",
        ] {
            assert!(allowed_preview_url(ok), "{ok}");
        }
        for bad in [
            "http://images.steamusercontent.com/x.jpg",
            "https://evilsteamusercontent.com/x.jpg",
            "https://images.steamusercontent.com.evil.com/x.jpg",
            "https://images.steamusercontent.com@evil.com/x.jpg",
            "https://evil.com@images.steamusercontent.com/x.jpg",
            "https://images.steamusercontent.com:8443/x.jpg",
            "https://images.steamusercontent.com:443/x.jpg",
            "ftp://images.steamusercontent.com/x.jpg",
            "https://steamusercontent.com/x.jpg",
            "not a url",
        ] {
            assert!(!allowed_preview_url(bad), "{bad}");
        }
    }

    #[test]
    fn writes_the_icon_and_reports_its_path() {
        let dir = tmp("icon");
        let stub = StubFetch::new(vec![Reply::Json(one_item("1559212036", 1759140000))])
            .with_image(PREVIEW, vec![0xFF, 0xD8, 0x01], "image/jpeg");

        let out = fetch_details(&args(&dir, &["1559212036"]), &stub).unwrap();
        assert_eq!(out.len(), 1);
        let icon = out[0].icon.clone().unwrap();
        assert_eq!(icon, dir.join("1559212036-1759140000.jpg"));
        assert_eq!(std::fs::read(&icon).unwrap(), vec![0xFF, 0xD8, 0x01]);
        assert_eq!(out[0].title, "Community Framework");
        assert_eq!(out[0].size, 25123);
        assert_eq!(out[0].updated, 1759140000);
        assert_eq!(stub.gets(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reuses_a_cached_icon_without_a_request() {
        let dir = tmp("cached");
        std::fs::write(dir.join("7-42.png"), [1, 2, 3]).unwrap();
        let stub = StubFetch::new(vec![Reply::Json(
            r#"{"response":{"publishedfiledetails":[{"publishedfileid":"7","result":1,"preview_url":"https://images.steamusercontent.com/x.png","time_updated":42}]}}"#.into(),
        )]);

        let out = fetch_details(&args(&dir, &["7"]), &stub).unwrap();
        assert_eq!(out[0].icon.as_deref(), Some(dir.join("7-42.png").as_path()));
        assert_eq!(stub.gets(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_details_when_an_icon_download_fails() {
        let dir = tmp("fail");
        let stub = StubFetch::new(vec![Reply::Json(one_item("7", 42))]);

        let out = fetch_details(&args(&dir, &["7"]), &stub).unwrap();
        assert_eq!(out[0].icon, None);
        assert_eq!(out[0].title, "Community Framework");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_a_disallowed_preview_url() {
        let dir = tmp("disallowed");
        let stub = StubFetch::new(vec![Reply::Json(
            r#"{"response":{"publishedfiledetails":[{"publishedfileid":"7","result":1,"title":"T","preview_url":"https://evil.example/x.jpg","time_updated":42}]}}"#.into(),
        )]);

        let out = fetch_details(&args(&dir, &["7"]), &stub).unwrap();
        assert_eq!(out[0].icon, None);
        assert_eq!(stub.gets(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn errors_when_the_details_call_fails() {
        let dir = tmp("details-fail");
        let stub = StubFetch::new(vec![Reply::Fail("http 500".into())]);
        let err = fetch_details(&args(&dir, &["7"]), &stub).unwrap_err();
        assert!(matches!(err, WorkshopError::Fetch(_)), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn honours_download_icons_false() {
        let dir = tmp("no-icons");
        let stub = StubFetch::new(vec![Reply::Json(one_item("7", 42))]).with_image(
            PREVIEW,
            vec![1],
            "image/jpeg",
        );

        let mut a = args(&dir, &["7"]);
        a.download_icons = false;
        let out = fetch_details(&a, &stub).unwrap();
        assert_eq!(out[0].icon, None);
        assert_eq!(stub.gets(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunks_120_ids_into_three_requests() {
        let dir = tmp("chunks");
        let ids: Vec<String> = (0..120).map(|i| (1000 + i).to_string()).collect();
        let stub = StubFetch::new(vec![Reply::EchoBody, Reply::EchoBody, Reply::EchoBody]);

        let a = WorkshopDetailsArgs {
            ids,
            cache_dir: dir.to_path_buf(),
            download_icons: false,
        };
        let bodies: Vec<String> = fetch_details(&a, &stub)
            .unwrap()
            .into_iter()
            .map(|d| d.title)
            .collect();

        assert_eq!(stub.posts(), 3);
        assert_eq!(bodies.len(), 3, "{bodies:?}");
        assert!(
            bodies[0].starts_with("itemcount=50&publishedfileids[0]=1000"),
            "{bodies:?}"
        );
        assert!(
            bodies[0].ends_with("publishedfileids[49]=1049"),
            "{bodies:?}"
        );
        assert!(
            bodies[1].starts_with("itemcount=50&publishedfileids[0]=1050"),
            "{bodies:?}"
        );
        assert!(
            bodies[2].starts_with("itemcount=20&publishedfileids[0]=1100"),
            "{bodies:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_found_items_in_input_order() {
        let dir = tmp("order");
        let stub = StubFetch::new(vec![Reply::Json(
            r#"{"response":{"publishedfiledetails":[
                {"publishedfileid":"8","result":1,"title":"B"},
                {"publishedfileid":"7","result":1,"title":"A"},
                {"publishedfileid":"9","result":9}]}}"#
                .into(),
        )]);

        let out = fetch_details(&args(&dir, &["7", "8"]), &stub).unwrap();
        let ids: Vec<&str> = out.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["7", "8"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_item_that_was_not_asked_for_is_dropped_and_not_cached() {
        let dir = tmp("unasked");
        let stub = StubFetch::new(vec![Reply::Json(
            r#"{"response":{"publishedfiledetails":[
                {"publishedfileid":"7","result":1,"title":"A","preview_url":"https://images.steamusercontent.com/x.jpg","time_updated":42},
                {"publishedfileid":"a/b","result":1,"title":"B","preview_url":"https://images.steamusercontent.com/x.jpg","time_updated":42}]}}"#
                .into(),
        )])
        .with_image(PREVIEW, vec![0xFF, 0xD8, 0x01], "image/jpeg");

        let out = fetch_details(&args(&dir, &["7"]), &stub).unwrap();
        let ids: Vec<&str> = out.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["7"]);
        assert!(!dir.join("a").exists());
        assert_eq!(stub.gets(), 1);
        let _ = std::fs::remove_dir_all(&dir);

        let dir = tmp("unasked-digits");
        let stub = StubFetch::new(vec![Reply::Json(one_item("99", 42))]).with_image(
            PREVIEW,
            vec![0xFF, 0xD8, 0x01],
            "image/jpeg",
        );
        let out = fetch_details(&args(&dir, &["7"]), &stub).unwrap();
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(stub.gets(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registered_with_the_network_effect() {
        let core = crate::registry::core();
        let cmd = core.command("workshop.details").expect("registered");
        assert_eq!(cmd.spec().effect, modfoundry_core::EffectKind::Network);
        let schema = cmd.args_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], serde_json::json!(["ids", "cache_dir"]));
        assert_eq!(schema["properties"]["ids"]["type"], "array");
        assert_eq!(schema["properties"]["download_icons"]["type"], "boolean");
        assert!(!cmd.previewable());
    }
}
